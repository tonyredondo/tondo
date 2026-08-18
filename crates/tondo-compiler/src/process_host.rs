use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command as OsCommand, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant as StdInstant};

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

use os_pipe::pipe;
use tondo_stdlib::testing::{
    FloatTolerance, Generator, MAX_SHRINK_CANDIDATES, TextDiff, diff_text,
};
use tondo_stdlib::{
    io as stdlib_io, json, math, messagepack, path, protobuf, serialization as stdlib_serialization,
};
use tondo_vm::runtime::{
    RuntimeHostValueKind, RuntimeValue, VmError, VmHost, VmTestNodeKind, VmTestNodeOutcome,
};

use crate::test_backend::{TestExecutionKind, TestParticipation};
use crate::test_control::{ControlError, EnvelopeHandle};

const INT_MIN: i128 = i64::MIN as i128;
const INT_MAX: i128 = i64::MAX as i128;
const NANOS_PER_MICROSECOND: i128 = 1_000;
const NANOS_PER_MILLISECOND: i128 = 1_000_000;
const NANOS_PER_SECOND: i128 = 1_000_000_000;
const DEFAULT_MAX_TIME_RESOURCES: usize = 1_048_576;
static NEXT_CLOCK_DOMAIN: AtomicU64 = AtomicU64::new(1);
static NEXT_ATOMIC_TEMP: AtomicU64 = AtomicU64::new(1);
static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);
const MAX_TEMP_DIRECTORY_ENTRIES: usize = 1_048_576;
const MAX_SHRINK_DEPTH: usize = 64;

fn testing_value_text(value: &RuntimeValue) -> String {
    const MAX_BYTES: usize = 1_024;
    let mut text = format!("{value:?}");
    if text.len() > MAX_BYTES {
        text.truncate(MAX_BYTES);
        text.push_str("...<truncated>");
    }
    text
}

fn push_shrink_candidate(
    candidates: &mut Vec<RuntimeValue>,
    candidate: RuntimeValue,
    limit: usize,
) {
    if candidates.len() < limit && !candidates.contains(&candidate) {
        candidates.push(candidate);
    }
}

fn generic_argument(name: &str) -> Option<&str> {
    let start = name.find('[')?;
    name.strip_suffix(']').map(|name| &name[start + 1..])
}

fn generic_parts<'a>(name: &'a str, constructor: &str) -> Option<Vec<&'a str>> {
    let prefix = name.strip_prefix(constructor)?.strip_prefix('[')?;
    let inner = prefix.strip_suffix(']')?;
    let mut depth = 0_u32;
    let mut start = 0_usize;
    let mut parts = Vec::new();
    for (index, character) in inner.char_indices() {
        match character {
            '[' => depth = depth.saturating_add(1),
            ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(inner[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(inner[start..].trim());
    Some(parts)
}

fn json_to_runtime(
    value: &json::JsonValue,
    expected: &str,
) -> Result<RuntimeValue, json::JsonErrorKind> {
    use json::JsonValue;
    let expected = expected.trim();
    match (expected, value) {
        ("Bool", JsonValue::Bool(value)) => Ok(RuntimeValue::Bool(*value)),
        ("String", JsonValue::String(value)) => Ok(RuntimeValue::String(value.clone())),
        ("Unit", JsonValue::Null) => Ok(RuntimeValue::Unit),
        ("Byte", JsonValue::Number(number)) => number
            .to_uint()
            .ok()
            .and_then(|value| u8::try_from(value).ok())
            .map(RuntimeValue::Byte)
            .ok_or(json::JsonErrorKind::NumberRange),
        ("Int" | "Int8" | "Int16" | "Int32" | "Int64", JsonValue::Number(number)) => {
            let value = number.to_int().map_err(|error| error.kind)?;
            let in_range = match expected {
                "Int8" => i8::try_from(value).is_ok(),
                "Int16" => i16::try_from(value).is_ok(),
                "Int32" => i32::try_from(value).is_ok(),
                "Int" | "Int64" => true,
                _ => unreachable!("integer match is exhaustive"),
            };
            in_range
                .then_some(RuntimeValue::Integer(i128::from(value)))
                .ok_or(json::JsonErrorKind::NumberRange)
        }
        ("UInt" | "UInt8" | "UInt16" | "UInt32" | "UInt64", JsonValue::Number(number)) => {
            let value = number.to_uint().map_err(|error| error.kind)?;
            let in_range = match expected {
                "UInt8" => u8::try_from(value).is_ok(),
                "UInt16" => u16::try_from(value).is_ok(),
                "UInt32" => u32::try_from(value).is_ok(),
                "UInt" | "UInt64" => true,
                _ => unreachable!("unsigned integer match is exhaustive"),
            };
            in_range
                .then_some(RuntimeValue::Integer(i128::from(value)))
                .ok_or(json::JsonErrorKind::NumberRange)
        }
        ("Float32", JsonValue::Number(number)) => number
            .to_float32()
            .map(|value| RuntimeValue::Float(f64::from(value)))
            .map_err(|error| error.kind),
        ("Float" | "Float64", JsonValue::Number(number)) => number
            .to_float64()
            .map(RuntimeValue::Float)
            .map_err(|error| error.kind),
        _ if expected.ends_with('?') => {
            if matches!(value, JsonValue::Null) {
                Ok(RuntimeValue::OptionNone)
            } else {
                json_to_runtime(value, &expected[..expected.len() - 1])
                    .map(Box::new)
                    .map(RuntimeValue::OptionSome)
            }
        }
        _ => {
            if let Some(parts) = generic_parts(expected, "Array")
                && let [item] = parts.as_slice()
                && let JsonValue::Array(values) = value
            {
                return values
                    .iter()
                    .map(|value| json_to_runtime(value, item))
                    .collect::<Result<Vec<_>, _>>()
                    .map(RuntimeValue::Array);
            }
            if let Some(parts) = generic_parts(expected, "Map")
                && let [key, item] = parts.as_slice()
                && *key == "String"
                && let JsonValue::Object(members) = value
            {
                return members
                    .iter()
                    .map(|member| {
                        Ok((
                            RuntimeValue::String(member.key.clone()),
                            json_to_runtime(&member.value, item)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, json::JsonErrorKind>>()
                    .map(RuntimeValue::Map);
            }
            Err(json::JsonErrorKind::TypeMismatch)
        }
    }
}

/// Mirrors the sealed `Shrink` prelude protocol for runtime values.  The
/// compiler proves the same shape before a Tondo call reaches this host
/// boundary; keeping the check here as well makes stale or forged bytecode
/// fail atomically instead of producing a partial candidate list.
fn shrink_runtime_value(
    value: &RuntimeValue,
    limit: usize,
    depth: usize,
) -> Result<Vec<RuntimeValue>, &'static str> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    if limit > MAX_SHRINK_CANDIDATES {
        return Err("shrink candidate limit exceeded");
    }
    if depth > MAX_SHRINK_DEPTH {
        return Err("shrink depth limit exceeded");
    }

    let mut candidates = Vec::new();
    match value {
        RuntimeValue::Integer(original) => {
            let mut current = *original;
            while current != 0 && candidates.len() < limit {
                current /= 2;
                push_shrink_candidate(&mut candidates, RuntimeValue::Integer(current), limit);
            }
        }
        RuntimeValue::Float(original) => {
            if !original.is_nan() {
                for candidate in [0.0, *original / 2.0, -*original / 2.0] {
                    if candidates.len() == limit {
                        break;
                    }
                    push_shrink_candidate(&mut candidates, RuntimeValue::Float(candidate), limit);
                }
            }
        }
        RuntimeValue::String(text) => {
            for length in 0..=text.len() {
                if candidates.len() == limit {
                    break;
                }
                let end = text.floor_char_boundary(length);
                push_shrink_candidate(
                    &mut candidates,
                    RuntimeValue::String(text[..end].to_owned()),
                    limit,
                );
            }
        }
        RuntimeValue::Array(values) => {
            for length in 0..=values.len() {
                if candidates.len() == limit {
                    break;
                }
                push_shrink_candidate(
                    &mut candidates,
                    RuntimeValue::Array(values[..length].to_vec()),
                    limit,
                );
            }
            for index in 0..values.len() {
                if candidates.len() == limit {
                    break;
                }
                let remaining = limit.saturating_sub(candidates.len());
                let element_candidates =
                    shrink_runtime_value(&values[index], remaining, depth + 1)?;
                for element in element_candidates {
                    if candidates.len() == limit {
                        break;
                    }
                    let mut candidate = values.clone();
                    candidate[index] = element;
                    push_shrink_candidate(&mut candidates, RuntimeValue::Array(candidate), limit);
                }
            }
        }
        _ => return Err("value does not implement the sealed Shrink protocol"),
    }

    Ok(candidates)
}

#[derive(Clone)]
struct ProcessStage {
    program: String,
    arguments: Vec<String>,
    merge_stderr: bool,
}

#[derive(Clone)]
struct ProcessPlan {
    stages: Vec<ProcessStage>,
}

#[derive(Debug, Clone)]
struct ExitStatus {
    code: Option<i32>,
    success: bool,
    downstream_closed_pipe: bool,
}

#[derive(Debug, Clone)]
struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    combined: Vec<u8>,
    statuses: Vec<ExitStatus>,
}

#[derive(Debug, Clone)]
struct EnvSnapshot {
    arguments: Vec<Vec<u8>>,
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
struct JsonLimitsInput {
    values: [i128; 8],
}

#[derive(Debug, Clone, Copy)]
struct JsonDecodeOptionsInput {
    limits: JsonLimitsInput,
    duplicate_keys: json::JsonDuplicatePolicy,
    unknown_fields: json::JsonUnknownFieldPolicy,
    numbers: json::JsonNumberPolicy,
}

#[derive(Debug, Clone, Copy)]
struct JsonEncodeOptionsInput {
    limits: JsonLimitsInput,
    canonical: bool,
}

enum HostValue {
    Command(ProcessPlan),
    Pipeline(ProcessPlan),
    Bytes(Vec<u8>),
    ExitStatus(ExitStatus),
    ProcessOutput(ProcessOutput),
    ProcessHandle(ProcessGroup),
    ProcessError {
        _message: String,
    },
    ProcessExitError {
        _output: ProcessOutput,
    },
    Utf8Error {
        _message: String,
    },
    BytesBuilder(Vec<u8>),
    BytesError {
        _message: String,
    },
    FormatBuilder(Vec<u8>),
    FormatError {
        _message: String,
    },
    TextError {
        _message: String,
    },
    CollectionError {
        _message: String,
    },
    Path(path::Path),
    PathError {
        _message: String,
    },
    File {
        file: std::fs::File,
        readable: bool,
        writable: bool,
    },
    Directory {
        path: PathBuf,
    },
    Metadata {
        _file: bool,
        _directory: bool,
        _symlink: bool,
        _len: u64,
        _readonly: bool,
    },
    OpenMode(FsOpenMode),
    FsError {
        _message: String,
    },
    MathError {
        _message: String,
    },
    FloatTolerance(FloatTolerance),
    FloatToleranceError {
        _message: String,
    },
    TextDiff(TextDiff),
    TempDirectory {
        path: PathBuf,
    },
    TempError {
        _message: String,
    },
    Generator(Generator),
    #[allow(dead_code)]
    GenerationId {
        seed: u64,
        case_index: u64,
    },
    GenerationError {
        _message: String,
    },
    Reader {
        stream: StreamKind,
        offset: usize,
    },
    Writer {
        stream: StreamKind,
    },
    IoLimits(stdlib_io::IoLimits),
    IoError {
        _message: String,
    },
    ConsoleError {
        _message: String,
    },
    Instant {
        domain: u64,
        nanos: i128,
    },
    Timer {
        domain: u64,
        deadline: i128,
    },
    DurationError {
        _message: String,
    },
    ClockError {
        _message: String,
    },
    EnvSnapshot(EnvSnapshot),
    EnvName(Vec<u8>),
    EnvValue(Vec<u8>),
    EnvError {
        _message: String,
    },
    VirtualTime {
        domain: u64,
    },
    JsonValue(json::JsonValue),
    JsonLimits(JsonLimitsInput),
    JsonDecodeOptions(JsonDecodeOptionsInput),
    JsonEncodeOptions(JsonEncodeOptionsInput),
    JsonDuplicatePolicy(json::JsonDuplicatePolicy),
    JsonUnknownFieldPolicy(json::JsonUnknownFieldPolicy),
    JsonNumberPolicy(json::JsonNumberPolicy),
    JsonValueView {
        _bytes: u64,
    },
    JsonRaw {
        _bytes: u64,
    },
    JsonNumber(json::JsonNumber),
    JsonReader {
        reader: json::JsonReader<'static>,
        finished: bool,
    },
    JsonSerializationReader {
        reader: json::JsonReader<'static>,
        depth: usize,
        finished: bool,
    },
    JsonWriter {
        writer: json::JsonWriter,
        stream: StreamKind,
        finished: bool,
    },
    JsonBufferWriter {
        writer: Option<json::JsonWriter>,
        error: Option<json::JsonErrorKind>,
        finished: bool,
    },
    MessagePackValue(messagepack::MessagePackValue),
    #[allow(dead_code)]
    MessagePackValueView(Vec<u8>),
    #[allow(dead_code)]
    MessagePackRaw(Vec<u8>),
    MessagePackTimestamp(messagepack::MessagePackTimestamp),
    MessagePackLimits(messagepack::MessagePackLimits),
    MessagePackDecodeOptions(messagepack::MessagePackDecodeOptions),
    MessagePackEncodeOptions(messagepack::MessagePackEncodeOptions),
    MessagePackDuplicatePolicy(messagepack::MessagePackDuplicatePolicy),
    MessagePackUnknownExtensionPolicy(messagepack::MessagePackUnknownExtensionPolicy),
    MessagePackNonMinimalPolicy(messagepack::MessagePackNonMinimalPolicy),
    MessagePackReader {
        reader: messagepack::MessagePackReader<'static>,
        finished: bool,
    },
    MessagePackWriter {
        writer: messagepack::MessagePackWriter,
        stream: Option<StreamKind>,
        finished: bool,
    },
    #[allow(dead_code)]
    ProtoDescriptor(String),
    ProtoLimits(protobuf::ProtoLimits),
    ProtoDecodeOptions(protobuf::ProtoDecodeOptions),
    ProtoEncodeOptions(protobuf::ProtoEncodeOptions),
    ProtoWireTypePolicy(protobuf::ProtoWireTypePolicy),
    ProtoUnknownPolicy(protobuf::ProtoUnknownPolicy),
    ProtoReader {
        reader: protobuf::ProtoReader<'static, ()>,
        finished: bool,
    },
    ProtoWriter {
        writer: protobuf::ProtoWriter,
        stream: Option<StreamKind>,
        finished: bool,
    },
    #[allow(dead_code)]
    UnknownFields(protobuf::UnknownFields),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Stdin,
    Stdout,
    Stderr,
}

fn json_error_kind_ordinal(kind: json::JsonErrorKind) -> u32 {
    match kind {
        json::JsonErrorKind::InvalidUtf8 => 0,
        json::JsonErrorKind::InvalidSyntax => 1,
        json::JsonErrorKind::UnexpectedEof => 2,
        json::JsonErrorKind::InvalidEscape => 3,
        json::JsonErrorKind::InvalidUnicodeScalar => 4,
        json::JsonErrorKind::InvalidNumber => 5,
        json::JsonErrorKind::DuplicateKey => 6,
        json::JsonErrorKind::UnknownField => 7,
        json::JsonErrorKind::MissingField => 8,
        json::JsonErrorKind::TypeMismatch => 9,
        json::JsonErrorKind::NumberRange => 10,
        json::JsonErrorKind::LimitExceeded => 11,
        json::JsonErrorKind::IoError => 12,
        json::JsonErrorKind::TrailingData => 13,
        json::JsonErrorKind::CanonicalizationError => 14,
    }
}

fn messagepack_error_kind_ordinal(kind: messagepack::MessagePackErrorKind) -> u32 {
    match kind {
        messagepack::MessagePackErrorKind::UnexpectedEof => 0,
        messagepack::MessagePackErrorKind::InvalidTag => 1,
        messagepack::MessagePackErrorKind::InvalidUtf8 => 2,
        messagepack::MessagePackErrorKind::InvalidLength => 3,
        messagepack::MessagePackErrorKind::NonMinimalEncoding => 4,
        messagepack::MessagePackErrorKind::InvalidExtension => 5,
        messagepack::MessagePackErrorKind::TypeMismatch => 6,
        messagepack::MessagePackErrorKind::DuplicateKey => 7,
        messagepack::MessagePackErrorKind::NumberRange => 8,
        messagepack::MessagePackErrorKind::DeterministicKeyCollision => 9,
        messagepack::MessagePackErrorKind::OutOfOrderKey => 10,
        messagepack::MessagePackErrorKind::LimitExceeded => 11,
        messagepack::MessagePackErrorKind::IoError => 12,
        messagepack::MessagePackErrorKind::TrailingData => 13,
    }
}

fn protobuf_error_kind_ordinal(kind: protobuf::ProtoErrorKind) -> u32 {
    match kind {
        protobuf::ProtoErrorKind::UnexpectedEof => 0,
        protobuf::ProtoErrorKind::InvalidTag => 1,
        protobuf::ProtoErrorKind::InvalidWireType => 2,
        protobuf::ProtoErrorKind::InvalidVarint => 3,
        protobuf::ProtoErrorKind::InvalidLength => 4,
        protobuf::ProtoErrorKind::InvalidUtf8 => 5,
        protobuf::ProtoErrorKind::TypeMismatch => 6,
        protobuf::ProtoErrorKind::InvalidPacked => 7,
        protobuf::ProtoErrorKind::NumberRange => 8,
        protobuf::ProtoErrorKind::InvalidFieldNumber => 9,
        protobuf::ProtoErrorKind::InvalidGroup => 10,
        protobuf::ProtoErrorKind::LimitExceeded => 11,
        protobuf::ProtoErrorKind::IoError => 12,
        protobuf::ProtoErrorKind::TrailingData => 13,
        protobuf::ProtoErrorKind::SchemaMismatch => 14,
    }
}

fn runtime_json_length(value: &RuntimeValue) -> Result<Option<usize>, VmError> {
    match value {
        RuntimeValue::OptionNone => Ok(None),
        RuntimeValue::OptionSome(value) => match value.as_ref() {
            RuntimeValue::Integer(value) => usize::try_from(*value)
                .map(Some)
                .map_err(|_| VmError::Host("JsonEvent length is outside usize".into())),
            _ => Err(VmError::Host("JsonEvent length is not an integer".into())),
        },
        _ => Err(VmError::Host("JsonEvent length is not optional".into())),
    }
}

fn runtime_json_length_value(value: Option<usize>) -> RuntimeValue {
    value.map_or(RuntimeValue::OptionNone, |value| {
        RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(value as i128)))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FsOpenMode {
    Read,
    Write,
    ReadWrite,
    Append,
    Create,
    CreateNew,
}

enum ClockProvider {
    Real { origin: StdInstant },
    Virtual { now: i128, resolution: i128 },
}

impl ClockProvider {
    fn real() -> Self {
        Self::Real {
            origin: StdInstant::now(),
        }
    }

    #[allow(dead_code)]
    fn virtual_time(resolution: i128) -> Result<Self, VmError> {
        if resolution <= 0 {
            return Err(VmError::Host(
                "virtual clock resolution must be positive".into(),
            ));
        }
        Ok(Self::Virtual { now: 0, resolution })
    }

    fn now(&self) -> Result<i128, VmError> {
        match self {
            Self::Real { origin } => i128::try_from(origin.elapsed().as_nanos())
                .map_err(|_| VmError::Host("monotonic clock value exceeds Int domain".into())),
            Self::Virtual { now, .. } => Ok(*now),
        }
    }

    fn resolution(&self) -> i128 {
        match self {
            Self::Real { .. } => 1,
            Self::Virtual { resolution, .. } => *resolution,
        }
    }

    #[allow(dead_code)]
    fn advance_virtual(&mut self, delta: i128) -> Result<(), VmError> {
        let Self::Virtual { now, .. } = self else {
            return Err(VmError::Host("cannot advance the real clock".into()));
        };
        if delta < 0 {
            return Err(VmError::Host("virtual clock cannot move backwards".into()));
        }
        let next = now
            .checked_add(delta)
            .ok_or_else(|| VmError::Host("virtual clock value overflow".into()))?;
        if next > INT_MAX {
            return Err(VmError::Host(
                "virtual clock value exceeds the Int domain".into(),
            ));
        }
        *now = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum CompletionMode {
    Status,
    Output,
    Run,
    Check,
    Cancel,
}

struct AsyncJob {
    receiver: mpsc::Receiver<Result<ProcessOutput, String>>,
    cancellation: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    mode: CompletionMode,
}

struct TimeJob {
    deadline: i128,
    cancellation: bool,
    completion: Option<RuntimeValue>,
    counts_resource: bool,
    kind: TimeJobKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeJobKind {
    Ordinary,
    Settle,
    Advance { target: i128 },
}

pub(crate) struct BootstrapHost {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    stdin: Vec<u8>,
    arguments: Vec<String>,
    environment: BTreeMap<Vec<u8>, Vec<u8>>,
    environment_available: bool,
    env_snapshot_id: Option<u64>,
    values: BTreeMap<u64, HostValue>,
    jobs: BTreeMap<u64, AsyncJob>,
    ready_jobs: BTreeMap<u64, Result<RuntimeValue, VmError>>,
    ready_fs_jobs: BTreeSet<u64>,
    time_jobs: BTreeMap<u64, TimeJob>,
    clock: ClockProvider,
    previous_clock: Option<(ClockProvider, u64)>,
    clock_domain: u64,
    virtual_controller: Option<u64>,
    next_value: u64,
    next_job: u64,
    max_bytes: u64,
    max_time_resources: usize,
    time_resources: usize,
    testing: Option<EnvelopeHandle>,
    testing_participation: Option<TestParticipation>,
    testing_stack: Vec<Option<EnvelopeHandle>>,
}

impl BootstrapHost {
    pub(crate) fn new(arguments: Vec<String>) -> Self {
        Self::with_max_bytes(arguments, u64::MAX)
    }

    pub(crate) fn with_max_bytes(arguments: Vec<String>, max_bytes: u64) -> Self {
        Self::with_limits(arguments, max_bytes, DEFAULT_MAX_TIME_RESOURCES)
    }

    #[allow(dead_code)]
    pub(crate) fn with_stdin(stdin: impl Into<Vec<u8>>) -> Self {
        let mut host = Self::with_limits(Vec::new(), u64::MAX, DEFAULT_MAX_TIME_RESOURCES);
        host.stdin = stdin.into();
        host
    }

    fn with_limits(arguments: Vec<String>, max_bytes: u64, max_time_resources: usize) -> Self {
        Self::with_environment_limits(
            arguments,
            BTreeMap::new(),
            true,
            max_bytes,
            max_time_resources,
        )
    }

    fn with_environment_limits(
        arguments: Vec<String>,
        environment: BTreeMap<Vec<u8>, Vec<u8>>,
        environment_available: bool,
        max_bytes: u64,
        max_time_resources: usize,
    ) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdin: Vec::new(),
            arguments,
            environment,
            environment_available,
            env_snapshot_id: None,
            values: BTreeMap::new(),
            jobs: BTreeMap::new(),
            ready_jobs: BTreeMap::new(),
            ready_fs_jobs: BTreeSet::new(),
            time_jobs: BTreeMap::new(),
            clock: ClockProvider::real(),
            previous_clock: None,
            clock_domain: NEXT_CLOCK_DOMAIN.fetch_add(1, Ordering::Relaxed),
            virtual_controller: None,
            next_value: 0,
            next_job: 0,
            max_bytes,
            max_time_resources,
            time_resources: 0,
            testing: None,
            testing_participation: None,
            testing_stack: Vec::new(),
        }
    }

    pub(crate) fn install_testing_envelope(&mut self, envelope: EnvelopeHandle) {
        self.testing = Some(envelope);
    }

    pub(crate) fn install_testing_participation(&mut self, participation: TestParticipation) {
        self.testing_participation = Some(participation);
    }

    fn testing_envelope(&self) -> Result<EnvelopeHandle, VmError> {
        self.testing.clone().ok_or_else(|| {
            VmError::Host("std.testing is only available inside a test worker".into())
        })
    }

    fn testing_result(
        envelope: &EnvelopeHandle,
        result: Result<(), ControlError>,
    ) -> Result<RuntimeValue, VmError> {
        match result {
            Ok(()) => Ok(RuntimeValue::Unit),
            Err(ControlError::FailNow { message }) => {
                let _ = envelope.fail_now(message);
                Ok(RuntimeValue::Unit)
            }
            Err(ControlError::Skip { reason }) => {
                let _ = envelope.skip(reason);
                Ok(RuntimeValue::Unit)
            }
            Err(error) => {
                let _ = envelope.fail_now(error.to_string());
                Ok(RuntimeValue::Unit)
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_environment(
        arguments: Vec<String>,
        environment: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    ) -> Self {
        Self::with_environment_limits(
            arguments,
            environment.into_iter().collect(),
            true,
            u64::MAX,
            DEFAULT_MAX_TIME_RESOURCES,
        )
    }

    #[allow(dead_code)]
    fn with_unavailable_environment(arguments: Vec<String>) -> Self {
        Self::with_environment_limits(
            arguments,
            BTreeMap::new(),
            false,
            u64::MAX,
            DEFAULT_MAX_TIME_RESOURCES,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn with_max_time_resources(
        arguments: Vec<String>,
        max_time_resources: usize,
    ) -> Self {
        Self::with_limits(arguments, u64::MAX, max_time_resources)
    }

    #[allow(dead_code)]
    pub(crate) fn with_virtual_time(
        arguments: Vec<String>,
        resolution: i128,
    ) -> Result<Self, VmError> {
        let mut host = Self::with_max_bytes(arguments, u64::MAX);
        host.clock = ClockProvider::virtual_time(resolution)?;
        Ok(host)
    }

    #[allow(dead_code)]
    pub(crate) fn advance_virtual_time(&mut self, delta: i128) -> Result<(), VmError> {
        self.clock.advance_virtual(delta)
    }

    pub(crate) fn take_stdout(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.stdout)
    }

    #[allow(dead_code)]
    pub(crate) fn take_stderr(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.stderr)
    }

    fn allocate(&mut self, kind: RuntimeHostValueKind, value: HostValue) -> RuntimeValue {
        let id = self.next_value;
        self.next_value = self
            .next_value
            .checked_add(1)
            .expect("host value identity space exhausted");
        self.values.insert(id, value);
        RuntimeValue::Host { kind, id }
    }

    fn process_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::ProcessError,
            HostValue::ProcessError {
                _message: message.into(),
            },
        )
    }

    fn result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.process_error(message)))
    }

    fn bytes_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::BytesError,
            HostValue::BytesError {
                _message: message.into(),
            },
        )
    }

    fn bytes_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.bytes_error(message)))
    }

    fn json_error(&self, error: &json::JsonError) -> RuntimeValue {
        self.json_error_parts(error.kind, error.location, &error.path)
    }

    fn json_result_structured_error(&self, error: &json::JsonError) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.json_error(error)))
    }

    fn json_result_error(&self, kind: json::JsonErrorKind) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.json_error_parts(
            kind,
            json::JsonLocation {
                offset: 0,
                line: 1,
                column: 1,
            },
            &json::JsonPath::root(),
        )))
    }

    fn json_error_parts(
        &self,
        kind: json::JsonErrorKind,
        location: json::JsonLocation,
        path: &json::JsonPath,
    ) -> RuntimeValue {
        RuntimeValue::Record {
            name: "JsonError".into(),
            values: vec![
                RuntimeValue::Variant {
                    name: "JsonErrorKind".into(),
                    variant: json_error_kind_ordinal(kind),
                    values: Vec::new(),
                },
                RuntimeValue::Record {
                    name: "JsonLocation".into(),
                    values: vec![
                        RuntimeValue::Integer(location.offset as i128),
                        RuntimeValue::Integer(location.line as i128),
                        RuntimeValue::Integer(location.column as i128),
                    ],
                },
                RuntimeValue::Newtype {
                    name: "JsonPath".into(),
                    value: Box::new(RuntimeValue::Array(
                        path.segments()
                            .iter()
                            .map(|segment| match segment {
                                json::JsonPathSegment::Key(key) => {
                                    RuntimeValue::String(key.clone())
                                }
                                json::JsonPathSegment::Index(index) => {
                                    RuntimeValue::String(index.to_string())
                                }
                            })
                            .collect(),
                    )),
                },
            ],
        }
    }

    fn format_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::FormatError,
            HostValue::FormatError {
                _message: message.into(),
            },
        )
    }

    fn format_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.format_error(message)))
    }

    fn text_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::TextError,
            HostValue::TextError {
                _message: message.into(),
            },
        )
    }

    fn text_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.text_error(message)))
    }

    fn collection_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::CollectionError,
            HostValue::CollectionError {
                _message: message.into(),
            },
        )
    }

    fn collection_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.collection_error(message)))
    }

    fn path(&self, value: &RuntimeValue) -> Result<&path::Path, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Path,
            id,
        } = value
        else {
            return Err(VmError::Host("Path value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Path(path)) => Ok(path),
            _ => Err(VmError::Host("Path token is stale".into())),
        }
    }

    fn filesystem_path(&self, value: &RuntimeValue) -> Result<PathBuf, String> {
        let path = self
            .path(value)
            .map_err(|error| format!("invalid Path value: {error}"))?;
        #[cfg(unix)]
        {
            Ok(PathBuf::from(OsString::from_vec(path.as_bytes().to_vec())))
        }
        #[cfg(not(unix))]
        {
            path.to_string()
                .map(PathBuf::from)
                .map_err(|error| format!("path is not representable on this target: {error:?}"))
        }
    }

    fn file_id(&self, value: &RuntimeValue) -> Result<u64, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::File,
            id,
        } = value
        else {
            return Err(VmError::Host("File value is invalid".into()));
        };
        if matches!(self.values.get(id), Some(HostValue::File { .. })) {
            Ok(*id)
        } else {
            Err(VmError::Host("File token is stale".into()))
        }
    }

    fn directory_path(&self, value: &RuntimeValue) -> Result<(u64, PathBuf), VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Directory,
            id,
        } = value
        else {
            return Err(VmError::Host("Directory value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Directory { path }) => Ok((*id, path.clone())),
            _ => Err(VmError::Host("Directory token is stale".into())),
        }
    }

    fn fs_open_mode(&self, value: &RuntimeValue) -> Result<FsOpenMode, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::OpenMode,
            id,
        } = value
        else {
            return Err(VmError::Host("OpenMode value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::OpenMode(mode)) => Ok(*mode),
            _ => Err(VmError::Host("OpenMode token is stale".into())),
        }
    }

    fn path_bytes(&self, value: &RuntimeValue) -> Result<Vec<u8>, VmError> {
        Ok(self.path(value)?.as_bytes().to_vec())
    }

    fn path_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::PathError,
            HostValue::PathError {
                _message: message.into(),
            },
        )
    }

    fn path_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.path_error(message)))
    }

    fn fs_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::FsError,
            HostValue::FsError {
                _message: message.into(),
            },
        )
    }

    fn fs_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.fs_error(message)))
    }

    fn math_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.allocate(
            RuntimeHostValueKind::MathError,
            HostValue::MathError {
                _message: message.into(),
            },
        )))
    }

    fn float_tolerance(&self, value: &RuntimeValue) -> Result<FloatTolerance, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::FloatTolerance,
            id,
        } = value
        else {
            return Err(VmError::Host("FloatTolerance value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::FloatTolerance(tolerance)) => Ok(*tolerance),
            _ => Err(VmError::Host("FloatTolerance token is stale".into())),
        }
    }

    fn float_tolerance_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::FloatToleranceError,
            HostValue::FloatToleranceError {
                _message: message.into(),
            },
        )
    }

    fn float_tolerance_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.float_tolerance_error(message)))
    }

    fn text_diff(&self, value: &RuntimeValue) -> Result<&TextDiff, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::TextDiff,
            id,
        } = value
        else {
            return Err(VmError::Host("TextDiff value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::TextDiff(diff)) => Ok(diff),
            _ => Err(VmError::Host("TextDiff token is stale".into())),
        }
    }

    fn temp_directory(&self, value: &RuntimeValue) -> Result<PathBuf, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::TempDirectory,
            id,
        } = value
        else {
            return Err(VmError::Host("TempDirectory value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::TempDirectory { path }) => Ok(path.clone()),
            _ => Err(VmError::Host("TempDirectory token is stale".into())),
        }
    }

    fn temp_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::TempError,
            HostValue::TempError {
                _message: message.into(),
            },
        )
    }

    fn temp_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.temp_error(message)))
    }

    fn generator_mut(&mut self, value: &RuntimeValue) -> Result<&mut Generator, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Generator,
            id,
        } = value
        else {
            return Err(VmError::Host("Generator value is invalid".into()));
        };
        match self.values.get_mut(id) {
            Some(HostValue::Generator(generator)) => Ok(generator),
            _ => Err(VmError::Host("Generator token is stale".into())),
        }
    }

    fn generation_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::GenerationError,
            HostValue::GenerationError {
                _message: message.into(),
            },
        )
    }

    fn generation_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.generation_error(message)))
    }

    fn reader_state(&self, value: &RuntimeValue) -> Result<(u64, StreamKind, usize), VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Reader,
            id,
        } = value
        else {
            return Err(VmError::Host("Reader value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Reader { stream, offset }) => Ok((*id, *stream, *offset)),
            _ => Err(VmError::Host("Reader token is stale".into())),
        }
    }

    fn writer_stream(&self, value: &RuntimeValue) -> Result<StreamKind, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Writer,
            id,
        } = value
        else {
            return Err(VmError::Host("Writer value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Writer { stream }) => Ok(*stream),
            _ => Err(VmError::Host("Writer token is stale".into())),
        }
    }

    fn io_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::IoError,
            HostValue::IoError {
                _message: message.into(),
            },
        )
    }

    fn io_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.io_error(message)))
    }

    fn io_limits(&self, value: &RuntimeValue) -> Result<stdlib_io::IoLimits, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::IoLimits,
            id,
        } = value
        else {
            return Err(VmError::Host("IoLimits value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::IoLimits(limits)) => Ok(*limits),
            _ => Err(VmError::Host("IoLimits token is stale".into())),
        }
    }

    fn console_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::ConsoleError,
            HostValue::ConsoleError {
                _message: message.into(),
            },
        )
    }

    fn console_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.console_error(message)))
    }

    fn valid_temp_prefix(prefix: &str) -> bool {
        prefix.len() <= 32
            && prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }

    fn remove_temp_tree(path: &std::path::Path, entries: &mut usize) -> io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "temporary tree contains a symlink",
            ));
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                *entries = entries.saturating_add(1);
                if *entries > MAX_TEMP_DIRECTORY_ENTRIES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "temporary tree entry limit exceeded",
                    ));
                }
                Self::remove_temp_tree(&entry?.path(), entries)?;
            }
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        }
    }

    fn duration_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::DurationError,
            HostValue::DurationError {
                _message: message.into(),
            },
        )
    }

    fn duration_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.duration_error(message)))
    }

    fn clock_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::ClockError,
            HostValue::ClockError {
                _message: message.into(),
            },
        )
    }

    fn clock_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.clock_error(message)))
    }

    fn env_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::EnvError,
            HostValue::EnvError {
                _message: message.into(),
            },
        )
    }

    fn env_result_error(&mut self, message: impl Into<String>) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.env_error(message)))
    }

    fn valid_environment_name(bytes: &[u8]) -> bool {
        !bytes.is_empty() && !bytes.iter().any(|byte| *byte == 0 || *byte == b'=')
    }

    fn environment_name(&self, value: &RuntimeValue) -> Result<Vec<u8>, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::EnvName,
            id,
        } = value
        else {
            return Err(VmError::Host("std.env.Name receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::EnvName(bytes)) => Ok(bytes.clone()),
            _ => Err(VmError::Host("std.env.Name token is stale".into())),
        }
    }

    fn environment_value(&self, value: &RuntimeValue) -> Result<Vec<u8>, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::EnvValue,
            id,
        } = value
        else {
            return Err(VmError::Host("std.env.Value receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::EnvValue(bytes)) => Ok(bytes.clone()),
            _ => Err(VmError::Host("std.env.Value token is stale".into())),
        }
    }

    fn environment_snapshot(&mut self) -> Result<RuntimeValue, RuntimeValue> {
        if !self.environment_available {
            return Err(self.env_result_error("environment snapshot is unavailable"));
        }
        if let Some(id) = self.env_snapshot_id {
            if matches!(self.values.get(&id), Some(HostValue::EnvSnapshot(_))) {
                return Ok(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::EnvSnapshot,
                    id,
                });
            }
            self.env_snapshot_id = None;
        }

        let host_arguments = self.arguments.clone();
        let host_environment = self.environment.clone();
        let mut total = 0_u64;
        let mut arguments = Vec::with_capacity(host_arguments.len());
        for argument in host_arguments {
            let bytes = argument.into_bytes();
            let length = u64::try_from(bytes.len()).map_err(|_| {
                self.env_result_error("environment argument length is not representable")
            })?;
            total = total
                .checked_add(length)
                .ok_or_else(|| self.env_result_error("environment snapshot byte count overflow"))?;
            arguments.push(bytes);
        }
        for (name, value) in &host_environment {
            if !Self::valid_environment_name(name) {
                return Err(self.env_result_error("environment contains an invalid name"));
            }
            for bytes in [name, value] {
                let length = u64::try_from(bytes.len()).map_err(|_| {
                    self.env_result_error("environment entry length is not representable")
                })?;
                total = total.checked_add(length).ok_or_else(|| {
                    self.env_result_error("environment snapshot byte count overflow")
                })?;
            }
        }
        if total > self.max_bytes {
            return Err(self.env_result_error("environment snapshot exceeds byte limit"));
        }

        let snapshot = self.allocate(
            RuntimeHostValueKind::EnvSnapshot,
            HostValue::EnvSnapshot(EnvSnapshot {
                arguments,
                entries: host_environment,
            }),
        );
        let RuntimeValue::Host { id, .. } = snapshot else {
            unreachable!("environment snapshots are host values")
        };
        self.env_snapshot_id = Some(id);
        Ok(snapshot)
    }

    fn environment_snapshot_data(&self, value: &RuntimeValue) -> Result<EnvSnapshot, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::EnvSnapshot,
            id,
        } = value
        else {
            return Err(VmError::Host("std.env.Snapshot receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::EnvSnapshot(snapshot)) => Ok(snapshot.clone()),
            _ => Err(VmError::Host("std.env.Snapshot token is stale".into())),
        }
    }

    fn duration(value: &RuntimeValue) -> Result<i128, VmError> {
        let RuntimeValue::Integer(value) = value else {
            return Err(VmError::Host("Duration is represented by an Int".into()));
        };
        if (INT_MIN..=INT_MAX).contains(value) {
            Ok(*value)
        } else {
            Err(VmError::Host("Duration is outside the Int domain".into()))
        }
    }

    fn instant(&self, value: &RuntimeValue) -> Result<(u64, i128), VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Instant,
            id,
        } = value
        else {
            return Err(VmError::Host("Instant receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Instant { domain, nanos }) => Ok((*domain, *nanos)),
            _ => Err(VmError::Host("Instant token is stale".into())),
        }
    }

    fn timer(&self, value: &RuntimeValue) -> Result<(u64, i128), VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Timer,
            id,
        } = value
        else {
            return Err(VmError::Host("Timer receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Timer { domain, deadline }) => Ok((*domain, *deadline)),
            _ => Err(VmError::Host("Timer token is stale".into())),
        }
    }

    fn allocate_instant(&mut self, nanos: i128) -> RuntimeValue {
        self.allocate(
            RuntimeHostValueKind::Instant,
            HostValue::Instant {
                domain: self.clock_domain,
                nanos,
            },
        )
    }

    fn allocate_timer(&mut self, deadline: i128) -> Result<RuntimeValue, RuntimeValue> {
        self.reserve_time_resource()?;
        Ok(self.allocate(
            RuntimeHostValueKind::Timer,
            HostValue::Timer {
                domain: self.clock_domain,
                deadline,
            },
        ))
    }

    fn reserve_time_resource(&mut self) -> Result<(), RuntimeValue> {
        if self.time_resources >= self.max_time_resources {
            return Err(self.clock_result_error("time resource limit reached"));
        }
        self.time_resources += 1;
        Ok(())
    }

    fn release_time_resource(&mut self) {
        self.time_resources = self
            .time_resources
            .checked_sub(1)
            .expect("time resource accounting underflow");
    }

    fn validate_delay(&mut self, value: &RuntimeValue) -> Result<i128, RuntimeValue> {
        let delay =
            Self::duration(value).map_err(|_| self.clock_result_error("invalid duration"))?;
        if delay < 0 {
            return Err(self.clock_result_error("delay must not be negative"));
        }
        Ok(delay)
    }

    fn deadline_value(&mut self, delay: &RuntimeValue) -> Result<RuntimeValue, RuntimeValue> {
        let delay =
            Self::duration(delay).map_err(|_| self.clock_result_error("invalid duration"))?;
        let now = self
            .clock
            .now()
            .map_err(|_| self.clock_result_error("monotonic clock is unavailable"))?;
        let deadline = now
            .checked_add(delay)
            .ok_or_else(|| self.clock_result_error("deadline exceeds the clock range"))?;
        Ok(self.allocate_instant(deadline))
    }

    fn timer_deadline_value(&mut self, delay: &RuntimeValue) -> Result<i128, RuntimeValue> {
        let delay = self.validate_delay(delay)?;
        let now = self
            .clock
            .now()
            .map_err(|_| self.clock_result_error("monotonic clock is unavailable"))?;
        now.checked_add(delay)
            .ok_or_else(|| self.clock_result_error("deadline exceeds the clock range"))
    }

    fn ensure_bytes_len(&self, length: usize) -> Result<(), String> {
        if u64::try_from(length).is_ok_and(|length| length <= self.max_bytes) {
            Ok(())
        } else {
            Err(format!(
                "byte buffer length {length} exceeds the configured limit {}",
                self.max_bytes
            ))
        }
    }

    fn plan(&self, value: &RuntimeValue) -> Result<ProcessPlan, VmError> {
        let RuntimeValue::Host { kind, id } = value else {
            return Err(VmError::Host("process plan is not a host value".into()));
        };
        match (kind, self.values.get(id)) {
            (RuntimeHostValueKind::Command, Some(HostValue::Command(plan)))
            | (RuntimeHostValueKind::Pipeline, Some(HostValue::Pipeline(plan))) => Ok(plan.clone()),
            _ => Err(VmError::Host("process plan token is invalid".into())),
        }
    }

    fn bytes(&self, value: &RuntimeValue) -> Result<&[u8], VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Bytes,
            id,
        } = value
        else {
            return Err(VmError::Host("Bytes receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::Bytes(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("Bytes token is stale".into())),
        }
    }

    fn bytes_id(&self, value: &RuntimeValue) -> Result<u64, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::Bytes,
            id,
        } = value
        else {
            return Err(VmError::Host("Bytes receiver is invalid".into()));
        };
        matches!(self.values.get(id), Some(HostValue::Bytes(_)))
            .then_some(*id)
            .ok_or_else(|| VmError::Host("Bytes token is stale".into()))
    }

    fn json_value(&self, value: &RuntimeValue) -> Result<&json::JsonValue, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(VmError::Host("JSON Value receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::JsonValue(value)) => Ok(value),
            _ => Err(VmError::Host("JSON Value token is stale".into())),
        }
    }

    fn json_limits_input(&self, value: &RuntimeValue) -> Result<JsonLimitsInput, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(VmError::Host("JsonLimits value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::JsonLimits(limits)) => Ok(*limits),
            _ => Err(VmError::Host("JsonLimits token is stale".into())),
        }
    }

    fn json_limits(&self, input: JsonLimitsInput) -> Result<json::JsonLimits, json::JsonErrorKind> {
        let [
            document,
            depth,
            array,
            object,
            string,
            number,
            events,
            output,
        ] = input.values;
        let convert =
            |value: i128| usize::try_from(value).map_err(|_| json::JsonErrorKind::LimitExceeded);
        Ok(json::JsonLimits {
            max_document_bytes: convert(document)?,
            max_depth: convert(depth)?,
            max_array_items: convert(array)?,
            max_object_members: convert(object)?,
            max_string_bytes: convert(string)?,
            max_number_bytes: convert(number)?,
            max_events: convert(events)?,
            max_output_bytes: convert(output)?,
        })
    }

    fn json_duplicate_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<json::JsonDuplicatePolicy, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(VmError::Host("JsonDuplicatePolicy value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::JsonDuplicatePolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host("JsonDuplicatePolicy token is stale".into())),
        }
    }

    fn json_unknown_field_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<json::JsonUnknownFieldPolicy, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(VmError::Host(
                "JsonUnknownFieldPolicy value is invalid".into(),
            ));
        };
        match self.values.get(id) {
            Some(HostValue::JsonUnknownFieldPolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host(
                "JsonUnknownFieldPolicy token is stale".into(),
            )),
        }
    }

    fn json_number_policy(&self, value: &RuntimeValue) -> Result<json::JsonNumberPolicy, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(VmError::Host("JsonNumberPolicy value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::JsonNumberPolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host("JsonNumberPolicy token is stale".into())),
        }
    }

    fn json_decode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<json::JsonDecodeOptions, json::JsonErrorKind> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(json::JsonErrorKind::TypeMismatch);
        };
        let Some(HostValue::JsonDecodeOptions(options)) = self.values.get(id) else {
            return Err(json::JsonErrorKind::IoError);
        };
        Ok(json::JsonDecodeOptions {
            limits: self.json_limits(options.limits)?,
            duplicate_keys: options.duplicate_keys,
            unknown_fields: options.unknown_fields,
            numbers: options.numbers,
        })
    }

    fn json_encode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<json::JsonEncodeOptions, json::JsonErrorKind> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonValue,
            id,
        } = value
        else {
            return Err(json::JsonErrorKind::TypeMismatch);
        };
        let Some(HostValue::JsonEncodeOptions(options)) = self.values.get(id) else {
            return Err(json::JsonErrorKind::IoError);
        };
        Ok(json::JsonEncodeOptions {
            limits: self.json_limits(options.limits)?,
            canonical: options.canonical,
        })
    }

    fn messagepack_error_parts(&self, error: &messagepack::MessagePackError) -> RuntimeValue {
        RuntimeValue::Record {
            name: "MessagePackError".into(),
            values: vec![
                RuntimeValue::Variant {
                    name: "MessagePackErrorKind".into(),
                    variant: messagepack_error_kind_ordinal(error.kind),
                    values: Vec::new(),
                },
                RuntimeValue::Integer(error.offset as i128),
                RuntimeValue::Newtype {
                    name: "MessagePackPath".into(),
                    value: Box::new(RuntimeValue::Array(Vec::new())),
                },
            ],
        }
    }

    fn messagepack_result_error(&self, error: &messagepack::MessagePackError) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.messagepack_error_parts(error)))
    }

    fn messagepack_result_kind(&self, kind: messagepack::MessagePackErrorKind) -> RuntimeValue {
        let error = messagepack::MessagePackError {
            kind,
            offset: 0,
            path: Default::default(),
        };
        self.messagepack_result_error(&error)
    }

    fn messagepack_limits(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackLimits, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("MessagePackLimits value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::MessagePackLimits(limits)) => Ok(*limits),
            _ => Err(VmError::Host("MessagePackLimits token is stale".into())),
        }
    }

    fn messagepack_duplicate_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackDuplicatePolicy, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host(
                "MessagePack duplicate policy is invalid".into(),
            ));
        };
        match self.values.get(id) {
            Some(HostValue::MessagePackDuplicatePolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host(
                "MessagePack duplicate policy is stale".into(),
            )),
        }
    }

    fn messagepack_unknown_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackUnknownExtensionPolicy, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host(
                "MessagePack extension policy is invalid".into(),
            ));
        };
        match self.values.get(id) {
            Some(HostValue::MessagePackUnknownExtensionPolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host(
                "MessagePack extension policy is stale".into(),
            )),
        }
    }

    fn messagepack_nonminimal_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackNonMinimalPolicy, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host(
                "MessagePack non-minimal policy is invalid".into(),
            ));
        };
        match self.values.get(id) {
            Some(HostValue::MessagePackNonMinimalPolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host(
                "MessagePack non-minimal policy is stale".into(),
            )),
        }
    }

    fn messagepack_decode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackDecodeOptions, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("MessagePackDecodeOptions is invalid".into()));
        };
        let Some(HostValue::MessagePackDecodeOptions(options)) = self.values.get(id) else {
            return Err(VmError::Host(
                "MessagePackDecodeOptions token is stale".into(),
            ));
        };
        Ok(*options)
    }

    fn messagepack_encode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackEncodeOptions, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("MessagePackEncodeOptions is invalid".into()));
        };
        let Some(HostValue::MessagePackEncodeOptions(options)) = self.values.get(id) else {
            return Err(VmError::Host(
                "MessagePackEncodeOptions token is stale".into(),
            ));
        };
        Ok(*options)
    }

    fn runtime_messagepack_value(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackValue, messagepack::MessagePackErrorKind> {
        match value {
            RuntimeValue::Host { kind, id } if *kind == RuntimeHostValueKind::MessagePackValue => {
                match self.values.get(id) {
                    Some(HostValue::MessagePackValue(value)) => Ok(value.clone()),
                    _ => Err(messagepack::MessagePackErrorKind::TypeMismatch),
                }
            }
            RuntimeValue::Unit => Ok(messagepack::MessagePackValue::Nil),
            RuntimeValue::Bool(value) => Ok(messagepack::MessagePackValue::Bool(*value)),
            RuntimeValue::Byte(value) => Ok(messagepack::MessagePackValue::UInt(u64::from(*value))),
            RuntimeValue::Integer(value) if *value >= 0 => Ok(messagepack::MessagePackValue::Int(
                i64::try_from(*value)
                    .map_err(|_| messagepack::MessagePackErrorKind::NumberRange)?,
            )),
            RuntimeValue::Integer(value) => Ok(messagepack::MessagePackValue::Int(
                i64::try_from(*value)
                    .map_err(|_| messagepack::MessagePackErrorKind::NumberRange)?,
            )),
            RuntimeValue::Float(value) => {
                Ok(messagepack::MessagePackValue::Float64(value.to_bits()))
            }
            RuntimeValue::String(value) => Ok(messagepack::MessagePackValue::String(value.clone())),
            RuntimeValue::Array(values) => values
                .iter()
                .map(|value| self.runtime_messagepack_value(value))
                .collect::<Result<Vec<_>, _>>()
                .map(messagepack::MessagePackValue::Array),
            RuntimeValue::Map(entries) => entries
                .iter()
                .map(|(key, value)| {
                    Ok(messagepack::MessagePackEntry {
                        key: self.runtime_messagepack_value(key)?,
                        value: self.runtime_messagepack_value(value)?,
                    })
                })
                .collect::<Result<Vec<_>, messagepack::MessagePackErrorKind>>()
                .map(messagepack::MessagePackValue::Map),
            _ => Err(messagepack::MessagePackErrorKind::TypeMismatch),
        }
    }

    fn messagepack_to_typed_runtime(
        &self,
        value: &messagepack::MessagePackValue,
        expected: &str,
    ) -> Result<RuntimeValue, messagepack::MessagePackErrorKind> {
        use messagepack::MessagePackValue as Value;
        let expected = expected.trim();
        match (expected, value) {
            ("Unit", Value::Nil) => Ok(RuntimeValue::Unit),
            ("Bool", Value::Bool(value)) => Ok(RuntimeValue::Bool(*value)),
            ("String", Value::String(value)) => Ok(RuntimeValue::String(value.clone())),
            ("Byte", Value::UInt(value)) => {
                Ok(RuntimeValue::Byte(u8::try_from(*value).map_err(|_| {
                    messagepack::MessagePackErrorKind::NumberRange
                })?))
            }
            ("Int" | "Int8" | "Int16" | "Int32" | "Int64", Value::Int(value)) => {
                let in_range = match expected {
                    "Int8" => i8::try_from(*value).is_ok(),
                    "Int16" => i16::try_from(*value).is_ok(),
                    "Int32" => i32::try_from(*value).is_ok(),
                    _ => true,
                };
                in_range
                    .then_some(RuntimeValue::Integer(i128::from(*value)))
                    .ok_or(messagepack::MessagePackErrorKind::NumberRange)
            }
            ("UInt" | "UInt8" | "UInt16" | "UInt32" | "UInt64", Value::UInt(value)) => {
                let in_range = match expected {
                    "UInt8" => u8::try_from(*value).is_ok(),
                    "UInt16" => u16::try_from(*value).is_ok(),
                    "UInt32" => u32::try_from(*value).is_ok(),
                    _ => true,
                };
                in_range
                    .then_some(RuntimeValue::Integer(i128::from(*value)))
                    .ok_or(messagepack::MessagePackErrorKind::NumberRange)
            }
            ("Float32", Value::Float32(bits)) => {
                Ok(RuntimeValue::Float(f32::from_bits(*bits) as f64))
            }
            ("Float" | "Float64", Value::Float64(bits)) => {
                Ok(RuntimeValue::Float(f64::from_bits(*bits)))
            }
            _ if expected.ends_with('?') => {
                if matches!(value, Value::Nil) {
                    Ok(RuntimeValue::OptionNone)
                } else {
                    self.messagepack_to_typed_runtime(value, &expected[..expected.len() - 1])
                        .map(Box::new)
                        .map(RuntimeValue::OptionSome)
                }
            }
            _ => {
                if let Some(parts) = generic_parts(expected, "Array")
                    && let [item] = parts.as_slice()
                    && let Value::Array(values) = value
                {
                    return values
                        .iter()
                        .map(|value| self.messagepack_to_typed_runtime(value, item))
                        .collect::<Result<Vec<_>, _>>()
                        .map(RuntimeValue::Array);
                }
                Err(messagepack::MessagePackErrorKind::TypeMismatch)
            }
        }
    }

    fn runtime_messagepack_event(&mut self, event: messagepack::MessagePackEvent) -> RuntimeValue {
        use messagepack::MessagePackEvent as Event;
        let (variant, values) = match event {
            Event::Nil => (0, Vec::new()),
            Event::Bool(value) => (1, vec![RuntimeValue::Bool(value)]),
            Event::Int(value) => (2, vec![RuntimeValue::Integer(i128::from(value))]),
            Event::UInt(value) => (3, vec![RuntimeValue::Integer(i128::from(value))]),
            Event::Float32(value) => (4, vec![RuntimeValue::Float(f32::from_bits(value) as f64)]),
            Event::Float64(value) => (5, vec![RuntimeValue::Float(f64::from_bits(value))]),
            Event::String(value) => (6, vec![RuntimeValue::String(value)]),
            Event::Binary(value) => (
                7,
                vec![self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(value))],
            ),
            Event::StartArray(value) => (
                8,
                vec![value.map_or(RuntimeValue::OptionNone, |value| {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(value as i128)))
                })],
            ),
            Event::EndArray => (9, Vec::new()),
            Event::StartMap(value) => (
                10,
                vec![value.map_or(RuntimeValue::OptionNone, |value| {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(value as i128)))
                })],
            ),
            Event::MapKey => (11, Vec::new()),
            Event::EndMap => (12, Vec::new()),
            Event::Ext(value) => (
                13,
                vec![RuntimeValue::Record {
                    name: "MessagePackExt".into(),
                    values: vec![
                        RuntimeValue::Integer(i128::from(value.type_code)),
                        self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(value.payload)),
                    ],
                }],
            ),
        };
        RuntimeValue::Variant {
            name: "MessagePackEvent".into(),
            variant,
            values,
        }
    }

    fn messagepack_event(
        &self,
        value: &RuntimeValue,
    ) -> Result<messagepack::MessagePackEvent, VmError> {
        let RuntimeValue::Variant {
            name,
            variant,
            values,
        } = value
        else {
            return Err(VmError::Host("MessagePackEvent value is invalid".into()));
        };
        if name != "MessagePackEvent" {
            return Err(VmError::Host("MessagePackEvent name is invalid".into()));
        }
        let value_at = |index: usize| {
            values
                .get(index)
                .ok_or_else(|| VmError::Host("MessagePackEvent payload is incomplete".into()))
        };
        Ok(match *variant {
            0 => messagepack::MessagePackEvent::Nil,
            1 => messagepack::MessagePackEvent::Bool(matches!(
                value_at(0)?,
                RuntimeValue::Bool(true)
            )),
            2 => messagepack::MessagePackEvent::Int(match value_at(0)? {
                RuntimeValue::Integer(value) => i64::try_from(*value)
                    .map_err(|_| VmError::Host("MessagePack Int out of range".into()))?,
                _ => return Err(VmError::Host("MessagePack Int payload invalid".into())),
            }),
            3 => messagepack::MessagePackEvent::UInt(match value_at(0)? {
                RuntimeValue::Integer(value) => u64::try_from(*value)
                    .map_err(|_| VmError::Host("MessagePack UInt out of range".into()))?,
                _ => return Err(VmError::Host("MessagePack UInt payload invalid".into())),
            }),
            4 => messagepack::MessagePackEvent::Float32(match value_at(0)? {
                RuntimeValue::Float(value) => (*value as f32).to_bits(),
                _ => return Err(VmError::Host("MessagePack Float32 payload invalid".into())),
            }),
            5 => messagepack::MessagePackEvent::Float64(match value_at(0)? {
                RuntimeValue::Float(value) => value.to_bits(),
                _ => return Err(VmError::Host("MessagePack Float64 payload invalid".into())),
            }),
            6 => messagepack::MessagePackEvent::String(match value_at(0)? {
                RuntimeValue::String(value) => value.clone(),
                _ => return Err(VmError::Host("MessagePack String payload invalid".into())),
            }),
            7 => messagepack::MessagePackEvent::Binary(self.bytes(value_at(0)?)?.to_vec()),
            8 => messagepack::MessagePackEvent::StartArray(
                runtime_json_length(value_at(0)?)
                    .map_err(|error| VmError::Host(error.to_string()))?,
            ),
            9 => messagepack::MessagePackEvent::EndArray,
            10 => messagepack::MessagePackEvent::StartMap(
                runtime_json_length(value_at(0)?)
                    .map_err(|error| VmError::Host(error.to_string()))?,
            ),
            11 => messagepack::MessagePackEvent::MapKey,
            12 => messagepack::MessagePackEvent::EndMap,
            13 => {
                let RuntimeValue::Record { values, .. } = value_at(0)? else {
                    return Err(VmError::Host("MessagePackExt payload invalid".into()));
                };
                let type_code = match values.first() {
                    Some(RuntimeValue::Integer(value)) => i8::try_from(*value)
                        .map_err(|_| VmError::Host("MessagePack ext type out of range".into()))?,
                    _ => return Err(VmError::Host("MessagePack ext type invalid".into())),
                };
                let payload = self
                    .bytes(
                        values
                            .get(1)
                            .ok_or_else(|| VmError::Host("MessagePack ext bytes missing".into()))?,
                    )?
                    .to_vec();
                messagepack::MessagePackEvent::Ext(messagepack::MessagePackExt {
                    type_code,
                    payload,
                })
            }
            _ => return Err(VmError::Host("MessagePackEvent variant is invalid".into())),
        })
    }

    fn protobuf_error_parts(&self, error: &protobuf::ProtoError) -> RuntimeValue {
        RuntimeValue::Record {
            name: "ProtoError".into(),
            values: vec![
                RuntimeValue::Variant {
                    name: "ProtoErrorKind".into(),
                    variant: protobuf_error_kind_ordinal(error.kind),
                    values: Vec::new(),
                },
                error.offset.map_or(RuntimeValue::OptionNone, |offset| {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(offset as i128)))
                }),
                RuntimeValue::Newtype {
                    name: "ProtoPath".into(),
                    value: Box::new(RuntimeValue::Array(Vec::new())),
                },
            ],
        }
    }

    fn protobuf_result_error(&self, error: &protobuf::ProtoError) -> RuntimeValue {
        RuntimeValue::ResultErr(Box::new(self.protobuf_error_parts(error)))
    }

    fn protobuf_result_kind(&self, kind: protobuf::ProtoErrorKind) -> RuntimeValue {
        let error = protobuf::ProtoError {
            kind,
            offset: None,
            path: protobuf::ProtoPath::root(),
        };
        self.protobuf_result_error(&error)
    }

    fn proto_limits(&self, value: &RuntimeValue) -> Result<protobuf::ProtoLimits, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("ProtoLimits value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProtoLimits(limits)) => Ok(*limits),
            _ => Err(VmError::Host("ProtoLimits token is stale".into())),
        }
    }

    fn proto_decode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<protobuf::ProtoDecodeOptions, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("ProtoDecodeOptions value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProtoDecodeOptions(options)) => Ok(*options),
            _ => Err(VmError::Host("ProtoDecodeOptions token is stale".into())),
        }
    }

    fn proto_encode_options(
        &self,
        value: &RuntimeValue,
    ) -> Result<protobuf::ProtoEncodeOptions, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("ProtoEncodeOptions value is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProtoEncodeOptions(options)) => Ok(*options),
            _ => Err(VmError::Host("ProtoEncodeOptions token is stale".into())),
        }
    }

    fn proto_wire_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<protobuf::ProtoWireTypePolicy, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("Proto wire policy is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProtoWireTypePolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host("Proto wire policy is stale".into())),
        }
    }

    fn proto_unknown_policy(
        &self,
        value: &RuntimeValue,
    ) -> Result<protobuf::ProtoUnknownPolicy, VmError> {
        let RuntimeValue::Host { id, .. } = value else {
            return Err(VmError::Host("Proto unknown policy is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProtoUnknownPolicy(policy)) => Ok(*policy),
            _ => Err(VmError::Host("Proto unknown policy is stale".into())),
        }
    }

    fn runtime_proto_event(&mut self, event: protobuf::ProtoEvent) -> RuntimeValue {
        use protobuf::ProtoEvent as Event;
        let (variant, values) = match event {
            Event::StartMessage(value) => (0, vec![RuntimeValue::String(value)]),
            Event::EndMessage => (1, Vec::new()),
            Event::Field(number, wire) => (
                2,
                vec![
                    RuntimeValue::Integer(number as i128),
                    self.runtime_proto_wire_type(wire),
                ],
            ),
            Event::Varint(value) => (3, vec![RuntimeValue::Integer(value as i128)]),
            Event::Fixed32(value) => (4, vec![RuntimeValue::Integer(value as i128)]),
            Event::Fixed64(value) => (5, vec![RuntimeValue::Integer(value as i128)]),
            Event::StartLengthDelimited(number) => (6, vec![RuntimeValue::Integer(number as i128)]),
            Event::Bytes(value) => (
                7,
                vec![self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(value))],
            ),
            Event::EndLengthDelimited => (8, Vec::new()),
            Event::StartPacked(number) => (9, vec![RuntimeValue::Integer(number as i128)]),
            Event::EndPacked => (10, Vec::new()),
            Event::Unknown(field) => (11, vec![self.runtime_unknown_field(field)]),
        };
        RuntimeValue::Variant {
            name: "ProtoEvent".into(),
            variant,
            values,
        }
    }

    fn runtime_proto_wire_type(&self, wire: protobuf::ProtoWireType) -> RuntimeValue {
        RuntimeValue::Variant {
            name: "ProtoWireType".into(),
            variant: wire as u32,
            values: Vec::new(),
        }
    }

    fn runtime_unknown_field(&mut self, field: protobuf::UnknownField) -> RuntimeValue {
        RuntimeValue::Record {
            name: "UnknownField".into(),
            values: vec![
                RuntimeValue::Integer(field.number as i128),
                self.runtime_proto_wire_type(field.wire_type),
                self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(field.tag_bytes),
                ),
                self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(field.payload_bytes),
                ),
            ],
        }
    }

    fn proto_wire_type(&self, value: &RuntimeValue) -> Result<protobuf::ProtoWireType, VmError> {
        let RuntimeValue::Variant { name, variant, .. } = value else {
            return Err(VmError::Host("ProtoWireType value is invalid".into()));
        };
        if name != "ProtoWireType" {
            return Err(VmError::Host("ProtoWireType name is invalid".into()));
        }
        match *variant {
            0 => Ok(protobuf::ProtoWireType::Varint),
            1 => Ok(protobuf::ProtoWireType::Fixed64),
            2 => Ok(protobuf::ProtoWireType::LengthDelimited),
            3 => Ok(protobuf::ProtoWireType::StartGroup),
            4 => Ok(protobuf::ProtoWireType::EndGroup),
            5 => Ok(protobuf::ProtoWireType::Fixed32),
            _ => Err(VmError::Host("ProtoWireType variant is invalid".into())),
        }
    }

    fn proto_event(&self, value: &RuntimeValue) -> Result<protobuf::ProtoEvent, VmError> {
        let RuntimeValue::Variant {
            name,
            variant,
            values,
        } = value
        else {
            return Err(VmError::Host("ProtoEvent value is invalid".into()));
        };
        if name != "ProtoEvent" {
            return Err(VmError::Host("ProtoEvent name is invalid".into()));
        }
        let payload = |index: usize| {
            values
                .get(index)
                .ok_or_else(|| VmError::Host("ProtoEvent payload is incomplete".into()))
        };
        Ok(match *variant {
            0 => protobuf::ProtoEvent::StartMessage(match payload(0)? {
                RuntimeValue::String(value) => value.clone(),
                _ => return Err(VmError::Host("Proto message name is invalid".into())),
            }),
            1 => protobuf::ProtoEvent::EndMessage,
            2 => protobuf::ProtoEvent::Field(
                match payload(0)? {
                    RuntimeValue::Integer(value) => u32::try_from(*value)
                        .map_err(|_| VmError::Host("Proto field number is out of range".into()))?,
                    _ => return Err(VmError::Host("Proto field number is invalid".into())),
                },
                self.proto_wire_type(payload(1)?)?,
            ),
            3 => protobuf::ProtoEvent::Varint(match payload(0)? {
                RuntimeValue::Integer(value) => u64::try_from(*value)
                    .map_err(|_| VmError::Host("Proto varint is out of range".into()))?,
                _ => return Err(VmError::Host("Proto varint is invalid".into())),
            }),
            4 => protobuf::ProtoEvent::Fixed32(match payload(0)? {
                RuntimeValue::Integer(value) => u32::try_from(*value)
                    .map_err(|_| VmError::Host("Proto fixed32 is out of range".into()))?,
                _ => return Err(VmError::Host("Proto fixed32 is invalid".into())),
            }),
            5 => protobuf::ProtoEvent::Fixed64(match payload(0)? {
                RuntimeValue::Integer(value) => u64::try_from(*value)
                    .map_err(|_| VmError::Host("Proto fixed64 is out of range".into()))?,
                _ => return Err(VmError::Host("Proto fixed64 is invalid".into())),
            }),
            6 => protobuf::ProtoEvent::StartLengthDelimited(match payload(0)? {
                RuntimeValue::Integer(value) => u32::try_from(*value)
                    .map_err(|_| VmError::Host("Proto field number is out of range".into()))?,
                _ => return Err(VmError::Host("Proto field number is invalid".into())),
            }),
            7 => protobuf::ProtoEvent::Bytes(self.bytes(payload(0)?)?.to_vec()),
            8 => protobuf::ProtoEvent::EndLengthDelimited,
            9 => protobuf::ProtoEvent::StartPacked(match payload(0)? {
                RuntimeValue::Integer(value) => u32::try_from(*value)
                    .map_err(|_| VmError::Host("Proto field number is out of range".into()))?,
                _ => return Err(VmError::Host("Proto field number is invalid".into())),
            }),
            10 => protobuf::ProtoEvent::EndPacked,
            _ => {
                return Err(VmError::Host(
                    "ProtoEvent variant is not supported by the host bridge".into(),
                ));
            }
        })
    }

    fn json_number(&self, value: &RuntimeValue) -> Result<&json::JsonNumber, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::JsonNumber,
            id,
        } = value
        else {
            return Err(VmError::Host("JsonNumber receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::JsonNumber(number)) => Ok(number),
            _ => Err(VmError::Host("JsonNumber token is stale".into())),
        }
    }

    fn json_event(&self, value: &RuntimeValue) -> Result<json::JsonEvent, VmError> {
        let RuntimeValue::Variant {
            name,
            variant,
            values,
        } = value
        else {
            return Err(VmError::Host("JsonEvent value is invalid".into()));
        };
        if name != "JsonEvent" {
            return Err(VmError::Host(
                "JsonEvent value has the wrong nominal type".into(),
            ));
        }
        match (*variant, values.as_slice()) {
            (0, [length]) => Ok(json::JsonEvent::StartArray(runtime_json_length(length)?)),
            (1, []) => Ok(json::JsonEvent::EndArray),
            (2, [length]) => Ok(json::JsonEvent::StartObject(runtime_json_length(length)?)),
            (3, []) => Ok(json::JsonEvent::EndObject),
            (4, [RuntimeValue::String(key)]) => Ok(json::JsonEvent::Key(key.clone())),
            (5, []) => Ok(json::JsonEvent::Null),
            (6, [RuntimeValue::Bool(value)]) => Ok(json::JsonEvent::Bool(*value)),
            (7, [number]) => Ok(json::JsonEvent::Number(self.json_number(number)?.clone())),
            (8, [RuntimeValue::String(value)]) => Ok(json::JsonEvent::String(value.clone())),
            _ => Err(VmError::Host("JsonEvent payload is invalid".into())),
        }
    }

    fn runtime_json_event(&mut self, event: json::JsonEvent) -> RuntimeValue {
        let (variant, values) = match event {
            json::JsonEvent::StartArray(length) => (0, vec![runtime_json_length_value(length)]),
            json::JsonEvent::EndArray => (1, Vec::new()),
            json::JsonEvent::StartObject(length) => (2, vec![runtime_json_length_value(length)]),
            json::JsonEvent::EndObject => (3, Vec::new()),
            json::JsonEvent::Key(key) => (4, vec![RuntimeValue::String(key)]),
            json::JsonEvent::Null => (5, Vec::new()),
            json::JsonEvent::Bool(value) => (6, vec![RuntimeValue::Bool(value)]),
            json::JsonEvent::Number(number) => (
                7,
                vec![self.allocate(
                    RuntimeHostValueKind::JsonNumber,
                    HostValue::JsonNumber(number),
                )],
            ),
            json::JsonEvent::String(value) => (8, vec![RuntimeValue::String(value)]),
        };
        RuntimeValue::Variant {
            name: "JsonEvent".into(),
            variant,
            values,
        }
    }

    fn runtime_serialization_event(&mut self, event: json::JsonEvent) -> RuntimeValue {
        let (variant, values) = match event {
            json::JsonEvent::StartArray(length) => (8, vec![runtime_json_length_value(length)]),
            json::JsonEvent::EndArray => (9, Vec::new()),
            json::JsonEvent::StartObject(length) => (
                13,
                vec![
                    RuntimeValue::String(String::new()),
                    runtime_json_length_value(length),
                ],
            ),
            json::JsonEvent::EndObject => (15, Vec::new()),
            json::JsonEvent::Key(key) => (14, vec![RuntimeValue::String(key)]),
            json::JsonEvent::Null => (0, Vec::new()),
            json::JsonEvent::Bool(value) => (1, vec![RuntimeValue::Bool(value)]),
            json::JsonEvent::Number(number) => {
                if let Ok(value) = number.to_int() {
                    (2, vec![RuntimeValue::Integer(i128::from(value))])
                } else if let Ok(value) = number.to_uint() {
                    (3, vec![RuntimeValue::Integer(i128::from(value))])
                } else if let Ok(value) = number.to_float64() {
                    (5, vec![RuntimeValue::Float(value)])
                } else {
                    (5, vec![RuntimeValue::Float(f64::NAN)])
                }
            }
            json::JsonEvent::String(value) => (6, vec![RuntimeValue::String(value)]),
        };
        RuntimeValue::Variant {
            name: "SerializationEvent".into(),
            variant,
            values,
        }
    }

    fn serialization_event_to_json(
        event: &RuntimeValue,
    ) -> Result<json::JsonEvent, json::JsonErrorKind> {
        let RuntimeValue::Variant {
            name,
            variant,
            values,
        } = event
        else {
            return Err(json::JsonErrorKind::TypeMismatch);
        };
        if name != "SerializationEvent" {
            return Err(json::JsonErrorKind::TypeMismatch);
        }
        match (*variant, values.as_slice()) {
            (0, []) => Ok(json::JsonEvent::Null),
            (1, [RuntimeValue::Bool(value)]) => Ok(json::JsonEvent::Bool(*value)),
            (2 | 3, [RuntimeValue::Integer(value)]) => json::JsonNumber::parse(&value.to_string())
                .map(json::JsonEvent::Number)
                .map_err(|error| error.kind),
            (4 | 5, [RuntimeValue::Float(value)]) if value.is_finite() => {
                json::JsonNumber::parse(&value.to_string())
                    .map(json::JsonEvent::Number)
                    .map_err(|error| error.kind)
            }
            (6, [RuntimeValue::String(value)]) => Ok(json::JsonEvent::String(value.clone())),
            (8, [length]) => Ok(json::JsonEvent::StartArray(
                runtime_json_length(length).map_err(|_| json::JsonErrorKind::TypeMismatch)?,
            )),
            (9, []) => Ok(json::JsonEvent::EndArray),
            (13, [RuntimeValue::String(_), length]) => Ok(json::JsonEvent::StartObject(
                runtime_json_length(length).map_err(|_| json::JsonErrorKind::TypeMismatch)?,
            )),
            (14, [RuntimeValue::String(value)]) => Ok(json::JsonEvent::Key(value.clone())),
            (15, []) => Ok(json::JsonEvent::EndObject),
            _ => Err(json::JsonErrorKind::TypeMismatch),
        }
    }

    fn runtime_json_value(
        &self,
        value: &RuntimeValue,
    ) -> Result<json::JsonValue, json::JsonErrorKind> {
        use json::{JsonMember, JsonNumber, JsonValue};
        match value {
            RuntimeValue::Unit | RuntimeValue::OptionNone => Ok(JsonValue::Null),
            RuntimeValue::Bool(value) => Ok(JsonValue::Bool(*value)),
            RuntimeValue::Integer(value) => match JsonNumber::parse(&value.to_string()) {
                Ok(number) => Ok(JsonValue::Number(number)),
                Err(error) => Err(error.kind),
            },
            RuntimeValue::Float(value) if value.is_finite() => {
                match JsonNumber::parse(&value.to_string()) {
                    Ok(number) => Ok(JsonValue::Number(number)),
                    Err(error) => Err(error.kind),
                }
            }
            RuntimeValue::Byte(value) => match JsonNumber::parse(&value.to_string()) {
                Ok(number) => Ok(JsonValue::Number(number)),
                Err(error) => Err(error.kind),
            },
            RuntimeValue::String(value) => Ok(JsonValue::String(value.clone())),
            RuntimeValue::Array(values) => values
                .iter()
                .map(|value| self.runtime_json_value(value))
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Array),
            RuntimeValue::Map(entries) => entries
                .iter()
                .map(|(key, value)| {
                    let RuntimeValue::String(key) = key else {
                        return Err(json::JsonErrorKind::TypeMismatch);
                    };
                    Ok(JsonMember {
                        key: key.clone(),
                        value: self.runtime_json_value(value)?,
                    })
                })
                .collect::<Result<Vec<_>, json::JsonErrorKind>>()
                .map(JsonValue::Object),
            RuntimeValue::OptionSome(value) => self.runtime_json_value(value),
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::JsonValue,
                id,
            } => match self.values.get(id) {
                Some(HostValue::JsonValue(value)) => Ok(value.clone()),
                _ => Err(json::JsonErrorKind::IoError),
            },
            RuntimeValue::Float(_) => Err(json::JsonErrorKind::NumberRange),
            _ => Err(json::JsonErrorKind::TypeMismatch),
        }
    }

    fn builder(&self, value: &RuntimeValue) -> Result<&[u8], VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::BytesBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("BytesBuilder receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::BytesBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("BytesBuilder token is stale".into())),
        }
    }

    fn builder_mut(&mut self, value: &RuntimeValue) -> Result<&mut Vec<u8>, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::BytesBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("BytesBuilder receiver is invalid".into()));
        };
        match self.values.get_mut(id) {
            Some(HostValue::BytesBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("BytesBuilder token is stale".into())),
        }
    }

    fn format_builder(&self, value: &RuntimeValue) -> Result<&[u8], VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::FormatBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("format Builder receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::FormatBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("format Builder token is stale".into())),
        }
    }

    fn format_builder_mut(&mut self, value: &RuntimeValue) -> Result<&mut Vec<u8>, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::FormatBuilder,
            id,
        } = value
        else {
            return Err(VmError::Host("format Builder receiver is invalid".into()));
        };
        match self.values.get_mut(id) {
            Some(HostValue::FormatBuilder(bytes)) => Ok(bytes),
            _ => Err(VmError::Host("format Builder token is stale".into())),
        }
    }

    fn array_bytes(value: &RuntimeValue) -> Result<Vec<u8>, VmError> {
        let RuntimeValue::Array(values) = value else {
            return Err(VmError::Host("expected Array[Byte]".into()));
        };
        values
            .iter()
            .map(|value| match value {
                RuntimeValue::Byte(value) => Ok(*value),
                _ => Err(VmError::Host("expected Array[Byte] element".into())),
            })
            .collect()
    }

    fn array_chars(value: &RuntimeValue) -> Result<String, VmError> {
        let RuntimeValue::Array(values) = value else {
            return Err(VmError::Host("expected Array[Char]".into()));
        };
        values
            .iter()
            .map(|value| match value {
                RuntimeValue::Char(value) => Ok(*value),
                _ => Err(VmError::Host("expected Array[Char] element".into())),
            })
            .collect()
    }

    fn bytes_array(bytes: &[u8]) -> RuntimeValue {
        RuntimeValue::Array(bytes.iter().copied().map(RuntimeValue::Byte).collect())
    }

    fn output(&self, value: &RuntimeValue) -> Result<ProcessOutput, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ProcessOutput,
            id,
        } = value
        else {
            return Err(VmError::Host("ProcessOutput receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ProcessOutput(output)) => Ok(output.clone()),
            _ => Err(VmError::Host("ProcessOutput token is stale".into())),
        }
    }

    fn status(&self, value: &RuntimeValue) -> Result<ExitStatus, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ExitStatus,
            id,
        } = value
        else {
            return Err(VmError::Host("ExitStatus receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::ExitStatus(status)) => Ok(status.clone()),
            _ => Err(VmError::Host("ExitStatus token is stale".into())),
        }
    }

    fn allocate_statuses(&mut self, statuses: Vec<ExitStatus>) -> RuntimeValue {
        RuntimeValue::Array(
            statuses
                .into_iter()
                .map(|status| {
                    self.allocate(
                        RuntimeHostValueKind::ExitStatus,
                        HostValue::ExitStatus(status),
                    )
                })
                .collect(),
        )
    }

    fn complete_output(
        &mut self,
        mode: CompletionMode,
        result: Result<ProcessOutput, String>,
    ) -> RuntimeValue {
        let output = match result {
            Ok(output) => output,
            Err(message) => return self.result_error(message),
        };
        if let Err(message) = self.ensure_bytes_len(output.stdout.len()) {
            return self.result_error(format!("process stdout exceeds byte limit: {message}"));
        }
        if let Err(message) = self.ensure_bytes_len(output.stderr.len()) {
            return self.result_error(format!("process stderr exceeds byte limit: {message}"));
        }
        if let Err(message) = self.ensure_bytes_len(output.combined.len()) {
            return self.result_error(format!(
                "combined process output exceeds byte limit: {message}"
            ));
        }
        match mode {
            CompletionMode::Status | CompletionMode::Run | CompletionMode::Cancel => {
                RuntimeValue::ResultOk(Box::new(self.allocate_statuses(output.statuses)))
            }
            CompletionMode::Output => {
                let output = self.allocate(
                    RuntimeHostValueKind::ProcessOutput,
                    HostValue::ProcessOutput(output),
                );
                RuntimeValue::ResultOk(Box::new(output))
            }
            CompletionMode::Check if check_succeeded(&output.statuses) => {
                let output = self.allocate(
                    RuntimeHostValueKind::ProcessOutput,
                    HostValue::ProcessOutput(output),
                );
                RuntimeValue::ResultOk(Box::new(output))
            }
            CompletionMode::Check => {
                let error = self.allocate(
                    RuntimeHostValueKind::ProcessExitError,
                    HostValue::ProcessExitError { _output: output },
                );
                RuntimeValue::ResultErr(Box::new(error))
            }
        }
    }

    fn spawn_job(
        &mut self,
        group: Result<ProcessGroup, ProcessPlan>,
        mode: CompletionMode,
    ) -> Result<u64, VmError> {
        let cancellation = match &group {
            Ok(group) => Arc::clone(&group.cancellation),
            Err(_) => Arc::new(AtomicBool::new(false)),
        };
        if matches!(mode, CompletionMode::Cancel) {
            cancellation.store(true, Ordering::Release);
        }
        let worker_cancellation = Arc::clone(&cancellation);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("tondo-process".into())
            .spawn(move || {
                let result = match group {
                    Ok(group) => group.finish(),
                    Err(plan) => ProcessGroup::spawn(&plan, worker_cancellation)
                        .and_then(ProcessGroup::finish),
                };
                let _ = sender.send(result);
            })
            .map_err(|error| VmError::Host(format!("cannot start process worker: {error}")))?;
        let id = self.next_job;
        self.next_job = self
            .next_job
            .checked_add(1)
            .ok_or_else(|| VmError::Host("async host identity space exhausted".into()))?;
        self.jobs.insert(
            id,
            AsyncJob {
                receiver,
                cancellation,
                worker: Some(worker),
                mode,
            },
        );
        Ok(id)
    }

    fn finish_job(
        &mut self,
        id: u64,
        result: Result<ProcessOutput, String>,
    ) -> Result<RuntimeValue, VmError> {
        let mut job = self
            .jobs
            .remove(&id)
            .ok_or_else(|| VmError::Host(format!("unknown async host call #{id}")))?;
        if let Some(worker) = job.worker.take() {
            worker
                .join()
                .map_err(|_| VmError::Host("process worker panicked".into()))?;
        }
        Ok(self.complete_output(job.mode, result))
    }

    fn mode(name: &str) -> Option<CompletionMode> {
        Some(match name.rsplit('.').next()? {
            "status" => CompletionMode::Status,
            "output" => CompletionMode::Output,
            "run" => CompletionMode::Run,
            "check" => CompletionMode::Check,
            "cancel" => CompletionMode::Cancel,
            _ => return None,
        })
    }

    fn next_job_id(&mut self) -> Result<u64, VmError> {
        let id = self.next_job;
        self.next_job = self
            .next_job
            .checked_add(1)
            .ok_or_else(|| VmError::Host("async host identity space exhausted".into()))?;
        Ok(id)
    }

    fn start_time_job(
        &mut self,
        deadline: i128,
        completion: Option<RuntimeValue>,
        already_reserved: bool,
    ) -> Result<u64, VmError> {
        let mut completion = completion;
        let counts_resource = if already_reserved {
            true
        } else if completion.is_some() {
            false
        } else if self.time_resources < self.max_time_resources {
            self.time_resources += 1;
            true
        } else {
            completion = Some(self.clock_result_error("time resource limit reached"));
            false
        };
        let id = match self.next_job_id() {
            Ok(id) => id,
            Err(error) => {
                if counts_resource {
                    self.release_time_resource();
                }
                return Err(error);
            }
        };
        self.time_jobs.insert(
            id,
            TimeJob {
                deadline,
                cancellation: false,
                completion,
                counts_resource,
                kind: TimeJobKind::Ordinary,
            },
        );
        Ok(id)
    }

    fn finish_time_job(&mut self, id: u64) -> Result<RuntimeValue, VmError> {
        let job = self
            .time_jobs
            .remove(&id)
            .ok_or_else(|| VmError::Host(format!("unknown async time call #{id}")))?;
        if job.counts_resource {
            self.release_time_resource();
        }
        if !job.cancellation {
            match job.kind {
                TimeJobKind::Settle => self
                    .testing_envelope()?
                    .record_runtime_virtual_settle()
                    .map_err(|error| VmError::Host(format!("{}: {error}", error.code())))?,
                TimeJobKind::Advance { target } => self
                    .testing_envelope()?
                    .record_runtime_virtual_advance(target)
                    .map_err(|error| VmError::Host(format!("{}: {error}", error.code())))?,
                TimeJobKind::Ordinary => {}
            }
        }
        Ok(job
            .completion
            .unwrap_or(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))))
    }

    fn start_virtual_control_job(&mut self, kind: TimeJobKind) -> Result<u64, VmError> {
        let id = self.next_job_id()?;
        self.time_jobs.insert(
            id,
            TimeJob {
                deadline: self.clock.now()?,
                cancellation: false,
                completion: Some(RuntimeValue::Unit),
                counts_resource: false,
                kind,
            },
        );
        Ok(id)
    }

    fn virtual_controller(&self, value: &RuntimeValue) -> Result<u64, VmError> {
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::VirtualTime,
            id,
        } = value
        else {
            return Err(VmError::Host("VirtualTime receiver is invalid".into()));
        };
        match self.values.get(id) {
            Some(HostValue::VirtualTime { domain })
                if self.virtual_controller == Some(*id) && *domain == self.clock_domain =>
            {
                Ok(*id)
            }
            _ => Err(VmError::Host("VirtualTime controller is stale".into())),
        }
    }
}

impl Default for BootstrapHost {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl VmHost for BootstrapHost {
    fn begin_virtual_time(&mut self) -> Result<RuntimeValue, VmError> {
        if self.previous_clock.is_some() || self.virtual_controller.is_some() {
            return Err(VmError::Host(
                "P2004: virtual time domain is already active".into(),
            ));
        }
        let envelope = self.testing_envelope()?;
        envelope
            .begin_runtime_virtual_time()
            .map_err(|error| VmError::Host(format!("{}: {error}", error.code())))?;
        let virtual_clock = ClockProvider::virtual_time(1)?;
        self.previous_clock = Some((
            std::mem::replace(&mut self.clock, virtual_clock),
            self.clock_domain,
        ));
        self.clock_domain = NEXT_CLOCK_DOMAIN.fetch_add(1, Ordering::Relaxed);
        let controller = self.allocate(
            RuntimeHostValueKind::VirtualTime,
            HostValue::VirtualTime {
                domain: self.clock_domain,
            },
        );
        let RuntimeValue::Host { id, .. } = controller else {
            unreachable!("allocate always returns a host token")
        };
        self.virtual_controller = Some(id);
        Ok(controller)
    }

    fn finish_virtual_time(&mut self, controller: &RuntimeValue) -> Result<(), VmError> {
        let id = self.virtual_controller(controller)?;
        let pending = (!self.time_jobs.is_empty())
            .then(|| VmError::Host("P2003: virtual time closed with pending local timers".into()));
        let elapsed_ns = self.clock.now()?;
        self.values.remove(&id);
        self.virtual_controller = None;
        let (previous, previous_domain) = self.previous_clock.take().ok_or_else(|| {
            VmError::Host("virtual time has no production clock to restore".into())
        })?;
        self.clock = previous;
        self.clock_domain = previous_domain;
        let envelope = self.testing_envelope()?;
        envelope
            .finish_runtime_virtual_time(elapsed_ns)
            .map_err(|error| VmError::Host(format!("{}: {error}", error.code())))?;
        if let Some(error) = pending {
            return Err(error);
        }
        Ok(())
    }

    fn is_virtual_quiescence_call(&self, call: u64) -> bool {
        self.time_jobs.get(&call).is_some_and(|job| {
            matches!(job.kind, TimeJobKind::Settle | TimeJobKind::Advance { .. })
        })
    }

    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        // Generic host callables are monomorphized in bytecode and carry their
        // type arguments in brackets (for example `assertSome[Int]`). The
        // host contract is owned by the unspecialized function name.
        let specialized_name = name;
        let name = name.split_once('[').map_or(name, |(base, _)| base);
        match (name, arguments) {
            ("std.console.print", [RuntimeValue::String(text)]) => {
                self.stdout.extend_from_slice(text.as_bytes());
                Ok(RuntimeValue::Unit)
            }
            ("std.console.println", [RuntimeValue::String(text)]) => {
                self.stdout.extend_from_slice(text.as_bytes());
                self.stdout.push(b'\n');
                Ok(RuntimeValue::Unit)
            }
            ("std.console.flush", []) => Ok(RuntimeValue::Unit),
            ("std.console.stdin", []) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                RuntimeHostValueKind::Reader,
                HostValue::Reader {
                    stream: StreamKind::Stdin,
                    offset: 0,
                },
            )))),
            ("std.console.stdout", []) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                RuntimeHostValueKind::Writer,
                HostValue::Writer {
                    stream: StreamKind::Stdout,
                },
            )))),
            ("std.console.stderr", []) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                RuntimeHostValueKind::Writer,
                HostValue::Writer {
                    stream: StreamKind::Stderr,
                },
            )))),
            ("std.console.readLine", [reader]) => {
                let (id, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(self.console_result_error("readLine requires stdin"));
                }
                if offset >= self.stdin.len() {
                    return Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone)));
                }
                let remaining = &self.stdin[offset..];
                let end = remaining
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(remaining.len(), |index| index + 1);
                let line = &remaining[..end];
                let value = line.strip_suffix(b"\n").unwrap_or(line);
                let value = match std::str::from_utf8(value) {
                    Ok(value) => value.to_owned(),
                    Err(_) => return Ok(self.console_result_error("stdin is not UTF-8")),
                };
                if let Some(HostValue::Reader { offset, .. }) = self.values.get_mut(&id) {
                    *offset = offset.saturating_add(end);
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                    Box::new(RuntimeValue::String(value)),
                ))))
            }
            ("std.io.defaultLimits", []) => Ok(self.allocate(
                RuntimeHostValueKind::IoLimits,
                HostValue::IoLimits(stdlib_io::IoLimits::default()),
            )),
            (
                "std.io.limits",
                [
                    RuntimeValue::Integer(max_bytes),
                    RuntimeValue::Integer(max_read),
                ],
            ) => {
                let Ok(max_bytes) = usize::try_from(*max_bytes) else {
                    return Ok(self.io_result_error("max_bytes is invalid"));
                };
                let Ok(max_read) = usize::try_from(*max_read) else {
                    return Ok(self.io_result_error("max_read is invalid"));
                };
                match stdlib_io::IoLimits::new(max_bytes, max_read) {
                    Ok(limits) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::IoLimits, HostValue::IoLimits(limits)),
                    ))),
                    Err(error) => Ok(self.io_result_error(format!("{error:?}"))),
                }
            }
            ("std.io.readAll", [reader, limits]) => {
                let limits = self.io_limits(limits)?;
                let (id, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(self.io_result_error("reader is not readable"));
                }
                let remaining = self.stdin.len().saturating_sub(offset);
                // The public helper checks the aggregate bound before touching
                // the handle, so a rejected operation cannot consume input.
                if remaining > limits.max_bytes {
                    return Ok(self.io_result_error("readAll exceeds max_bytes"));
                }
                let mut output = Vec::with_capacity(remaining);
                let mut cursor = offset;
                while cursor < self.stdin.len() {
                    let count = limits.max_read.min(self.stdin.len() - cursor);
                    if let Err(message) = self.ensure_bytes_len(count) {
                        return Ok(self.io_result_error(message));
                    }
                    output.extend_from_slice(&self.stdin[cursor..cursor + count]);
                    cursor += count;
                }
                if let Some(HostValue::Reader { offset, .. }) = self.values.get_mut(&id) {
                    *offset = cursor;
                }
                let bytes = self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output));
                Ok(RuntimeValue::ResultOk(Box::new(bytes)))
            }
            ("std.io.writeAll", [writer, bytes]) => {
                let bytes = self.bytes(bytes)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.io_result_error(message));
                }
                let stream = self.writer_stream(writer)?;
                if stream == StreamKind::Stdin {
                    return Ok(self.io_result_error("stdin is not writable"));
                }
                match stream {
                    StreamKind::Stdout => self.stdout.extend_from_slice(&bytes),
                    StreamKind::Stderr => self.stderr.extend_from_slice(&bytes),
                    StreamKind::Stdin => unreachable!("stdin rejected above"),
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.io.Reader.read", [reader, RuntimeValue::Integer(maximum)]) => {
                let Ok(maximum) = usize::try_from(*maximum) else {
                    return Ok(self.io_result_error("read length is invalid"));
                };
                if maximum == 0 {
                    return Ok(self.io_result_error("read length must be positive"));
                }
                let (id, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(self.io_result_error("reader is not readable"));
                }
                if offset >= self.stdin.len() {
                    return Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone)));
                }
                let count = maximum.min(self.stdin.len() - offset);
                if let Err(message) = self.ensure_bytes_len(count) {
                    return Ok(self.io_result_error(message));
                }
                let bytes = self.stdin[offset..offset + count].to_vec();
                if let Some(HostValue::Reader { offset, .. }) = self.values.get_mut(&id) {
                    *offset = offset.saturating_add(count);
                }
                let bytes = self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(bytes));
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                    Box::new(bytes),
                ))))
            }
            ("std.io.Writer.write", [writer, bytes]) => {
                let stream = self.writer_stream(writer)?;
                let bytes = self.bytes(bytes)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.io_result_error(message));
                }
                match stream {
                    StreamKind::Stdout => self.stdout.extend_from_slice(&bytes),
                    StreamKind::Stderr => self.stderr.extend_from_slice(&bytes),
                    StreamKind::Stdin => return Ok(self.io_result_error("stdin is not writable")),
                }
                let count = i128::try_from(bytes.len())
                    .map_err(|_| VmError::Host("write length does not fit in Int".into()))?;
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                    count,
                ))))
            }
            ("std.io.Writer.flush", [writer]) => {
                let stream = self.writer_stream(writer)?;
                if stream == StreamKind::Stdin {
                    return Ok(self.io_result_error("stdin is not writable"));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.math.floor", [RuntimeValue::Float(value)]) => {
                Ok(RuntimeValue::Float(math::floor(*value)))
            }
            ("std.math.ceil", [RuntimeValue::Float(value)]) => {
                Ok(RuntimeValue::Float(math::ceil(*value)))
            }
            ("std.math.round", [RuntimeValue::Float(value)]) => {
                Ok(RuntimeValue::Float(math::round(*value)))
            }
            ("std.math.truncate", [RuntimeValue::Float(value)]) => {
                Ok(RuntimeValue::Float(math::truncate(*value)))
            }
            ("std.math.abs", [RuntimeValue::Float(value)]) => {
                Ok(RuntimeValue::Float(math::abs(*value)))
            }
            ("std.math.sqrt", [RuntimeValue::Float(value)]) => {
                match tondo_stdlib::math::sqrt(*value) {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Float(value)))),
                    Err(error) => Ok(self.math_result_error(format!("{error:?}"))),
                }
            }
            (
                "std.math.fma",
                [
                    RuntimeValue::Float(a),
                    RuntimeValue::Float(b),
                    RuntimeValue::Float(c),
                ],
            ) => Ok(RuntimeValue::Float(math::fma(*a, *b, *c))),
            ("std.math.min", [RuntimeValue::Float(left), RuntimeValue::Float(right)]) => {
                Ok(RuntimeValue::Float(math::min(*left, *right)))
            }
            ("std.math.max", [RuntimeValue::Float(left), RuntimeValue::Float(right)]) => {
                Ok(RuntimeValue::Float(math::max(*left, *right)))
            }
            ("intrinsic.messagepack.MessagePackDuplicatePolicy.Preserve", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackDuplicatePolicy(
                    messagepack::MessagePackDuplicatePolicy::Preserve,
                ),
            )),
            ("intrinsic.messagepack.MessagePackDuplicatePolicy.Reject", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackDuplicatePolicy(
                    messagepack::MessagePackDuplicatePolicy::Reject,
                ),
            )),
            ("intrinsic.messagepack.MessagePackDuplicatePolicy.First", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackDuplicatePolicy(
                    messagepack::MessagePackDuplicatePolicy::First,
                ),
            )),
            ("intrinsic.messagepack.MessagePackDuplicatePolicy.Last", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackDuplicatePolicy(
                    messagepack::MessagePackDuplicatePolicy::Last,
                ),
            )),
            ("intrinsic.messagepack.MessagePackUnknownExtensionPolicy.Preserve", []) => Ok(self
                .allocate(
                    RuntimeHostValueKind::MessagePackValue,
                    HostValue::MessagePackUnknownExtensionPolicy(
                        messagepack::MessagePackUnknownExtensionPolicy::Preserve,
                    ),
                )),
            ("intrinsic.messagepack.MessagePackUnknownExtensionPolicy.Reject", []) => Ok(self
                .allocate(
                    RuntimeHostValueKind::MessagePackValue,
                    HostValue::MessagePackUnknownExtensionPolicy(
                        messagepack::MessagePackUnknownExtensionPolicy::Reject,
                    ),
                )),
            ("intrinsic.messagepack.MessagePackNonMinimalPolicy.Accept", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackNonMinimalPolicy(
                    messagepack::MessagePackNonMinimalPolicy::Accept,
                ),
            )),
            ("intrinsic.messagepack.MessagePackNonMinimalPolicy.Reject", []) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackNonMinimalPolicy(
                    messagepack::MessagePackNonMinimalPolicy::Reject,
                ),
            )),
            ("intrinsic.messagepack.MessagePackLimits.construct", values) if values.len() == 9 => {
                let mut converted = [0usize; 9];
                for (index, value) in values.iter().enumerate() {
                    converted[index] = match value {
                        RuntimeValue::Integer(value) => usize::try_from(*value).map_err(|_| {
                            VmError::Host("MessagePack limit is out of range".into())
                        })?,
                        _ => {
                            return Err(VmError::Host(
                                "MessagePack limit is not an integer".into(),
                            ));
                        }
                    };
                }
                Ok(self.allocate(
                    RuntimeHostValueKind::MessagePackValue,
                    HostValue::MessagePackLimits(messagepack::MessagePackLimits {
                        max_document_bytes: converted[0],
                        max_depth: converted[1],
                        max_array_items: converted[2],
                        max_map_pairs: converted[3],
                        max_string_bytes: converted[4],
                        max_binary_bytes: converted[5],
                        max_ext_bytes: converted[6],
                        max_events: converted[7],
                        max_output_bytes: converted[8],
                    }),
                ))
            }
            (
                "intrinsic.messagepack.MessagePackDecodeOptions.construct",
                [limits, dynamic, typed, nonminimal, unknown],
            ) => {
                let limits = self.messagepack_limits(limits)?;
                let dynamic = self.messagepack_duplicate_policy(dynamic)?;
                let typed = self.messagepack_duplicate_policy(typed)?;
                let nonminimal = self.messagepack_nonminimal_policy(nonminimal)?;
                let unknown = self.messagepack_unknown_policy(unknown)?;
                Ok(self.allocate(
                    RuntimeHostValueKind::MessagePackValue,
                    HostValue::MessagePackDecodeOptions(messagepack::MessagePackDecodeOptions {
                        limits,
                        dynamic_map_duplicates: dynamic,
                        typed_map_duplicates: typed,
                        non_minimal: nonminimal,
                        unknown_extensions: unknown,
                    }),
                ))
            }
            (
                "intrinsic.messagepack.MessagePackEncodeOptions.construct",
                [limits, RuntimeValue::Bool(deterministic)],
            ) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackValue,
                HostValue::MessagePackEncodeOptions(messagepack::MessagePackEncodeOptions {
                    limits: self.messagepack_limits(limits)?,
                    deterministic: *deterministic,
                }),
            )),
            ("intrinsic.protobuf.ProtoWireTypePolicy.PreserveUnknown", []) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoWireTypePolicy,
                HostValue::ProtoWireTypePolicy(protobuf::ProtoWireTypePolicy::PreserveUnknown),
            )),
            ("intrinsic.protobuf.ProtoWireTypePolicy.Reject", []) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoWireTypePolicy,
                HostValue::ProtoWireTypePolicy(protobuf::ProtoWireTypePolicy::Reject),
            )),
            ("intrinsic.protobuf.ProtoUnknownPolicy.Preserve", []) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoUnknownPolicy,
                HostValue::ProtoUnknownPolicy(protobuf::ProtoUnknownPolicy::Preserve),
            )),
            ("intrinsic.protobuf.ProtoUnknownPolicy.Discard", []) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoUnknownPolicy,
                HostValue::ProtoUnknownPolicy(protobuf::ProtoUnknownPolicy::Discard),
            )),
            ("intrinsic.protobuf.ProtoLimits.construct", values) if values.len() == 16 => {
                let mut converted = [0usize; 16];
                for (index, value) in values.iter().enumerate() {
                    converted[index] = match value {
                        RuntimeValue::Integer(value) => usize::try_from(*value)
                            .map_err(|_| VmError::Host("Proto limit is out of range".into()))?,
                        _ => return Err(VmError::Host("Proto limit is not an integer".into())),
                    };
                }
                Ok(self.allocate(
                    RuntimeHostValueKind::ProtoLimits,
                    HostValue::ProtoLimits(protobuf::ProtoLimits {
                        max_schema_bytes: converted[0],
                        max_imports: converted[1],
                        max_generated_types: converted[2],
                        max_generated_bytes: converted[3],
                        max_message_bytes: converted[4],
                        max_depth: converted[5],
                        max_fields: converted[6],
                        max_repeated_items: converted[7],
                        max_map_entries: converted[8],
                        max_string_bytes: converted[9],
                        max_bytes_field_bytes: converted[10],
                        max_packed_bytes: converted[11],
                        max_unknown_bytes: converted[12],
                        max_varint_bytes: converted[13],
                        max_events: converted[14],
                        max_output_bytes: converted[15],
                    }),
                ))
            }
            (
                "intrinsic.protobuf.ProtoDecodeOptions.construct",
                [
                    limits,
                    wire_type,
                    unknown,
                    RuntimeValue::Bool(reject_nonminimal),
                ],
            ) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoDecodeOptions,
                HostValue::ProtoDecodeOptions(protobuf::ProtoDecodeOptions {
                    limits: self.proto_limits(limits)?,
                    wire_type: self.proto_wire_policy(wire_type)?,
                    unknown_fields: self.proto_unknown_policy(unknown)?,
                    reject_non_minimal_varints: *reject_nonminimal,
                }),
            )),
            (
                "intrinsic.protobuf.ProtoEncodeOptions.construct",
                [limits, RuntimeValue::Bool(deterministic)],
            ) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoEncodeOptions,
                HostValue::ProtoEncodeOptions(protobuf::ProtoEncodeOptions {
                    limits: self.proto_limits(limits)?,
                    deterministic: *deterministic,
                }),
            )),
            ("intrinsic.json.JsonDuplicatePolicy.Reject", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonDuplicatePolicy(json::JsonDuplicatePolicy::Reject),
            )),
            ("intrinsic.json.JsonDuplicatePolicy.First", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonDuplicatePolicy(json::JsonDuplicatePolicy::First),
            )),
            ("intrinsic.json.JsonDuplicatePolicy.Last", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonDuplicatePolicy(json::JsonDuplicatePolicy::Last),
            )),
            ("intrinsic.json.JsonUnknownFieldPolicy.Reject", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonUnknownFieldPolicy(json::JsonUnknownFieldPolicy::Reject),
            )),
            ("intrinsic.json.JsonUnknownFieldPolicy.Ignore", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonUnknownFieldPolicy(json::JsonUnknownFieldPolicy::Ignore),
            )),
            ("intrinsic.json.JsonUnknownFieldPolicy.Capture", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonUnknownFieldPolicy(json::JsonUnknownFieldPolicy::Capture),
            )),
            ("intrinsic.json.JsonNumberPolicy.Exact", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonNumberPolicy(json::JsonNumberPolicy::Exact),
            )),
            ("intrinsic.json.JsonNumberPolicy.Float32", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonNumberPolicy(json::JsonNumberPolicy::Float32),
            )),
            ("intrinsic.json.JsonNumberPolicy.Float64", []) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonNumberPolicy(json::JsonNumberPolicy::Float64),
            )),
            (
                "intrinsic.json.JsonLimits.construct",
                [
                    RuntimeValue::Integer(document),
                    RuntimeValue::Integer(depth),
                    RuntimeValue::Integer(array),
                    RuntimeValue::Integer(object),
                    RuntimeValue::Integer(string),
                    RuntimeValue::Integer(number),
                    RuntimeValue::Integer(events),
                    RuntimeValue::Integer(output),
                ],
            ) => Ok(self.allocate(
                RuntimeHostValueKind::JsonValue,
                HostValue::JsonLimits(JsonLimitsInput {
                    values: [
                        *document, *depth, *array, *object, *string, *number, *events, *output,
                    ],
                }),
            )),
            (
                "intrinsic.json.JsonDecodeOptions.construct",
                [limits, duplicate, unknown, numbers],
            ) => {
                let duplicate_keys = self.json_duplicate_policy(duplicate)?;
                let unknown_fields = self.json_unknown_field_policy(unknown)?;
                let numbers = self.json_number_policy(numbers)?;
                let limits = self.json_limits_input(limits)?;
                Ok(self.allocate(
                    RuntimeHostValueKind::JsonValue,
                    HostValue::JsonDecodeOptions(JsonDecodeOptionsInput {
                        limits,
                        duplicate_keys,
                        unknown_fields,
                        numbers,
                    }),
                ))
            }
            (
                "intrinsic.json.JsonEncodeOptions.construct",
                [limits, RuntimeValue::Bool(canonical)],
            ) => {
                let limits = self.json_limits_input(limits)?;
                Ok(self.allocate(
                    RuntimeHostValueKind::JsonValue,
                    HostValue::JsonEncodeOptions(JsonEncodeOptionsInput {
                        limits,
                        canonical: *canonical,
                    }),
                ))
            }
            ("std.messagepack.validate", [bytes]) => {
                match messagepack::validate(self.bytes(bytes)?, Default::default()) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.validate", [bytes, options]) => {
                match messagepack::validate(
                    self.bytes(bytes)?,
                    self.messagepack_decode_options(options)?,
                ) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.parse", [bytes, options]) => {
                match messagepack::parse(
                    self.bytes(bytes)?,
                    self.messagepack_decode_options(options)?,
                ) {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackValue,
                        HostValue::MessagePackValue(value),
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.parseView", [bytes, options]) => {
                let input = self.bytes(bytes)?.to_vec();
                match messagepack::parse_view(&input, self.messagepack_decode_options(options)?) {
                    Ok(_) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackValueView,
                        HostValue::MessagePackValueView(input),
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.decode", [bytes, options]) => {
                let expected = generic_argument(specialized_name).unwrap_or("Unit");
                match messagepack::decode_value(
                    self.bytes(bytes)?,
                    self.messagepack_decode_options(options)?,
                ) {
                    Ok(value) => match self.messagepack_to_typed_runtime(&value, expected) {
                        Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(value))),
                        Err(kind) => Ok(self.messagepack_result_kind(kind)),
                    },
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.encode", [value, options]) => {
                let value = match self.runtime_messagepack_value(value) {
                    Ok(value) => value,
                    Err(kind) => return Ok(self.messagepack_result_kind(kind)),
                };
                match messagepack::encode_value(&value, self.messagepack_encode_options(options)?) {
                    Ok(output) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output)),
                    ))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.encodeDeterministic", [value, limits]) => {
                let value = match self.runtime_messagepack_value(value) {
                    Ok(value) => value,
                    Err(kind) => return Ok(self.messagepack_result_kind(kind)),
                };
                match messagepack::encode_deterministic_with_limits(
                    &value,
                    self.messagepack_limits(limits)?,
                ) {
                    Ok(output) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output)),
                    ))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.raw", [bytes, options]) => {
                match messagepack::raw(
                    self.bytes(bytes)?,
                    self.messagepack_decode_options(options)?,
                ) {
                    Ok(raw) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackRaw,
                        HostValue::MessagePackRaw(raw.as_bytes().to_vec()),
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.rawUnchecked", [bytes]) => Ok(self.allocate(
                RuntimeHostValueKind::MessagePackRaw,
                HostValue::MessagePackRaw(self.bytes(bytes)?.to_vec()),
            )),
            ("std.messagepack.MessagePackTimestamp.fromExt", [value]) => {
                let RuntimeValue::Record { values, .. } = value else {
                    return Ok(self
                        .messagepack_result_kind(messagepack::MessagePackErrorKind::TypeMismatch));
                };
                let type_code = match values.first() {
                    Some(RuntimeValue::Integer(value)) => i8::try_from(*value)
                        .map_err(|_| VmError::Host("timestamp type code out of range".into()))?,
                    _ => {
                        return Ok(self.messagepack_result_kind(
                            messagepack::MessagePackErrorKind::TypeMismatch,
                        ));
                    }
                };
                let payload = self
                    .bytes(
                        values
                            .get(1)
                            .ok_or_else(|| VmError::Host("timestamp payload missing".into()))?,
                    )?
                    .to_vec();
                match messagepack::MessagePackTimestamp::from_ext(&messagepack::MessagePackExt {
                    type_code,
                    payload,
                }) {
                    Ok(timestamp) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackTimestamp,
                        HostValue::MessagePackTimestamp(timestamp),
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.MessagePackTimestamp.toExt", [value]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackTimestamp,
                    id,
                } = value
                else {
                    return Ok(self
                        .messagepack_result_kind(messagepack::MessagePackErrorKind::TypeMismatch));
                };
                let Some(HostValue::MessagePackTimestamp(timestamp)) = self.values.get(id) else {
                    return Err(VmError::Host("MessagePackTimestamp token is stale".into()));
                };
                match timestamp.to_ext() {
                    Ok(ext) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Record {
                        name: "MessagePackExt".into(),
                        values: vec![
                            RuntimeValue::Integer(i128::from(ext.type_code)),
                            self.allocate(
                                RuntimeHostValueKind::Bytes,
                                HostValue::Bytes(ext.payload),
                            ),
                        ],
                    }))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.MessagePackReader.fromBytes", [bytes, options]) => {
                match messagepack::MessagePackReader::from_chunks(
                    [self.bytes(bytes)?.to_vec()],
                    self.messagepack_decode_options(options)?,
                ) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackReader,
                        HostValue::MessagePackReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.MessagePackReader.fromReader", [reader, options]) => {
                let (_, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(
                        self.messagepack_result_kind(messagepack::MessagePackErrorKind::IoError)
                    );
                }
                let input = self.stdin[offset..].to_vec();
                let options = self.messagepack_decode_options(options)?;
                match messagepack::MessagePackReader::from_chunks([input], options) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::MessagePackReader,
                        HostValue::MessagePackReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.MessagePackReader.next", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host(
                        "MessagePackReader receiver is invalid".into(),
                    ));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::MessagePackReader { reader, finished }) if !*finished => {
                        let result = reader.next();
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    Some(HostValue::MessagePackReader { .. }) => {
                        return Ok(self.messagepack_result_kind(
                            messagepack::MessagePackErrorKind::InvalidTag,
                        ));
                    }
                    _ => return Err(VmError::Host("MessagePackReader token is stale".into())),
                };
                match result {
                    Ok(Some(event)) => Ok(RuntimeValue::ResultOk(Box::new(
                        RuntimeValue::OptionSome(Box::new(self.runtime_messagepack_event(event))),
                    ))),
                    Ok(None) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.messagepack.MessagePackReader.own", [reader, event]) => {
                let event = self.messagepack_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host(
                        "MessagePackReader receiver is invalid".into(),
                    ));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::MessagePackReader { reader, finished }) => {
                        match reader.own(event) {
                            Ok(event) => Ok(RuntimeValue::ResultOk(Box::new(
                                self.runtime_messagepack_event(event),
                            ))),
                            Err(error) => {
                                *finished = true;
                                Ok(self.messagepack_result_error(&error))
                            }
                        }
                    }
                    _ => Err(VmError::Host("MessagePackReader token is stale".into())),
                }
            }
            ("std.messagepack.MessagePackReader.finish", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host(
                        "MessagePackReader receiver is invalid".into(),
                    ));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::MessagePackReader { reader, finished }) => {
                        let result = reader.finish();
                        *finished = true;
                        match result {
                            Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                            Err(error) => Ok(self.messagepack_result_error(&error)),
                        }
                    }
                    _ => Err(VmError::Host("MessagePackReader token is stale".into())),
                }
            }
            ("std.messagepack.MessagePackWriter.toWriter", [writer, options]) => {
                let stream = self.writer_stream(writer)?;
                if stream == StreamKind::Stdin {
                    return Ok(
                        self.messagepack_result_kind(messagepack::MessagePackErrorKind::IoError)
                    );
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::MessagePackWriter,
                    HostValue::MessagePackWriter {
                        writer: messagepack::MessagePackWriter::new(
                            self.messagepack_encode_options(options)?,
                        ),
                        stream: Some(stream),
                        finished: false,
                    },
                ))))
            }
            ("std.messagepack.MessagePackWriter.write", [writer, event]) => {
                let event = self.messagepack_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host(
                        "MessagePackWriter receiver is invalid".into(),
                    ));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::MessagePackWriter {
                        writer, finished, ..
                    }) => {
                        let result = writer.write(event);
                        if result.is_err() {
                            *finished = true;
                        }
                        match result {
                            Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                            Err(error) => Ok(self.messagepack_result_error(&error)),
                        }
                    }
                    _ => Err(VmError::Host("MessagePackWriter token is stale".into())),
                }
            }
            ("std.messagepack.MessagePackWriter.finish", [writer]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::MessagePackWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host(
                        "MessagePackWriter receiver is invalid".into(),
                    ));
                };
                let (stream, result) = match self.values.get_mut(id) {
                    Some(HostValue::MessagePackWriter {
                        writer,
                        stream,
                        finished,
                    }) => {
                        let result = writer.finish();
                        *finished = true;
                        (*stream, result)
                    }
                    _ => return Err(VmError::Host("MessagePackWriter token is stale".into())),
                };
                match result {
                    Ok(bytes) => {
                        if let Some(stream) = stream {
                            match stream {
                                StreamKind::Stdout => self.stdout.extend_from_slice(&bytes),
                                StreamKind::Stderr => self.stderr.extend_from_slice(&bytes),
                                StreamKind::Stdin => {}
                            }
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
                    }
                    Err(error) => Ok(self.messagepack_result_error(&error)),
                }
            }
            ("std.protobuf.validate", [bytes]) => {
                match protobuf::validate::<()>(self.bytes(bytes)?, Default::default()) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.validate", [bytes, options]) => {
                match protobuf::validate::<()>(
                    self.bytes(bytes)?,
                    self.proto_decode_options(options)?,
                ) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.decode", [bytes, options]) => {
                let expected = generic_argument(specialized_name).unwrap_or("Unit");
                if expected != "Unit" {
                    return Ok(self.protobuf_result_kind(protobuf::ProtoErrorKind::TypeMismatch));
                }
                match protobuf::decode::<Vec<u64>>(
                    self.bytes(bytes)?,
                    self.proto_decode_options(options)?,
                ) {
                    Ok(_) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.encode", [value, options]) => {
                if !matches!(value, RuntimeValue::Unit) {
                    return Ok(self.protobuf_result_kind(protobuf::ProtoErrorKind::TypeMismatch));
                }
                match protobuf::encode::<u64>(&0, self.proto_encode_options(options)?) {
                    Ok(output) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output)),
                    ))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.encodeDeterministic", [value, limits]) => {
                if !matches!(value, RuntimeValue::Unit) {
                    return Ok(self.protobuf_result_kind(protobuf::ProtoErrorKind::TypeMismatch));
                }
                match protobuf::encode_deterministic::<u64>(&0, self.proto_limits(limits)?) {
                    Ok(output) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output)),
                    ))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.descriptor", []) => Ok(self.allocate(
                RuntimeHostValueKind::ProtoDescriptor,
                HostValue::ProtoDescriptor(
                    generic_argument(specialized_name)
                        .unwrap_or("Unit")
                        .to_owned(),
                ),
            )),
            ("std.protobuf.ProtoReader.fromBytes", [bytes, options]) => {
                match protobuf::ProtoReader::<()>::from_chunks(
                    [self.bytes(bytes)?.to_vec()],
                    self.proto_decode_options(options)?,
                ) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::ProtoReader,
                        HostValue::ProtoReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.ProtoReader.fromReader", [reader, options]) => {
                let (_, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(self.protobuf_result_kind(protobuf::ProtoErrorKind::IoError));
                }
                match protobuf::ProtoReader::<()>::from_chunks(
                    [self.stdin[offset..].to_vec()],
                    self.proto_decode_options(options)?,
                ) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::ProtoReader,
                        HostValue::ProtoReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.ProtoReader.next", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ProtoReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("ProtoReader receiver is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::ProtoReader { reader, finished }) if !*finished => {
                        let result = reader.next();
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    Some(HostValue::ProtoReader { .. }) => {
                        return Ok(
                            self.protobuf_result_kind(protobuf::ProtoErrorKind::TrailingData)
                        );
                    }
                    _ => return Err(VmError::Host("ProtoReader token is stale".into())),
                };
                match result {
                    Ok(Some(event)) => Ok(RuntimeValue::ResultOk(Box::new(
                        RuntimeValue::OptionSome(Box::new(self.runtime_proto_event(event))),
                    ))),
                    Ok(None) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.ProtoReader.own", [reader, event]) => {
                let event = self.proto_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ProtoReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("ProtoReader receiver is invalid".into()));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::ProtoReader { reader, finished }) => match reader.own(event) {
                        Ok(event) => Ok(RuntimeValue::ResultOk(Box::new(
                            self.runtime_proto_event(event),
                        ))),
                        Err(error) => {
                            *finished = true;
                            Ok(self.protobuf_result_error(&error))
                        }
                    },
                    _ => Err(VmError::Host("ProtoReader token is stale".into())),
                }
            }
            ("std.protobuf.ProtoReader.finish", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ProtoReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("ProtoReader receiver is invalid".into()));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::ProtoReader { reader, finished }) => {
                        let result = reader.finish();
                        *finished = true;
                        match result {
                            Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                            Err(error) => Ok(self.protobuf_result_error(&error)),
                        }
                    }
                    _ => Err(VmError::Host("ProtoReader token is stale".into())),
                }
            }
            ("std.protobuf.ProtoWriter.toWriter", [writer, options]) => {
                let stream = self.writer_stream(writer)?;
                if stream == StreamKind::Stdin {
                    return Ok(self.protobuf_result_kind(protobuf::ProtoErrorKind::IoError));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::ProtoWriter,
                    HostValue::ProtoWriter {
                        writer: protobuf::ProtoWriter::new(self.proto_encode_options(options)?),
                        stream: Some(stream),
                        finished: false,
                    },
                ))))
            }
            ("std.protobuf.ProtoWriter.write", [writer, event]) => {
                let event = self.proto_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ProtoWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host("ProtoWriter receiver is invalid".into()));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::ProtoWriter {
                        writer, finished, ..
                    }) => {
                        let result = writer.write(event);
                        if result.is_err() {
                            *finished = true;
                        }
                        match result {
                            Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                            Err(error) => Ok(self.protobuf_result_error(&error)),
                        }
                    }
                    _ => Err(VmError::Host("ProtoWriter token is stale".into())),
                }
            }
            ("std.protobuf.ProtoWriter.finish", [writer]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ProtoWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host("ProtoWriter receiver is invalid".into()));
                };
                let (stream, result) = match self.values.get_mut(id) {
                    Some(HostValue::ProtoWriter {
                        writer,
                        stream,
                        finished,
                    }) => {
                        let result = writer.finish();
                        *finished = true;
                        (*stream, result)
                    }
                    _ => return Err(VmError::Host("ProtoWriter token is stale".into())),
                };
                match result {
                    Ok(bytes) => {
                        if let Some(stream) = stream {
                            match stream {
                                StreamKind::Stdout => self.stdout.extend_from_slice(&bytes),
                                StreamKind::Stderr => self.stderr.extend_from_slice(&bytes),
                                StreamKind::Stdin => {}
                            }
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
                    }
                    Err(error) => Ok(self.protobuf_result_error(&error)),
                }
            }
            ("std.protobuf.UnknownFields.count", [fields]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::UnknownFields,
                    id,
                } = fields
                else {
                    return Err(VmError::Host("UnknownFields receiver is invalid".into()));
                };
                match self.values.get(id) {
                    Some(HostValue::UnknownFields(fields)) => {
                        Ok(RuntimeValue::Integer(fields.count() as i128))
                    }
                    _ => Err(VmError::Host("UnknownFields token is stale".into())),
                }
            }
            ("std.protobuf.UnknownFields.discard", [fields]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::UnknownFields,
                    id,
                } = fields
                else {
                    return Err(VmError::Host("UnknownFields receiver is invalid".into()));
                };
                match self.values.get_mut(id) {
                    Some(HostValue::UnknownFields(fields)) => {
                        fields.discard();
                        Ok(RuntimeValue::Unit)
                    }
                    _ => Err(VmError::Host("UnknownFields token is stale".into())),
                }
            }
            ("std.json.validate", [bytes, options]) => {
                let input = self.bytes(bytes)?.to_vec();
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::validate_with_options(&input, options) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.canonicalize", [bytes, options]) => {
                let input = self.bytes(bytes)?.to_vec();
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::canonicalize_with_options(&input, options) {
                    Ok(output) => {
                        if self.ensure_bytes_len(output.len()).is_err() {
                            return Ok(self.json_result_error(json::JsonErrorKind::LimitExceeded));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(output),
                        ))))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.parse", [bytes, options]) => {
                let input = self.bytes(bytes)?;
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::parse_with_options(input, options) {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonValue,
                        HostValue::JsonValue(value),
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.parseView", [bytes, options]) => {
                let id = self.bytes_id(bytes)?;
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::parse_view(self.bytes(bytes)?, options) {
                    Ok(_) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonValueView,
                        HostValue::JsonValueView { _bytes: id },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.decode", [bytes, options]) => {
                let expected = generic_argument(specialized_name).ok_or_else(|| {
                    VmError::Host("specialized JSON decode has no type argument".into())
                })?;
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::parse_with_options(self.bytes(bytes)?, options) {
                    Ok(value) => match json_to_runtime(&value, expected) {
                        Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(value))),
                        Err(kind) => Ok(self.json_result_error(kind)),
                    },
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.encode", [value, options]) => {
                let options = match self.json_encode_options(options) {
                    Ok(options) => options,
                    Err(kind) => return Ok(self.json_result_error(kind)),
                };
                let value = match self.runtime_json_value(value) {
                    Ok(value) => value,
                    Err(kind) => return Ok(self.json_result_error(kind)),
                };
                match json::encode_with_options(&value, options) {
                    Ok(output) => {
                        if self.ensure_bytes_len(output.len()).is_err() {
                            return Ok(self.json_result_error(json::JsonErrorKind::LimitExceeded));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(output),
                        ))))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.encodeCanonical", [value, limits]) => {
                let value = self.json_value(value)?.clone();
                let limits = self.json_limits_input(limits)?;
                let limits = match self.json_limits(limits) {
                    Ok(limits) => limits,
                    Err(kind) => return Ok(self.json_result_error(kind)),
                };
                match json::encode_canonical_with_limits(&value, limits) {
                    Ok(output) => {
                        if self.ensure_bytes_len(output.len()).is_err() {
                            return Ok(self.json_result_error(json::JsonErrorKind::LimitExceeded));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(output),
                        ))))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.raw", [bytes]) => {
                let id = self.bytes_id(bytes)?;
                match json::raw(self.bytes(bytes)?, Default::default()) {
                    Ok(_) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonRaw,
                        HostValue::JsonRaw { _bytes: id },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.rawUnchecked", [bytes]) => {
                let id = self.bytes_id(bytes)?;
                let _raw = json::raw_unchecked(self.bytes(bytes)?);
                Ok(self.allocate(
                    RuntimeHostValueKind::JsonRaw,
                    HostValue::JsonRaw { _bytes: id },
                ))
            }
            ("std.json.JsonNumber.parse", [RuntimeValue::String(text)]) => {
                match json::JsonNumber::parse(text) {
                    Ok(number) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonNumber,
                        HostValue::JsonNumber(number),
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonNumber.text", [number]) => {
                Ok(RuntimeValue::String(self.json_number(number)?.text()))
            }
            ("std.json.JsonNumber.toInt", [number]) => match self.json_number(number)?.to_int() {
                Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                    i128::from(value),
                )))),
                Err(error) => Ok(self.json_result_structured_error(&error)),
            },
            ("std.json.JsonNumber.toUInt", [number]) => match self.json_number(number)?.to_uint() {
                Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                    i128::from(value),
                )))),
                Err(error) => Ok(self.json_result_structured_error(&error)),
            },
            ("std.json.JsonNumber.toFloat32", [number]) => {
                match self.json_number(number)?.to_float32() {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Float(
                        f64::from(value),
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonNumber.toFloat64", [number]) => {
                match self.json_number(number)?.to_float64() {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Float(value)))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonReader.fromBytes", [bytes, options]) => {
                let input = self.bytes(bytes)?.to_vec();
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::JsonReader::from_reader(std::io::Cursor::new(input), options) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonReader,
                        HostValue::JsonReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonReader.fromReader", [reader, options]) => {
                let (id, stream, offset) = self.reader_state(reader)?;
                if stream != StreamKind::Stdin {
                    return Ok(self.json_result_error(json::JsonErrorKind::IoError));
                }
                let input = self.stdin[offset..].to_vec();
                if let Some(HostValue::Reader { offset, .. }) = self.values.get_mut(&id) {
                    *offset = self.stdin.len();
                }
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::JsonReader::from_reader(std::io::Cursor::new(input), options) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonReader,
                        HostValue::JsonReader {
                            reader,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonReader.next", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JsonReader receiver is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonReader { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonReader { reader, finished }) => {
                        let result = reader.next();
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JsonReader token is stale".into())),
                };
                match result {
                    Ok(Some(event)) => {
                        let event = self.runtime_json_event(event);
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                            Box::new(event),
                        ))))
                    }
                    Ok(None) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonReader.fromBytesSerialization", [bytes, options]) => {
                let input = self.bytes(bytes)?.to_vec();
                let options = match self.json_decode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::JsonReader::from_reader(std::io::Cursor::new(input), options) {
                    Ok(reader) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonReader,
                        HostValue::JsonSerializationReader {
                            reader,
                            depth: 0,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonReader.nextSerialization", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JSON serialization reader is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonSerializationReader { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonSerializationReader {
                        reader,
                        depth,
                        finished,
                    }) => {
                        let event = reader.next();
                        match event {
                            Ok(Some(event)) => {
                                let root_complete = match event {
                                    json::JsonEvent::StartArray(_)
                                    | json::JsonEvent::StartObject(_) => {
                                        *depth = depth.saturating_add(1);
                                        false
                                    }
                                    json::JsonEvent::EndArray | json::JsonEvent::EndObject => {
                                        *depth = depth.saturating_sub(1);
                                        *depth == 0
                                    }
                                    json::JsonEvent::Key(_) => false,
                                    json::JsonEvent::Null
                                    | json::JsonEvent::Bool(_)
                                    | json::JsonEvent::Number(_)
                                    | json::JsonEvent::String(_) => *depth == 0,
                                };
                                if root_complete {
                                    match reader.next() {
                                        Ok(None) => *finished = true,
                                        Ok(Some(_)) => {
                                            *finished = true;
                                            return Ok(self.json_result_error(
                                                json::JsonErrorKind::TrailingData,
                                            ));
                                        }
                                        Err(error) => {
                                            *finished = true;
                                            return Ok(self.json_result_structured_error(&error));
                                        }
                                    }
                                }
                                Ok(Some(event))
                            }
                            Ok(None) => {
                                *finished = true;
                                Ok(None)
                            }
                            Err(error) => {
                                *finished = true;
                                Err(error)
                            }
                        }
                    }
                    _ => return Err(VmError::Host("JSON serialization reader is stale".into())),
                };
                match result {
                    Ok(Some(event)) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.runtime_serialization_event(event),
                    ))),
                    Ok(None) => Ok(self.json_result_error(json::JsonErrorKind::UnexpectedEof)),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonReader.nextSerializationBase64", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JSON serialization reader is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonSerializationReader { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonSerializationReader {
                        reader, finished, ..
                    }) => {
                        let result = reader.next();
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JSON serialization reader is stale".into())),
                };
                match result {
                    Ok(Some(json::JsonEvent::String(value))) => {
                        match stdlib_serialization::base64_decode(&value) {
                            Ok(bytes) => {
                                if self.ensure_bytes_len(bytes.len()).is_err() {
                                    return Ok(
                                        self.json_result_error(json::JsonErrorKind::LimitExceeded)
                                    );
                                }
                                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                                    RuntimeHostValueKind::Bytes,
                                    HostValue::Bytes(bytes),
                                ))))
                            }
                            Err(stdlib_serialization::Base64DecodeError) => {
                                if let Some(HostValue::JsonSerializationReader {
                                    finished, ..
                                }) = self.values.get_mut(id)
                                {
                                    *finished = true;
                                }
                                Ok(self.json_result_error(json::JsonErrorKind::TypeMismatch))
                            }
                        }
                    }
                    Ok(Some(_)) => {
                        if let Some(HostValue::JsonSerializationReader { finished, .. }) =
                            self.values.get_mut(id)
                        {
                            *finished = true;
                        }
                        Ok(self.json_result_error(json::JsonErrorKind::TypeMismatch))
                    }
                    Ok(None) => {
                        if let Some(HostValue::JsonSerializationReader { finished, .. }) =
                            self.values.get_mut(id)
                        {
                            *finished = true;
                        }
                        Ok(self.json_result_error(json::JsonErrorKind::UnexpectedEof))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonReader.peekSerialization", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JSON serialization reader is invalid".into()));
                };
                let result = match self.values.get(id) {
                    Some(HostValue::JsonSerializationReader { finished: true, .. }) => None,
                    Some(HostValue::JsonSerializationReader { reader, .. }) => {
                        let mut lookahead = reader.clone();
                        match lookahead.next() {
                            Ok(event) => event.map(Ok),
                            Err(error) => Some(Err(error)),
                        }
                    }
                    _ => return Err(VmError::Host("JSON serialization reader is stale".into())),
                };
                match result {
                    Some(Ok(event)) => {
                        let event = self.runtime_serialization_event(event);
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                            Box::new(event),
                        ))))
                    }
                    None => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                    Some(Err(error)) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonReader.rejectSerialization", [reader, error]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JSON serialization reader is invalid".into()));
                };
                if !matches!(
                    self.values.get(id),
                    Some(HostValue::JsonSerializationReader { .. })
                ) {
                    return Err(VmError::Host("JSON serialization reader is stale".into()));
                }
                let kind = match error {
                    RuntimeValue::Variant { name, variant, .. } if name == "SerializationError" => {
                        match variant {
                            2 => json::JsonErrorKind::MissingField,
                            3 => json::JsonErrorKind::DuplicateKey,
                            4 => json::JsonErrorKind::UnknownField,
                            5 | 6 => json::JsonErrorKind::LimitExceeded,
                            8 => json::JsonErrorKind::IoError,
                            _ => json::JsonErrorKind::TypeMismatch,
                        }
                    }
                    _ => json::JsonErrorKind::TypeMismatch,
                };
                Ok(self.json_error_parts(
                    kind,
                    json::JsonLocation {
                        offset: 0,
                        line: 1,
                        column: 1,
                    },
                    &json::JsonPath::root(),
                ))
            }
            ("std.json.JsonReader.own", [reader, event]) => {
                let event = self.json_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JsonReader receiver is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonReader { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonReader { reader, finished }) => {
                        let result = reader.own(event);
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JsonReader token is stale".into())),
                };
                match result {
                    Ok(event) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.runtime_json_event(event),
                    ))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonReader.finish", [reader]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonReader,
                    id,
                } = reader
                else {
                    return Err(VmError::Host("JsonReader receiver is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonReader { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonReader { reader, finished }) => {
                        let result = reader.finish();
                        *finished = true;
                        result
                    }
                    _ => return Err(VmError::Host("JsonReader token is stale".into())),
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonWriter.toWriter", [writer, options]) => {
                let stream = self.writer_stream(writer)?;
                if stream == StreamKind::Stdin {
                    return Ok(self.json_result_error(json::JsonErrorKind::IoError));
                }
                let options = match self.json_encode_options(options) {
                    Ok(options) => options,
                    Err(error) => return Ok(self.json_result_error(error)),
                };
                match json::JsonWriter::to_writer(options) {
                    Ok(writer) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::JsonWriter,
                        HostValue::JsonWriter {
                            writer,
                            stream,
                            finished: false,
                        },
                    )))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonWriter.write", [writer, event]) => {
                let event = self.json_event(event)?;
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host("JsonWriter receiver is invalid".into()));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonWriter { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonWriter {
                        writer, finished, ..
                    }) => {
                        let result = writer.write(event);
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JsonWriter token is stale".into())),
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.json.JsonWriter.finish", [writer]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host("JsonWriter receiver is invalid".into()));
                };
                let (stream, result) = match self.values.get_mut(id) {
                    Some(HostValue::JsonWriter { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonWriter {
                        writer,
                        stream,
                        finished,
                    }) => {
                        let result = writer.finish();
                        *finished = true;
                        (*stream, result)
                    }
                    _ => return Err(VmError::Host("JsonWriter token is stale".into())),
                };
                match result {
                    Ok(bytes) => {
                        if self.ensure_bytes_len(bytes.len()).is_err() {
                            return Ok(self.json_result_error(json::JsonErrorKind::LimitExceeded));
                        }
                        match stream {
                            StreamKind::Stdout => self.stdout.extend_from_slice(&bytes),
                            StreamKind::Stderr => self.stderr.extend_from_slice(&bytes),
                            StreamKind::Stdin => {
                                unreachable!("stdin rejected at construction")
                            }
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonWriter.buffer", [options]) => {
                let (writer, error) = match self.json_encode_options(options) {
                    Ok(options) => match json::JsonWriter::to_writer(options) {
                        Ok(writer) => (Some(writer), None),
                        Err(error) => (None, Some(error.kind)),
                    },
                    Err(error) => (None, Some(error)),
                };
                Ok(self.allocate(
                    RuntimeHostValueKind::JsonWriter,
                    HostValue::JsonBufferWriter {
                        writer,
                        error,
                        finished: false,
                    },
                ))
            }
            ("intrinsic.json.JsonWriter.write", [writer, event]) => {
                let event = match Self::serialization_event_to_json(event) {
                    Ok(event) => event,
                    Err(kind) => return Ok(self.json_result_error(kind)),
                };
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host(
                        "JSON buffer writer receiver is invalid".into(),
                    ));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonBufferWriter { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonBufferWriter {
                        writer,
                        error,
                        finished,
                    }) => {
                        if let Some(kind) = error.take() {
                            *finished = true;
                            return Ok(self.json_result_error(kind));
                        }
                        let result = writer
                            .as_mut()
                            .expect("a valid buffer writer remains installed")
                            .write(event);
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JSON buffer writer token is stale".into())),
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonWriter.writeBase64", [writer, bytes]) => {
                let encoded = stdlib_serialization::base64_encode(self.bytes(bytes)?);
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host(
                        "JSON buffer writer receiver is invalid".into(),
                    ));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonBufferWriter { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonBufferWriter {
                        writer,
                        error,
                        finished,
                    }) => {
                        if let Some(kind) = error.take() {
                            *finished = true;
                            return Ok(self.json_result_error(kind));
                        }
                        let result = writer
                            .as_mut()
                            .expect("a valid buffer writer remains installed")
                            .write(json::JsonEvent::String(encoded));
                        if result.is_err() {
                            *finished = true;
                        }
                        result
                    }
                    _ => return Err(VmError::Host("JSON buffer writer token is stale".into())),
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("intrinsic.json.JsonWriter.finish", [writer]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonWriter,
                    id,
                } = writer
                else {
                    return Err(VmError::Host(
                        "JSON buffer writer receiver is invalid".into(),
                    ));
                };
                let result = match self.values.get_mut(id) {
                    Some(HostValue::JsonBufferWriter { finished: true, .. }) => {
                        return Ok(self.json_result_error(json::JsonErrorKind::InvalidSyntax));
                    }
                    Some(HostValue::JsonBufferWriter {
                        writer,
                        error,
                        finished,
                    }) => {
                        if let Some(kind) = error.take() {
                            *finished = true;
                            return Ok(self.json_result_error(kind));
                        }
                        let result = writer
                            .as_mut()
                            .expect("a valid buffer writer remains installed")
                            .finish();
                        *finished = true;
                        result
                    }
                    _ => return Err(VmError::Host("JSON buffer writer token is stale".into())),
                };
                match result {
                    Ok(bytes) => {
                        if self.ensure_bytes_len(bytes.len()).is_err() {
                            return Ok(self.json_result_error(json::JsonErrorKind::LimitExceeded));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(bytes),
                        ))))
                    }
                    Err(error) => Ok(self.json_result_structured_error(&error)),
                }
            }
            ("std.messagepack.canonicalize", [bytes]) => {
                let input = self.bytes(bytes)?.to_vec();
                match messagepack::decode(&input) {
                    Ok(value) => match messagepack::encode_deterministic(&value) {
                        Ok(output) => {
                            if let Err(message) = self.ensure_bytes_len(output.len()) {
                                return Ok(self.bytes_result_error(message));
                            }
                            Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                                RuntimeHostValueKind::Bytes,
                                HostValue::Bytes(output),
                            ))))
                        }
                        Err(error) => Ok(self.bytes_result_error(error.to_string())),
                    },
                    Err(error) => Ok(self.bytes_result_error(error.to_string())),
                }
            }
            ("std.path.Path.fromString", [RuntimeValue::String(value)]) => {
                match path::Path::from_string(value) {
                    Ok(path) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Path, HostValue::Path(path)),
                    ))),
                    Err(error) => Ok(self.path_result_error(format!("{error:?}"))),
                }
            }
            ("std.path.Path.fromBytes", [bytes]) => {
                let input = self.bytes(bytes)?.to_vec();
                match path::Path::from_bytes(&input) {
                    Ok(path) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Path, HostValue::Path(path)),
                    ))),
                    Err(error) => Ok(self.path_result_error(format!("{error:?}"))),
                }
            }
            ("std.path.Path.join", [receiver, RuntimeValue::String(component)]) => {
                let receiver = self.path(receiver)?.clone();
                match receiver.join(component) {
                    Ok(path) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate(RuntimeHostValueKind::Path, HostValue::Path(path)),
                    ))),
                    Err(error) => Ok(self.path_result_error(format!("{error:?}"))),
                }
            }
            ("std.path.Path.parent", [receiver]) => {
                let parent = self.path(receiver)?.parent();
                Ok(parent
                    .map(|path| {
                        RuntimeValue::OptionSome(Box::new(
                            self.allocate(RuntimeHostValueKind::Path, HostValue::Path(path)),
                        ))
                    })
                    .unwrap_or(RuntimeValue::OptionNone))
            }
            ("std.path.Path.fileName", [receiver]) => Ok(self
                .path(receiver)?
                .file_name()
                .map(|value| RuntimeValue::OptionSome(Box::new(RuntimeValue::String(value.into()))))
                .unwrap_or(RuntimeValue::OptionNone)),
            ("std.path.Path.extension", [receiver]) => Ok(self
                .path(receiver)?
                .extension()
                .map(|value| RuntimeValue::OptionSome(Box::new(RuntimeValue::String(value.into()))))
                .unwrap_or(RuntimeValue::OptionNone)),
            ("std.path.Path.kind", [receiver]) => {
                Ok(RuntimeValue::Bool(self.path(receiver)?.is_absolute()))
            }
            ("std.path.Path.isEmpty", [receiver]) => {
                Ok(RuntimeValue::Bool(self.path(receiver)?.is_empty()))
            }
            ("std.path.Path.toString", [receiver]) => match self.path(receiver)?.to_string() {
                Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(
                    value,
                )))),
                Err(error) => Ok(self.path_result_error(format!("{error:?}"))),
            },
            ("std.path.Path.toBytes", [receiver]) => {
                let bytes = self.path(receiver)?.to_bytes();
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(bytes)))
            }
            ("std.fs.OpenMode.Read", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::Read),
            )),
            ("std.fs.OpenMode.Write", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::Write),
            )),
            ("std.fs.OpenMode.ReadWrite", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::ReadWrite),
            )),
            ("std.fs.OpenMode.Append", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::Append),
            )),
            ("std.fs.OpenMode.Create", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::Create),
            )),
            ("std.fs.OpenMode.CreateNew", []) => Ok(self.allocate(
                RuntimeHostValueKind::OpenMode,
                HostValue::OpenMode(FsOpenMode::CreateNew),
            )),
            ("std.fs.open", [receiver, mode]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let mode = self.fs_open_mode(mode)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => {
                        return Ok(self.fs_result_error("path is a directory"));
                    }
                    Ok(_) => {}
                    Err(error) if !matches!(mode, FsOpenMode::Create | FsOpenMode::CreateNew) => {
                        return Ok(self.fs_result_error(error.to_string()));
                    }
                    Err(_) => {}
                }
                let mut options = OpenOptions::new();
                match mode {
                    FsOpenMode::Read => {
                        options.read(true);
                    }
                    FsOpenMode::Write => {
                        options.write(true);
                    }
                    FsOpenMode::ReadWrite => {
                        options.read(true).write(true);
                    }
                    FsOpenMode::Append => {
                        options.append(true);
                    }
                    FsOpenMode::Create => {
                        options.write(true).create(true).truncate(true);
                    }
                    FsOpenMode::CreateNew => {
                        options.write(true).create_new(true);
                    }
                }
                match options.open(path) {
                    Ok(file) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::File,
                        HostValue::File {
                            file,
                            readable: matches!(mode, FsOpenMode::Read | FsOpenMode::ReadWrite),
                            writable: matches!(
                                mode,
                                FsOpenMode::Write
                                    | FsOpenMode::ReadWrite
                                    | FsOpenMode::Append
                                    | FsOpenMode::Create
                                    | FsOpenMode::CreateNew
                            ),
                        },
                    )))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.openDirectory", [receiver]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => {
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Directory,
                            HostValue::Directory { path },
                        ))))
                    }
                    Ok(_) => Ok(self.fs_result_error("path is not a directory")),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.metadata", [receiver]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::Metadata,
                        HostValue::Metadata {
                            _file: metadata.is_file(),
                            _directory: metadata.is_dir(),
                            _symlink: metadata.file_type().is_symlink(),
                            _len: metadata.len(),
                            _readonly: metadata.permissions().readonly(),
                        },
                    )))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.File.read", [receiver, RuntimeValue::Integer(maximum)]) => {
                let maximum = match usize::try_from(*maximum) {
                    Ok(maximum) if maximum > 0 => maximum,
                    _ => return Ok(self.fs_result_error("read length must be positive")),
                };
                if let Err(message) = self.ensure_bytes_len(maximum) {
                    return Ok(self.fs_result_error(message));
                }
                let id = self.file_id(receiver)?;
                let read = match self.values.get_mut(&id) {
                    Some(HostValue::File { file, readable, .. }) if *readable => {
                        let mut buffer = vec![0; maximum];
                        match file.read(&mut buffer) {
                            Ok(0) => Ok(None),
                            Ok(count) => {
                                buffer.truncate(count);
                                Ok(Some(buffer))
                            }
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Some(HostValue::File { .. }) => Err("file is not readable".to_owned()),
                    _ => Err("File token is stale".to_owned()),
                };
                match read {
                    Ok(None) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                    Ok(Some(bytes)) => {
                        let bytes =
                            self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(bytes));
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                            Box::new(bytes),
                        ))))
                    }
                    Err(error) => Ok(self.fs_result_error(error)),
                }
            }
            ("std.fs.File.write", [receiver, bytes]) => {
                let bytes = self.bytes(bytes)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.fs_result_error(message));
                }
                let id = self.file_id(receiver)?;
                let written = match self.values.get_mut(&id) {
                    Some(HostValue::File { file, writable, .. }) if *writable => {
                        file.write(&bytes).map_err(|error| error.to_string())
                    }
                    Some(HostValue::File { .. }) => Err("file is not writable".to_owned()),
                    _ => Err("File token is stale".to_owned()),
                };
                match written {
                    Ok(count) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                        i128::try_from(count).map_err(|_| {
                            VmError::Host("write length does not fit in Int".into())
                        })?,
                    )))),
                    Err(error) => Ok(self.fs_result_error(error)),
                }
            }
            ("std.fs.File.flush", [receiver]) => {
                let id = self.file_id(receiver)?;
                let flushed = match self.values.get_mut(&id) {
                    Some(HostValue::File { file, writable, .. }) if *writable => {
                        file.flush().map_err(|error| error.to_string())
                    }
                    Some(HostValue::File { .. }) => Err("file is not writable".to_owned()),
                    _ => Err("File token is stale".to_owned()),
                };
                match flushed {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error)),
                }
            }
            ("std.fs.Directory.list", [receiver]) => {
                let (_id, base) = self.directory_path(receiver)?;
                let mut entries = match std::fs::read_dir(&base) {
                    Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
                    Err(error) => Err(error),
                };
                let entries = match entries.as_mut() {
                    Ok(entries) => entries,
                    Err(error) => return Ok(self.fs_result_error(error.to_string())),
                };
                entries.sort_by(|left, right| {
                    native_file_name_bytes(left).cmp(&native_file_name_bytes(right))
                });
                if entries.len() > 1_048_576 {
                    return Ok(self.fs_result_error("directory entry limit exceeded"));
                }
                #[cfg(unix)]
                let base_bytes = {
                    use std::os::unix::ffi::OsStrExt;
                    base.as_os_str().as_bytes().to_vec()
                };
                #[cfg(not(unix))]
                let base_bytes = base.to_string_lossy().as_bytes().to_vec();
                let mut output = Vec::with_capacity(entries.len());
                let mut total_bytes = 0usize;
                for entry in entries.iter() {
                    let name = native_file_name_bytes(entry);
                    let separator =
                        usize::from(!base_bytes.is_empty() && !base_bytes.ends_with(b"/"));
                    let length = base_bytes
                        .len()
                        .checked_add(separator)
                        .and_then(|length| length.checked_add(name.len()))
                        .ok_or_else(|| {
                            VmError::Host("directory path length overflow".to_owned())
                        })?;
                    total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
                        VmError::Host("directory listing length overflow".to_owned())
                    })?;
                    if let Err(message) = self.ensure_bytes_len(total_bytes) {
                        return Ok(self.fs_result_error(message));
                    }
                    let mut child = base_bytes.clone();
                    if separator != 0 {
                        child.push(b'/');
                    }
                    child.extend_from_slice(&name);
                    let child = match path::Path::from_bytes(&child) {
                        Ok(path) => path,
                        Err(error) => return Ok(self.fs_result_error(format!("{error:?}"))),
                    };
                    output.push(self.allocate(RuntimeHostValueKind::Path, HostValue::Path(child)));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(
                    output,
                ))))
            }
            ("std.fs.readAll", [receiver]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                match std::fs::read(path) {
                    Ok(bytes) => {
                        if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                            return Ok(self.fs_result_error(message));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(bytes),
                        ))))
                    }
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.writeAll", [receiver, bytes]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let bytes = self.bytes(bytes)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.fs_result_error(message));
                }
                match std::fs::write(path, bytes) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.createDirectory", [receiver, RuntimeValue::Bool(parents)]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let result = if *parents {
                    std::fs::create_dir_all(path)
                } else {
                    std::fs::create_dir(path)
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.remove", [receiver]) => {
                let path = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let result = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
                    Ok(_) => std::fs::remove_file(path),
                    Err(error) => Err(error),
                };
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.list", [receiver]) => {
                let base = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let mut entries = match std::fs::read_dir(&base) {
                    Ok(entries) => entries.collect::<Result<Vec<_>, _>>(),
                    Err(error) => Err(error),
                };
                let entries = match entries.as_mut() {
                    Ok(entries) => entries,
                    Err(error) => return Ok(self.fs_result_error(error.to_string())),
                };
                entries.sort_by(|left, right| {
                    native_file_name_bytes(left).cmp(&native_file_name_bytes(right))
                });
                if entries.len() > 1_048_576 {
                    return Ok(self.fs_result_error("directory entry limit exceeded"));
                }
                let base_bytes = self.path_bytes(receiver)?;
                let mut output = Vec::with_capacity(entries.len());
                let mut total_bytes = 0usize;
                for entry in entries.iter() {
                    let name = native_file_name_bytes(entry);
                    let separator =
                        usize::from(!base_bytes.is_empty() && !base_bytes.ends_with(b"/"));
                    let length = base_bytes
                        .len()
                        .checked_add(separator)
                        .and_then(|length| length.checked_add(name.len()))
                        .ok_or_else(|| {
                            VmError::Host("directory path length overflow".to_owned())
                        })?;
                    total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
                        VmError::Host("directory listing length overflow".to_owned())
                    })?;
                    if let Err(message) = self.ensure_bytes_len(total_bytes) {
                        return Ok(self.fs_result_error(message));
                    }
                    let mut child = base_bytes.clone();
                    if separator != 0 {
                        child.push(b'/');
                    }
                    child.extend_from_slice(&name);
                    let child = match path::Path::from_bytes(&child) {
                        Ok(path) => path,
                        Err(error) => return Ok(self.fs_result_error(format!("{error:?}"))),
                    };
                    output.push(self.allocate(RuntimeHostValueKind::Path, HostValue::Path(child)));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(
                    output,
                ))))
            }
            ("std.fs.rename", [from, to]) => {
                let from = match self.filesystem_path(from) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let to = match self.filesystem_path(to) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                match std::fs::rename(from, to) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.fs.atomicWrite", [receiver, bytes]) => {
                let target = match self.filesystem_path(receiver) {
                    Ok(path) => path,
                    Err(error) => return Ok(self.fs_result_error(error)),
                };
                let bytes = self.bytes(bytes)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.fs_result_error(message));
                }
                let suffix = NEXT_ATOMIC_TEMP.fetch_add(1, Ordering::Relaxed);
                let temporary =
                    target.with_file_name(format!(".tondo-atomic-{}-{suffix}", std::process::id()));
                let result = (|| -> io::Result<()> {
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&temporary)?;
                    file.write_all(&bytes)?;
                    file.flush()?;
                    drop(file);
                    std::fs::rename(&temporary, &target)
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&temporary);
                }
                match result {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))),
                    Err(error) => Ok(self.fs_result_error(error.to_string())),
                }
            }
            ("std.text.String.empty", []) => Ok(RuntimeValue::String(String::new())),
            ("std.text.String.fromChars", [chars]) => {
                let text = Self::array_chars(chars)?;
                if let Err(message) = self.ensure_bytes_len(text.len()) {
                    return Ok(self.text_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(text))))
            }
            ("std.text.String.length", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::Integer(text.chars().count() as i128))
            }
            ("std.text.String.byteLength", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::Integer(text.len() as i128))
            }
            ("std.text.String.get", [RuntimeValue::String(text), RuntimeValue::Integer(index)]) => {
                let value = usize::try_from(*index)
                    .ok()
                    .and_then(|index| text.chars().nth(index))
                    .map(|value| RuntimeValue::OptionSome(Box::new(RuntimeValue::Char(value))))
                    .unwrap_or(RuntimeValue::OptionNone);
                Ok(value)
            }
            (
                "std.text.String.slice",
                [
                    RuntimeValue::String(text),
                    RuntimeValue::Integer(start),
                    RuntimeValue::Integer(end),
                ],
            ) => {
                let length = text.chars().count();
                let Some(start) = usize::try_from(*start).ok() else {
                    return Ok(self.text_result_error("slice start is not a valid scalar index"));
                };
                let Some(end) = usize::try_from(*end).ok() else {
                    return Ok(self.text_result_error("slice end is not a valid scalar index"));
                };
                if start > length || end > length {
                    return Ok(self.text_result_error(format!(
                        "slice [{start}, {end}) is outside a string of length {length}"
                    )));
                }
                if start > end {
                    return Ok(
                        self.text_result_error(format!("slice start {start} is after end {end}"))
                    );
                }
                let start_byte = text
                    .char_indices()
                    .nth(start)
                    .map_or(text.len(), |(offset, _)| offset);
                let end_byte = text
                    .char_indices()
                    .nth(end)
                    .map_or(text.len(), |(offset, _)| offset);
                let sliced = &text[start_byte..end_byte];
                if let Err(message) = self.ensure_bytes_len(sliced.len()) {
                    return Ok(self.text_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(
                    sliced.to_owned(),
                ))))
            }
            ("std.text.String.chars", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::String(text.clone()))
            }
            (
                "std.text.String.contains",
                [RuntimeValue::String(text), RuntimeValue::String(needle)],
            ) => Ok(RuntimeValue::Bool(text.contains(needle))),
            (
                "std.text.String.startsWith",
                [RuntimeValue::String(text), RuntimeValue::String(prefix)],
            ) => Ok(RuntimeValue::Bool(text.starts_with(prefix))),
            (
                "std.text.String.endsWith",
                [RuntimeValue::String(text), RuntimeValue::String(suffix)],
            ) => Ok(RuntimeValue::Bool(text.ends_with(suffix))),
            (
                "std.text.String.find",
                [RuntimeValue::String(text), RuntimeValue::String(needle)],
            ) => {
                let value = text.find(needle).map(|byte_offset| {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(
                        text[..byte_offset].chars().count() as i128,
                    )))
                });
                Ok(value.unwrap_or(RuntimeValue::OptionNone))
            }
            (
                "std.text.String.replace",
                [
                    RuntimeValue::String(text),
                    RuntimeValue::String(old),
                    RuntimeValue::String(new),
                ],
            ) => Ok(RuntimeValue::String(text.replace(old, new))),
            ("std.text.String.trim", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::String(text.trim().to_owned()))
            }
            ("std.text.String.toLowerAscii", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::String(text.to_ascii_lowercase()))
            }
            ("std.text.String.toUpperAscii", [RuntimeValue::String(text)]) => {
                Ok(RuntimeValue::String(text.to_ascii_uppercase()))
            }
            ("std.testing.log", [RuntimeValue::String(message)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(&envelope, envelope.log(message.clone()))
            }
            ("std.testing.assertEqual", [expected, actual]) => {
                let envelope = self.testing_envelope()?;
                let result = if expected == actual {
                    Ok(())
                } else {
                    Err(ControlError::FailNow {
                        message: format!(
                            "assertion failed: expected {}, actual {}",
                            testing_value_text(expected),
                            testing_value_text(actual)
                        ),
                    })
                };
                Self::testing_result(&envelope, result)
            }
            ("std.testing.assertNotEqual", [expected, actual]) => {
                let envelope = self.testing_envelope()?;
                let result = if expected != actual {
                    Ok(())
                } else {
                    Err(ControlError::FailNow {
                        message: format!(
                            "assertion failed: values are equal ({})",
                            testing_value_text(expected)
                        ),
                    })
                };
                Self::testing_result(&envelope, result)
            }
            (
                "std.testing.assertTextEqual",
                [RuntimeValue::String(expected), RuntimeValue::String(actual)],
            ) => {
                let envelope = self.testing_envelope()?;
                let result = if expected == actual {
                    Ok(())
                } else {
                    Err(ControlError::FailNow {
                        message: format!(
                            "text assertion failed\n{}",
                            diff_text(expected, actual).render()
                        ),
                    })
                };
                Self::testing_result(&envelope, result)
            }
            (
                "std.testing.diffText",
                [RuntimeValue::String(expected), RuntimeValue::String(actual)],
            ) => {
                let diff = diff_text(expected, actual);
                if let Err(message) = self.ensure_bytes_len(diff.render().len()) {
                    return Err(VmError::Host(message));
                }
                Ok(self.allocate(RuntimeHostValueKind::TextDiff, HostValue::TextDiff(diff)))
            }
            ("std.testing.TextDiff.render", [value]) => {
                let rendered = self.text_diff(value)?.render();
                if let Err(message) = self.ensure_bytes_len(rendered.len()) {
                    return Err(VmError::Host(message));
                }
                Ok(RuntimeValue::String(rendered))
            }
            ("std.testing.tempDirectory", [RuntimeValue::String(prefix)]) => {
                if !Self::valid_temp_prefix(prefix) {
                    return Ok(self.temp_result_error("temporary directory prefix is invalid"));
                }
                let root = PathBuf::from("target").join(".tondo-test-root");
                if let Err(error) = std::fs::create_dir_all(&root) {
                    return Ok(self.temp_result_error(error.to_string()));
                }
                let nonce = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
                let name = if prefix.is_empty() {
                    format!("tondo-{}-{nonce}", std::process::id())
                } else {
                    format!("{prefix}-{}-{nonce}", std::process::id())
                };
                let directory = root.join(name);
                match std::fs::create_dir(&directory) {
                    Ok(()) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                        RuntimeHostValueKind::TempDirectory,
                        HostValue::TempDirectory { path: directory },
                    )))),
                    Err(error) => Ok(self.temp_result_error(error.to_string())),
                }
            }
            ("std.testing.TempDirectory.path", [value]) => {
                let directory = self.temp_directory(value)?;
                let bytes = directory.to_string_lossy().as_bytes().to_vec();
                let path = path::Path::from_bytes(&bytes).map_err(|error| {
                    VmError::Host(format!("temporary path is invalid: {error:?}"))
                })?;
                Ok(self.allocate(RuntimeHostValueKind::Path, HostValue::Path(path)))
            }
            ("std.testing.TempDirectory.cleanup", [value]) => {
                let RuntimeValue::Host {
                    kind: RuntimeHostValueKind::TempDirectory,
                    id,
                } = value
                else {
                    return Err(VmError::Host("TempDirectory value is invalid".into()));
                };
                let directory = self.temp_directory(value)?;
                let mut entries = 0;
                Self::remove_temp_tree(&directory, &mut entries).map_err(|error| {
                    VmError::Host(format!("temporary directory cleanup failed: {error}"))
                })?;
                self.values.remove(id);
                Ok(RuntimeValue::Unit)
            }
            ("std.testing.Generator.new", [RuntimeValue::Integer(seed)]) => {
                let seed = match u64::try_from(*seed) {
                    Ok(seed) => seed,
                    Err(_) => return Ok(self.generation_result_error("seed is outside UInt64")),
                };
                Ok(self.allocate(
                    RuntimeHostValueKind::Generator,
                    HostValue::Generator(Generator::new(seed)),
                ))
            }
            (
                "std.testing.Generator.forCase",
                [
                    RuntimeValue::Integer(seed),
                    RuntimeValue::Integer(case_index),
                ],
            ) => {
                let (Ok(seed), Ok(case_index)) = (u64::try_from(*seed), u64::try_from(*case_index))
                else {
                    return Ok(self.generation_result_error("seed or case index is outside UInt64"));
                };
                Ok(self.allocate(
                    RuntimeHostValueKind::Generator,
                    HostValue::Generator(Generator::for_case(seed, case_index)),
                ))
            }
            ("std.testing.Generator.id", [generator]) => {
                let id = self.generator_mut(generator)?.id();
                Ok(self.allocate(
                    RuntimeHostValueKind::GenerationId,
                    HostValue::GenerationId {
                        seed: id.seed,
                        case_index: id.case_index,
                    },
                ))
            }
            ("std.testing.Generator.nextUInt", [generator]) => {
                match self.generator_mut(generator)?.next_u64() {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                        i128::from(value),
                    )))),
                    Err(error) => Ok(self.generation_result_error(format!("{error:?}"))),
                }
            }
            ("std.testing.Generator.nextBool", [generator]) => {
                match self.generator_mut(generator)?.next_bool() {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Bool(value)))),
                    Err(error) => Ok(self.generation_result_error(format!("{error:?}"))),
                }
            }
            (
                "std.testing.Generator.nextInt",
                [
                    generator,
                    RuntimeValue::Integer(minimum),
                    RuntimeValue::Integer(maximum),
                ],
            ) => match self.generator_mut(generator)?.next_int(*minimum, *maximum) {
                Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                    value,
                )))),
                Err(error) => Ok(self.generation_result_error(format!("{error:?}"))),
            },
            ("std.testing.Generator.nextBytes", [generator, RuntimeValue::Integer(maximum)]) => {
                let Ok(maximum) = usize::try_from(*maximum) else {
                    return Ok(self.generation_result_error("maximum length is invalid"));
                };
                match self.generator_mut(generator)?.next_bytes(maximum) {
                    Ok(value) => {
                        if let Err(message) = self.ensure_bytes_len(value.len()) {
                            return Ok(self.generation_result_error(message));
                        }
                        Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                            RuntimeHostValueKind::Bytes,
                            HostValue::Bytes(value),
                        ))))
                    }
                    Err(error) => Ok(self.generation_result_error(format!("{error:?}"))),
                }
            }
            ("std.testing.Generator.nextText", [generator, RuntimeValue::Integer(maximum)]) => {
                let Ok(maximum) = usize::try_from(*maximum) else {
                    return Ok(self.generation_result_error("maximum length is invalid"));
                };
                match self.generator_mut(generator)?.next_text(maximum) {
                    Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(
                        value,
                    )))),
                    Err(error) => Ok(self.generation_result_error(format!("{error:?}"))),
                }
            }
            ("std.testing.Generator.drawCount", [generator]) => Ok(RuntimeValue::Integer(
                i128::from(self.generator_mut(generator)?.draw_count()),
            )),
            ("std.testing.shrink", [value]) => {
                let candidates = match shrink_runtime_value(value, MAX_SHRINK_CANDIDATES, 0) {
                    Ok(candidates) => candidates,
                    Err(message) => return Ok(self.generation_result_error(message)),
                };
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(
                    candidates,
                ))))
            }
            ("std.testing.assertSome", [RuntimeValue::OptionSome(value)]) => Ok((**value).clone()),
            ("std.testing.assertSome", [RuntimeValue::OptionNone]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(
                    &envelope,
                    Err(ControlError::FailNow {
                        message: "assertion failed: expected Some, got None".into(),
                    }),
                )
            }
            ("std.testing.assertNone", [RuntimeValue::OptionNone]) => Ok(RuntimeValue::Unit),
            ("std.testing.assertNone", [RuntimeValue::OptionSome(value)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(
                    &envelope,
                    Err(ControlError::FailNow {
                        message: format!(
                            "assertion failed: expected None, got Some({})",
                            testing_value_text(value)
                        ),
                    }),
                )
            }
            ("std.testing.assertOk", [RuntimeValue::ResultOk(value)]) => Ok((**value).clone()),
            ("std.testing.assertOk", [RuntimeValue::ResultErr(error)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(
                    &envelope,
                    Err(ControlError::FailNow {
                        message: format!(
                            "assertion failed: expected Ok, got Err({})",
                            testing_value_text(error)
                        ),
                    }),
                )
            }
            ("std.testing.assertErr", [RuntimeValue::ResultErr(error)]) => Ok((**error).clone()),
            ("std.testing.assertErr", [RuntimeValue::ResultOk(value)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(
                    &envelope,
                    Err(ControlError::FailNow {
                        message: format!(
                            "assertion failed: expected Err, got Ok({})",
                            testing_value_text(value)
                        ),
                    }),
                )
            }
            (
                "std.testing.FloatTolerance.from",
                [RuntimeValue::Float(absolute), RuntimeValue::Float(relative)],
            ) => match FloatTolerance::new(*absolute, *relative) {
                Ok(tolerance) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::FloatTolerance,
                    HostValue::FloatTolerance(tolerance),
                )))),
                Err(error) => Ok(self.float_tolerance_result_error(format!("{error:?}"))),
            },
            (
                "std.testing.assertFloatNear",
                [
                    RuntimeValue::Float(expected),
                    RuntimeValue::Float(actual),
                    tolerance,
                ],
            ) => {
                let envelope = self.testing_envelope()?;
                let tolerance = self.float_tolerance(tolerance)?;
                let result = if tolerance.is_near(*expected, *actual) {
                    Ok(())
                } else {
                    Err(ControlError::FailNow {
                        message: format!(
                            "float assertion failed: expected {expected}, actual {actual}, absolute {}, relative {}",
                            tolerance.absolute, tolerance.relative
                        ),
                    })
                };
                Self::testing_result(&envelope, result)
            }
            (
                "std.testing.assertFloat32Near",
                [
                    RuntimeValue::Float(expected),
                    RuntimeValue::Float(actual),
                    tolerance,
                ],
            ) => {
                let envelope = self.testing_envelope()?;
                let tolerance = self.float_tolerance(tolerance)?;
                let result = if tolerance.is_near(*expected, *actual) {
                    Ok(())
                } else {
                    Err(ControlError::FailNow {
                        message: format!(
                            "float32 assertion failed: expected {expected}, actual {actual}, absolute {}, relative {}",
                            tolerance.absolute, tolerance.relative
                        ),
                    })
                };
                Self::testing_result(&envelope, result)
            }
            /*
             * Keep this arm unreachable for older bytecode so a stale client
             * fails with a host error instead of silently accepting the old
             * four-float ABI.
             */
            (
                "std.testing.assertFloatNear",
                [
                    RuntimeValue::Float(_),
                    RuntimeValue::Float(_),
                    RuntimeValue::Float(_),
                    RuntimeValue::Float(_),
                ],
            ) => Err(VmError::Host(
                "std.testing.assertFloatNear uses FloatTolerance.from".into(),
            )),
            ("std.testing.tags", [RuntimeValue::Map(entries)]) => {
                let envelope = self.testing_envelope()?;
                let mut tags = BTreeMap::new();
                for (key, value) in entries {
                    let (RuntimeValue::String(key), RuntimeValue::String(value)) = (key, value)
                    else {
                        return Err(VmError::Host(
                            "std.testing.tags expects Map[String, String]".into(),
                        ));
                    };
                    tags.insert(key.clone(), value.clone());
                }
                Self::testing_result(&envelope, envelope.tags(tags))
            }
            ("std.testing.failNow", [RuntimeValue::String(message)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(&envelope, envelope.fail_now(message.clone()))
            }
            ("std.testing.skip", [RuntimeValue::String(reason)]) => {
                let envelope = self.testing_envelope()?;
                Self::testing_result(&envelope, envelope.skip(reason.clone()))
            }
            (
                "std.testing.attach",
                [
                    RuntimeValue::String(name),
                    RuntimeValue::String(media_type),
                    bytes,
                ],
            ) => {
                let envelope = self.testing_envelope()?;
                let bytes = self.bytes(bytes)?.to_vec();
                Self::testing_result(
                    &envelope,
                    envelope.attach(name.clone(), media_type.clone(), bytes),
                )
            }
            (
                "std.testing.snapshot",
                [RuntimeValue::String(name), RuntimeValue::String(actual)],
            ) => {
                let envelope = self.testing_envelope()?;
                let result = envelope.snapshot(name.clone(), actual).map(|_| ());
                Self::testing_result(&envelope, result)
            }
            ("std.time.now", []) => match self.clock.now() {
                Ok(nanos) => Ok(RuntimeValue::ResultOk(Box::new(
                    self.allocate_instant(nanos),
                ))),
                Err(_) => Ok(self.clock_result_error("monotonic clock is unavailable")),
            },
            ("std.time.resolution", []) => {
                let resolution = self.clock.resolution();
                if (INT_MIN..=INT_MAX).contains(&resolution) && resolution > 0 {
                    Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(
                        resolution,
                    ))))
                } else {
                    Ok(self.clock_result_error("clock resolution is outside the Int range"))
                }
            }
            ("std.time.deadline", [delay]) => match self.deadline_value(delay) {
                Ok(value) => Ok(RuntimeValue::ResultOk(Box::new(value))),
                Err(error) => Ok(error),
            },
            ("std.time.Duration.fromNanoseconds", [RuntimeValue::Integer(value)]) => {
                if (INT_MIN..=INT_MAX).contains(value) {
                    Ok(RuntimeValue::Integer(*value))
                } else {
                    Err(VmError::Host("nanoseconds exceed the Int range".into()))
                }
            }
            ("std.time.Duration.fromMicroseconds", [RuntimeValue::Integer(value)])
            | ("std.time.Duration.fromMilliseconds", [RuntimeValue::Integer(value)])
            | ("std.time.Duration.fromSeconds", [RuntimeValue::Integer(value)]) => {
                let factor = match name {
                    "std.time.Duration.fromMicroseconds" => NANOS_PER_MICROSECOND,
                    "std.time.Duration.fromMilliseconds" => NANOS_PER_MILLISECOND,
                    _ => NANOS_PER_SECOND,
                };
                match value.checked_mul(factor) {
                    Some(value) if (INT_MIN..=INT_MAX).contains(&value) => Ok(
                        RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(value))),
                    ),
                    _ => Ok(self.duration_result_error("duration exceeds the Int range")),
                }
            }
            ("std.time.Duration.toNanoseconds", [value]) => {
                Ok(RuntimeValue::Integer(Self::duration(value)?))
            }
            ("std.time.Duration.add", [left, right])
            | ("std.time.Duration.subtract", [left, right])
            | ("std.time.Duration.multiply", [left, right]) => {
                let left = Self::duration(left)?;
                let right = Self::duration(right)?;
                let result = match name {
                    "std.time.Duration.add" => left.checked_add(right),
                    "std.time.Duration.subtract" => left.checked_sub(right),
                    _ => left.checked_mul(right),
                };
                match result {
                    Some(value) if (INT_MIN..=INT_MAX).contains(&value) => Ok(
                        RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(value))),
                    ),
                    _ => Ok(self.duration_result_error("duration arithmetic overflow")),
                }
            }
            ("std.time.Duration.negate", [value]) => {
                let value = Self::duration(value)?;
                match value.checked_neg() {
                    Some(value) if (INT_MIN..=INT_MAX).contains(&value) => Ok(
                        RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(value))),
                    ),
                    _ => Ok(self.duration_result_error("duration arithmetic overflow")),
                }
            }
            ("std.time.Duration.isZero", [value]) => {
                Ok(RuntimeValue::Bool(Self::duration(value)? == 0))
            }
            ("std.time.Duration.isNegative", [value]) => {
                Ok(RuntimeValue::Bool(Self::duration(value)? < 0))
            }
            ("std.time.Duration.isLessThan", [left, right]) => Ok(RuntimeValue::Bool(
                Self::duration(left)? < Self::duration(right)?,
            )),
            ("std.time.Instant.add", [receiver, duration])
            | ("std.time.Instant.subtract", [receiver, duration]) => {
                let (domain, instant) = self.instant(receiver)?;
                if domain != self.clock_domain {
                    return Ok(self.clock_result_error("instant belongs to another clock domain"));
                }
                let duration = Self::duration(duration)?;
                let value = if name.ends_with(".add") {
                    instant.checked_add(duration)
                } else {
                    instant.checked_sub(duration)
                };
                match value {
                    Some(value) => Ok(RuntimeValue::ResultOk(Box::new(
                        self.allocate_instant(value),
                    ))),
                    None => Ok(self.clock_result_error("instant arithmetic overflow")),
                }
            }
            ("std.time.Instant.durationSince", [receiver, other]) => {
                let (domain, instant) = self.instant(receiver)?;
                let (other_domain, other) = self.instant(other)?;
                if domain != self.clock_domain
                    || other_domain != self.clock_domain
                    || domain != other_domain
                {
                    return Ok(self.clock_result_error("instant belongs to another clock domain"));
                }
                match instant.checked_sub(other) {
                    Some(value) if (INT_MIN..=INT_MAX).contains(&value) => Ok(
                        RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(value))),
                    ),
                    _ => Ok(
                        self.clock_result_error("instant difference exceeds the Duration range")
                    ),
                }
            }
            ("std.time.Instant.isBefore", [receiver, other])
            | ("std.time.Instant.isAfter", [receiver, other]) => {
                let (domain, instant) = self.instant(receiver)?;
                let (other_domain, other) = self.instant(other)?;
                if domain != self.clock_domain
                    || other_domain != self.clock_domain
                    || domain != other_domain
                {
                    return Ok(self.clock_result_error("instant belongs to another clock domain"));
                }
                let before = instant < other;
                let value = if name.ends_with(".isBefore") {
                    before
                } else {
                    !before && instant != other
                };
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Bool(value))))
            }
            ("std.time.Timer.after", [delay]) => match self.timer_deadline_value(delay) {
                Ok(deadline) => match self.allocate_timer(deadline) {
                    Ok(timer) => Ok(RuntimeValue::ResultOk(Box::new(timer))),
                    Err(error) => Ok(error),
                },
                Err(error) => Ok(error),
            },
            ("std.time.Timer.at", [instant]) => {
                let (domain, deadline) = self.instant(instant)?;
                if domain != self.clock_domain {
                    return Ok(self.clock_result_error("instant belongs to another clock domain"));
                }
                match self.allocate_timer(deadline) {
                    Ok(timer) => Ok(RuntimeValue::ResultOk(Box::new(timer))),
                    Err(error) => Ok(error),
                }
            }
            ("std.time.Timer.cancel", [timer]) => {
                let (domain, _) = self.timer(timer)?;
                if domain != self.clock_domain {
                    return Err(VmError::Host(
                        "timer belongs to another clock domain".into(),
                    ));
                }
                let RuntimeValue::Host { id, .. } = timer else {
                    unreachable!("timer() validated the token")
                };
                self.values.remove(id);
                self.release_time_resource();
                Ok(RuntimeValue::Unit)
            }
            ("std.env.snapshot", []) => match self.environment_snapshot() {
                Ok(snapshot) => Ok(RuntimeValue::ResultOk(Box::new(snapshot))),
                Err(error) => Ok(error),
            },
            ("std.env.Name.fromText", [RuntimeValue::String(text)]) => {
                let bytes = text.as_bytes().to_vec();
                if !Self::valid_environment_name(&bytes) {
                    return Ok(self.env_result_error("environment name is invalid"));
                }
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.env_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::EnvName,
                    HostValue::EnvName(bytes),
                ))))
            }
            ("std.env.Name.fromBytes", [bytes]) => {
                let bytes = self.bytes(bytes)?.to_vec();
                if !Self::valid_environment_name(&bytes) {
                    return Ok(self.env_result_error("environment name is invalid"));
                }
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.env_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::EnvName,
                    HostValue::EnvName(bytes),
                ))))
            }
            ("std.env.Snapshot.arguments", [snapshot]) => {
                let snapshot = self.environment_snapshot_data(snapshot)?;
                Ok(RuntimeValue::Array(
                    snapshot
                        .arguments
                        .into_iter()
                        .map(|bytes| {
                            self.allocate(
                                RuntimeHostValueKind::EnvValue,
                                HostValue::EnvValue(bytes),
                            )
                        })
                        .collect(),
                ))
            }
            ("std.env.Snapshot.get", [snapshot, name]) => {
                let snapshot = self.environment_snapshot_data(snapshot)?;
                let name = self.environment_name(name)?;
                match snapshot.entries.get(&name) {
                    Some(bytes) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(
                        Box::new(self.allocate(
                            RuntimeHostValueKind::EnvValue,
                            HostValue::EnvValue(bytes.clone()),
                        )),
                    )))),
                    None => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))),
                }
            }
            ("std.env.Value.asText", [value]) => {
                let bytes = self.environment_value(value)?;
                Ok(String::from_utf8(bytes)
                    .map(|text| RuntimeValue::OptionSome(Box::new(RuntimeValue::String(text))))
                    .unwrap_or(RuntimeValue::OptionNone))
            }
            ("std.env.Value.asBytes", [value]) => {
                let bytes = self.environment_value(value)?;
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Err(VmError::Host(message));
                }
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(bytes)))
            }
            ("std.bytes.empty", []) => Ok(RuntimeValue::ResultOk(Box::new(
                self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(Vec::new())),
            ))),
            ("intrinsic.Bytes.fromString", [RuntimeValue::String(text)]) => {
                if let Err(message) = self.ensure_bytes_len(text.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(text.as_bytes().to_vec()),
                ))))
            }
            ("std.bytes.fromArray", [array]) => {
                let bytes = Self::array_bytes(array)?;
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes),
                ))))
            }
            ("std.bytes.builder", []) => Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                RuntimeHostValueKind::BytesBuilder,
                HostValue::BytesBuilder(Vec::new()),
            )))),
            ("std.bytes.Bytes.length", [receiver]) => Ok(RuntimeValue::Integer(
                i128::try_from(self.bytes(receiver)?.len())
                    .map_err(|_| VmError::Host("Bytes length does not fit in Int".into()))?,
            )),
            ("std.bytes.Bytes.get", [receiver, RuntimeValue::Integer(index)]) => {
                let bytes = self.bytes(receiver)?;
                let Some(index) = usize::try_from(*index).ok() else {
                    return Ok(RuntimeValue::OptionNone);
                };
                Ok(bytes
                    .get(index)
                    .copied()
                    .map(|byte| RuntimeValue::OptionSome(Box::new(RuntimeValue::Byte(byte))))
                    .unwrap_or(RuntimeValue::OptionNone))
            }
            (
                "std.bytes.Bytes.slice",
                [
                    receiver,
                    RuntimeValue::Integer(start),
                    RuntimeValue::Integer(end),
                ],
            ) => {
                let bytes = self.bytes(receiver)?.to_vec();
                let Some(start) = usize::try_from(*start).ok() else {
                    return Ok(self.bytes_result_error("slice start must be non-negative"));
                };
                let Some(end) = usize::try_from(*end).ok() else {
                    return Ok(self.bytes_result_error("slice end must be non-negative"));
                };
                if start > end || end > bytes.len() {
                    return Ok(self.bytes_result_error(format!(
                        "slice [{start}, {end}) is outside a buffer of length {}",
                        bytes.len()
                    )));
                }
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes[start..end].to_vec()),
                ))))
            }
            ("std.bytes.Bytes.toArray", [receiver]) => {
                let bytes = self.bytes(receiver)?.to_vec();
                if let Err(message) = self.ensure_bytes_len(bytes.len()) {
                    return Ok(self.bytes_result_error(message));
                }
                Ok(RuntimeValue::ResultOk(Box::new(Self::bytes_array(&bytes))))
            }
            ("intrinsic.String.fromBytes", [receiver]) => {
                match String::from_utf8(self.bytes(receiver)?.to_vec()) {
                    Ok(text) => Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(text)))),
                    Err(error) => {
                        let error = self.allocate(
                            RuntimeHostValueKind::Utf8Error,
                            HostValue::Utf8Error {
                                _message: error.to_string(),
                            },
                        );
                        Ok(RuntimeValue::ResultErr(Box::new(error)))
                    }
                }
            }
            ("std.bytes.Bytes.equal", [left, right]) => {
                Ok(RuntimeValue::Bool(self.bytes(left)? == self.bytes(right)?))
            }
            ("std.bytes.Bytes.hash", [receiver]) => {
                let mut hash = 14_695_981_039_346_656_037_u64;
                for byte in self.bytes(receiver)? {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(1_099_511_628_211);
                }
                Ok(RuntimeValue::Integer(i128::from(hash)))
            }
            ("std.bytes.BytesBuilder.length", [receiver]) => Ok(RuntimeValue::Integer(
                i128::try_from(self.builder(receiver)?.len())
                    .map_err(|_| VmError::Host("BytesBuilder length does not fit in Int".into()))?,
            )),
            ("std.bytes.BytesBuilder.appendByte", [receiver, RuntimeValue::Byte(byte)]) => {
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(1) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.push(*byte);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.append", [receiver, bytes]) => {
                let appended = self.bytes(bytes)?.to_vec();
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(appended.len()) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.extend_from_slice(&appended);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.appendArray", [receiver, array]) => {
                let appended = Self::array_bytes(array)?;
                let current = self.builder(receiver)?.len();
                let Some(length) = current.checked_add(appended.len()) else {
                    return Ok(self.bytes_result_error("BytesBuilder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.bytes_result_error(message));
                }
                self.builder_mut(receiver)?.extend_from_slice(&appended);
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.bytes.BytesBuilder.finish", [receiver]) => {
                let bytes = self.builder(receiver)?.to_vec();
                Ok(RuntimeValue::ResultOk(Box::new(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(bytes),
                ))))
            }
            ("std.format.Builder.new", []) => Ok(self.allocate(
                RuntimeHostValueKind::FormatBuilder,
                HostValue::FormatBuilder(Vec::new()),
            )),
            ("std.format.Builder.append", [receiver, RuntimeValue::String(text)]) => {
                let current = self.format_builder(receiver)?.len();
                let Some(length) = current.checked_add(text.len()) else {
                    return Ok(self.format_result_error("format Builder length overflow"));
                };
                if let Err(message) = self.ensure_bytes_len(length) {
                    return Ok(self.format_result_error(message));
                }
                self.format_builder_mut(receiver)?
                    .extend_from_slice(text.as_bytes());
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
            }
            ("std.format.Builder.finish", [receiver]) => {
                let bytes = self.format_builder(receiver)?.to_vec();
                let text = String::from_utf8(bytes)
                    .map_err(|_| VmError::Host("format Builder contains invalid UTF-8".into()))?;
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::String(text))))
            }
            ("std.collections.Array.new", []) => Ok(RuntimeValue::Array(Vec::new())),
            ("std.collections.Array.withCapacity", [RuntimeValue::Integer(capacity)]) => {
                let Ok(capacity) = usize::try_from(*capacity) else {
                    return Ok(self.collection_result_error(
                        "array capacity must be non-negative and fit the host size",
                    ));
                };
                if capacity as u64 > self.max_bytes {
                    return Ok(self.collection_result_error(
                        "array capacity exceeds the configured collection limit",
                    ));
                }
                let mut values = Vec::new();
                if values.try_reserve_exact(capacity).is_err() {
                    return Ok(self.collection_result_error(
                        "array capacity exceeds the configured collection limit",
                    ));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(
                    values,
                ))))
            }
            ("std.collections.Array.length", [RuntimeValue::Array(values)]) => {
                Ok(RuntimeValue::Integer(
                    i128::try_from(values.len())
                        .map_err(|_| VmError::Host("Array length does not fit in Int".into()))?,
                ))
            }
            (
                "std.collections.Array.get",
                [RuntimeValue::Array(values), RuntimeValue::Integer(index)],
            ) => {
                let index = if *index < 0 {
                    (*index)
                        .checked_neg()
                        .and_then(|offset| usize::try_from(offset).ok())
                        .and_then(|offset| values.len().checked_sub(offset))
                } else {
                    usize::try_from(*index).ok()
                };
                Ok(index
                    .and_then(|index| values.get(index))
                    .cloned()
                    .map(|value| RuntimeValue::OptionSome(Box::new(value)))
                    .unwrap_or(RuntimeValue::OptionNone))
            }
            (
                "std.collections.Array.slice",
                [
                    RuntimeValue::Array(values),
                    RuntimeValue::Integer(start),
                    RuntimeValue::Integer(end),
                ],
            ) => {
                let Some(start) = usize::try_from(*start).ok() else {
                    return Ok(self.collection_result_error("slice start must be non-negative"));
                };
                let Some(end) = usize::try_from(*end).ok() else {
                    return Ok(self.collection_result_error("slice end must be non-negative"));
                };
                if start > end || end > values.len() {
                    return Ok(self.collection_result_error(format!(
                        "slice [{start}, {end}) is outside an array of length {}",
                        values.len()
                    )));
                }
                Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(
                    values[start..end].to_vec(),
                ))))
            }
            ("std.collections.Map.new", []) => Ok(RuntimeValue::Map(Vec::new())),
            ("std.collections.Map.get", [RuntimeValue::Map(entries), key]) => Ok(entries
                .iter()
                .find(|(entry_key, _)| entry_key == key)
                .map(|(_, value)| RuntimeValue::OptionSome(Box::new(value.clone())))
                .unwrap_or(RuntimeValue::OptionNone)),
            ("std.collections.Map.contains", [RuntimeValue::Map(entries), key]) => Ok(
                RuntimeValue::Bool(entries.iter().any(|(entry_key, _)| entry_key == key)),
            ),
            ("std.collections.Set.new", []) => Ok(RuntimeValue::Set(Vec::new())),
            ("std.collections.Set.contains", [RuntimeValue::Set(values), value]) => {
                Ok(RuntimeValue::Bool(values.iter().any(|item| item == value)))
            }
            ("std.process.args", []) => Ok(RuntimeValue::Array(
                self.arguments
                    .iter()
                    .cloned()
                    .map(RuntimeValue::String)
                    .collect(),
            )),
            (
                "std.process.cmd" | "std.process.command",
                [
                    RuntimeValue::String(program),
                    RuntimeValue::Array(arguments),
                ],
            ) => {
                let arguments = arguments
                    .iter()
                    .map(|argument| match argument {
                        RuntimeValue::String(argument) => Ok(argument.clone()),
                        _ => Err(VmError::Host(
                            "std.process.command received a non-String argument".into(),
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(self.allocate(
                    RuntimeHostValueKind::Command,
                    HostValue::Command(ProcessPlan {
                        stages: vec![ProcessStage {
                            program: program.clone(),
                            arguments,
                            merge_stderr: false,
                        }],
                    }),
                ))
            }
            ("std.process.shell", [RuntimeValue::String(text)]) => Ok(self.allocate(
                RuntimeHostValueKind::Command,
                HostValue::Command(ProcessPlan {
                    stages: vec![shell_stage(text)],
                }),
            )),
            ("std.process.pipe", [left, right]) => {
                let mut plan = self.plan(left)?;
                plan.stages.extend(self.plan(right)?.stages);
                Ok(self.allocate(RuntimeHostValueKind::Pipeline, HostValue::Pipeline(plan)))
            }
            ("std.process.Command.mergeStderr", [receiver])
            | ("std.process.Pipeline.mergeStderr", [receiver]) => {
                let kind = match receiver {
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Command,
                        ..
                    } => RuntimeHostValueKind::Command,
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Pipeline,
                        ..
                    } => RuntimeHostValueKind::Pipeline,
                    _ => return Err(VmError::Host("process plan is not a host value".into())),
                };
                let mut plan = self.plan(receiver)?;
                let stage = plan
                    .stages
                    .last_mut()
                    .ok_or_else(|| VmError::Host("cannot redirect an empty process plan".into()))?;
                stage.merge_stderr = true;
                Ok(self.allocate(
                    kind,
                    if kind == RuntimeHostValueKind::Command {
                        HostValue::Command(plan)
                    } else {
                        HostValue::Pipeline(plan)
                    },
                ))
            }
            ("std.process.Command.start" | "std.process.Pipeline.start", [receiver]) => {
                match ProcessGroup::spawn(&self.plan(receiver)?, Arc::new(AtomicBool::new(false))) {
                    Ok(group) => {
                        let handle = self.allocate(
                            RuntimeHostValueKind::ProcessHandle,
                            HostValue::ProcessHandle(group),
                        );
                        Ok(RuntimeValue::ResultOk(Box::new(handle)))
                    }
                    Err(message) => Ok(self.result_error(message)),
                }
            }
            ("std.process.ProcessOutput.stdout", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output.stdout)))
            }
            ("std.process.ProcessOutput.stderr", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(output.stderr)))
            }
            ("std.process.ProcessOutput.combined", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(output.combined),
                ))
            }
            ("std.process.ProcessOutput.statuses", [receiver]) => {
                let output = self.output(receiver)?;
                Ok(self.allocate_statuses(output.statuses))
            }
            ("std.process.ExitStatus.code", [receiver]) => Ok(match self.status(receiver)?.code {
                Some(code) => {
                    RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(i128::from(code))))
                }
                None => RuntimeValue::OptionNone,
            }),
            ("std.process.ExitStatus.success", [receiver]) => {
                Ok(RuntimeValue::Bool(self.status(receiver)?.success))
            }
            ("std.console.print", _) => Err(VmError::Host(
                "std.console.print received an invalid bootstrap argument list".into(),
            )),
            ("std.console.println", _) => Err(VmError::Host(
                "std.console.println received an invalid bootstrap argument list".into(),
            )),
            ("std.console.flush", _) => Err(VmError::Host(
                "std.console.flush received an invalid bootstrap argument list".into(),
            )),
            _ => Err(VmError::UnsupportedHostCall(name.to_owned())),
        }
    }

    fn start_async(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<u64, VmError> {
        if matches!(
            name,
            "std.console.flush"
                | "std.console.readLine"
                | "std.process.Command.start"
                | "std.process.Pipeline.start"
                | "std.io.Reader.read"
                | "std.io.Writer.write"
                | "std.io.Writer.flush"
                | "std.io.readAll"
                | "std.io.writeAll"
                | "std.fs.open"
                | "std.fs.openDirectory"
                | "std.fs.readAll"
                | "std.fs.writeAll"
                | "std.fs.createDirectory"
                | "std.fs.remove"
                | "std.fs.metadata"
                | "std.fs.list"
                | "std.fs.rename"
                | "std.fs.atomicWrite"
                | "std.fs.File.read"
                | "std.fs.File.write"
                | "std.fs.File.flush"
                | "std.fs.Directory.list"
                | "std.json.JsonReader.fromReader"
                | "std.json.JsonWriter.toWriter"
                | "std.json.JsonWriter.write"
                | "std.json.JsonWriter.finish"
                | "std.messagepack.MessagePackReader.fromReader"
                | "std.messagepack.MessagePackWriter.toWriter"
                | "std.messagepack.MessagePackWriter.write"
                | "std.messagepack.MessagePackWriter.finish"
                | "std.protobuf.ProtoReader.fromReader"
                | "std.protobuf.ProtoWriter.toWriter"
                | "std.protobuf.ProtoWriter.write"
                | "std.protobuf.ProtoWriter.finish"
        ) {
            let call = self.next_job;
            self.next_job = self
                .next_job
                .checked_add(1)
                .ok_or_else(|| VmError::Host("async call identity space exhausted".into()))?;
            let result = self.invoke(name, arguments);
            if name.starts_with("std.fs.") {
                self.ready_fs_jobs.insert(call);
            }
            self.ready_jobs.insert(call, result);
            return Ok(call);
        }
        if name == "std.testing.VirtualTime.settle" {
            let [controller] = arguments else {
                return Err(VmError::Host(
                    "VirtualTime.settle received an invalid argument list".into(),
                ));
            };
            self.virtual_controller(controller)?;
            return self.start_virtual_control_job(TimeJobKind::Settle);
        }
        if name == "std.testing.VirtualTime.advance" {
            let [controller, duration] = arguments else {
                return Err(VmError::Host(
                    "VirtualTime.advance received an invalid argument list".into(),
                ));
            };
            self.virtual_controller(controller)?;
            let duration = Self::duration(duration)?;
            if duration < 0 {
                return Err(VmError::Host(
                    "P2005: virtual time duration cannot be negative".into(),
                ));
            }
            self.clock
                .advance_virtual(duration)
                .map_err(|error| VmError::Host(format!("P2005: {error}")))?;
            let target = self.clock.now()?;
            return self.start_virtual_control_job(TimeJobKind::Advance { target });
        }
        if name == "std.time.sleep" {
            let [delay] = arguments else {
                return Err(VmError::Host(
                    "std.time.sleep received an invalid bootstrap argument list".into(),
                ));
            };
            let completion = match self.validate_delay(delay) {
                Ok(delay) => {
                    let now = self.clock.now()?;
                    let deadline = now
                        .checked_add(delay)
                        .ok_or_else(|| VmError::Host("sleep deadline overflow".into()))?;
                    return self.start_time_job(deadline, None, false);
                }
                Err(error) => Some(error),
            };
            return self.start_time_job(i128::MIN, completion, false);
        }
        if name == "std.time.Timer.wait" {
            let [receiver] = arguments else {
                return Err(VmError::Host(
                    "std.time.Timer.wait received an invalid bootstrap argument list".into(),
                ));
            };
            let (domain, deadline) = self.timer(receiver)?;
            let RuntimeValue::Host { id, .. } = receiver else {
                unreachable!("timer() validated the token")
            };
            self.values.remove(id);
            let completion = (domain != self.clock_domain)
                .then(|| self.clock_result_error("timer belongs to another clock domain"));
            return self.start_time_job(deadline, completion, true);
        }
        let mode = Self::mode(name).ok_or_else(|| VmError::UnsupportedHostCall(name.to_owned()))?;
        let [receiver] = arguments else {
            return Err(VmError::Host(format!(
                "{name} received an invalid bootstrap argument list"
            )));
        };
        let group = match receiver {
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::ProcessHandle,
                id,
            } => match self.values.remove(id) {
                Some(HostValue::ProcessHandle(group)) => Ok(group),
                _ => return Err(VmError::Host("ProcessHandle token is stale".into())),
            },
            _ => Err(self.plan(receiver)?),
        };
        self.spawn_job(group, mode)
    }

    fn poll_async(&mut self, call: u64) -> Result<Option<RuntimeValue>, VmError> {
        if let Some(result) = self.ready_jobs.remove(&call) {
            self.ready_fs_jobs.remove(&call);
            return result.map(Some);
        }
        if self.time_jobs.contains_key(&call) {
            let ready = {
                let job = self
                    .time_jobs
                    .get(&call)
                    .expect("time job presence was checked");
                job.cancellation
                    || match job.kind {
                        TimeJobKind::Ordinary => {
                            job.completion.is_some() || self.clock.now()? >= job.deadline
                        }
                        TimeJobKind::Settle => {
                            self.jobs.is_empty()
                                && self.time_jobs.iter().all(|(id, candidate)| {
                                    *id == call || !matches!(candidate.kind, TimeJobKind::Ordinary)
                                })
                        }
                        TimeJobKind::Advance { target } => {
                            self.jobs.is_empty()
                                && self.time_jobs.iter().all(|(id, candidate)| {
                                    *id == call
                                        || !matches!(candidate.kind, TimeJobKind::Ordinary)
                                        || candidate.deadline > target
                                })
                        }
                    }
            };
            return ready.then(|| self.finish_time_job(call)).transpose();
        }
        let result = {
            let job = self
                .jobs
                .get(&call)
                .ok_or_else(|| VmError::Host(format!("unknown async host call #{call}")))?;
            match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(VmError::Host("process worker disconnected".into()));
                }
            }
        };
        result
            .map(|result| self.finish_job(call, result))
            .transpose()
    }

    fn wait_async(&mut self, calls: &[u64]) -> Result<(u64, RuntimeValue), VmError> {
        if calls.is_empty() {
            return Err(VmError::Host("host wait received no process calls".into()));
        }
        loop {
            for call in calls {
                if let Some(value) = self.poll_async(*call)? {
                    return Ok((*call, value));
                }
            }
            if matches!(self.clock, ClockProvider::Virtual { .. }) {
                let controller = calls.iter().find_map(|call| {
                    self.time_jobs.get(call).and_then(|job| match job.kind {
                        TimeJobKind::Settle => Some((TimeJobKind::Settle, i128::MAX)),
                        TimeJobKind::Advance { target } => {
                            Some((TimeJobKind::Advance { target }, target))
                        }
                        TimeJobKind::Ordinary => None,
                    })
                });
                if controller.is_some() && !self.jobs.is_empty() {
                    return Err(VmError::Host(
                        "P2003: virtual-time quiescence is blocked by an external wait".into(),
                    ));
                }
                let limit = controller.map_or(i128::MAX, |(_, limit)| limit);
                let next = calls
                    .iter()
                    .filter_map(|call| self.time_jobs.get(call))
                    .filter(|job| {
                        matches!(job.kind, TimeJobKind::Ordinary)
                            && job.completion.is_none()
                            && !job.cancellation
                            && job.deadline <= limit
                    })
                    .map(|job| job.deadline)
                    .min();
                if let Some(deadline) = next {
                    let now = self.clock.now()?;
                    if deadline > now {
                        self.clock.advance_virtual(deadline - now)?;
                        if let Some(envelope) = &self.testing {
                            envelope
                                .record_runtime_virtual_auto_advance(deadline)
                                .map_err(|error| {
                                    VmError::Host(format!("{}: {error}", error.code()))
                                })?;
                        }
                    }
                    continue;
                }
                if controller.is_some() {
                    continue;
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn cancel_async(&mut self, call: u64) -> Result<(), VmError> {
        if self.ready_jobs.contains_key(&call) {
            let cancelled = if self.ready_fs_jobs.contains(&call) {
                self.fs_result_error("operation cancelled")
            } else {
                self.io_result_error("operation cancelled")
            };
            self.ready_jobs.insert(call, Ok(cancelled));
            return Ok(());
        }
        if let Some(job) = self.time_jobs.get_mut(&call) {
            job.cancellation = true;
            return Ok(());
        }
        let job = self
            .jobs
            .get(&call)
            .ok_or_else(|| VmError::Host(format!("unknown async host call #{call}")))?;
        job.cancellation.store(true, Ordering::Release);
        Ok(())
    }

    fn cleanup(&mut self, value: &RuntimeValue) -> Result<(), VmError> {
        let RuntimeValue::Host {
            kind:
                kind @ (RuntimeHostValueKind::ProcessHandle
                | RuntimeHostValueKind::Timer
                | RuntimeHostValueKind::Reader
                | RuntimeHostValueKind::Writer
                | RuntimeHostValueKind::File
                | RuntimeHostValueKind::Directory),
            id,
        } = value
        else {
            return Ok(());
        };
        if matches!(kind, RuntimeHostValueKind::Timer) && self.values.remove(id).is_some() {
            self.release_time_resource();
        } else {
            self.values.remove(id);
        }
        Ok(())
    }

    fn begin_test_node(&mut self, kind: VmTestNodeKind, id: &str) -> Result<(), VmError> {
        let participation = self.testing_participation.clone().ok_or_else(|| {
            VmError::Host("test node boundary has no installed participation".into())
        })?;
        let kind = match kind {
            VmTestNodeKind::Leaf => TestExecutionKind::Leaf,
            VmTestNodeKind::Suite => TestExecutionKind::Suite,
        };
        let envelope = participation.envelope(id, kind).map_err(VmError::Host)?;
        self.testing_stack.push(self.testing.take());
        self.testing = Some(envelope);
        Ok(())
    }

    fn finish_test_node(
        &mut self,
        kind: VmTestNodeKind,
        id: &str,
        outcome: VmTestNodeOutcome,
    ) -> Result<(), VmError> {
        let participation = self.testing_participation.clone().ok_or_else(|| {
            VmError::Host("test node boundary has no installed participation".into())
        })?;
        let envelope = self
            .testing
            .take()
            .ok_or_else(|| VmError::Host(format!("test node `{id}` lost its evidence envelope")))?;
        let previous = self
            .testing_stack
            .pop()
            .ok_or_else(|| VmError::Host(format!("test node `{id}` has no enclosing envelope")))?;
        self.testing = previous;
        let kind = match kind {
            VmTestNodeKind::Leaf => TestExecutionKind::Leaf,
            VmTestNodeKind::Suite => TestExecutionKind::Suite,
        };
        let panic = match outcome {
            VmTestNodeOutcome::Passed => None,
            VmTestNodeOutcome::Panicked(panic) => Some(panic),
        };
        participation
            .finish(id, kind, envelope, panic)
            .map_err(VmError::Host)
    }

    fn begin_test_suite_cleanup(&mut self) -> Result<(), VmError> {
        let envelope = self.testing_envelope()?;
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Cleanup)
            .map_err(|error| VmError::Host(error.to_string()))
    }
}

impl Drop for BootstrapHost {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancellation.store(true, Ordering::Release);
        }
        for (_, mut job) in std::mem::take(&mut self.jobs) {
            if let Some(worker) = job.worker.take() {
                let _ = worker.join();
            }
        }
    }
}

struct ProcessCapture {
    handle: JoinHandle<io::Result<()>>,
}

struct ProcessGroup {
    children: Vec<Child>,
    captures: Vec<ProcessCapture>,
    events: Option<mpsc::Receiver<(usize, StreamKind, Vec<u8>)>>,
    cancellation: Arc<AtomicBool>,
}

impl ProcessGroup {
    fn spawn(plan: &ProcessPlan, cancellation: Arc<AtomicBool>) -> Result<Self, String> {
        if plan.stages.is_empty() {
            return Err("cannot execute an empty process plan".into());
        }
        let mut children = Vec::with_capacity(plan.stages.len());
        let mut captures = Vec::with_capacity(plan.stages.len() + 1);
        let (sender, receiver) = mpsc::channel();
        let mut previous_stdin: Option<Stdio> = None;

        for (index, stage) in plan.stages.iter().enumerate() {
            let final_stage = index + 1 == plan.stages.len();
            let mut command = OsCommand::new(&stage.program);
            command.args(&stage.arguments);
            configure_process_group(&mut command);
            if let Some(stdin) = previous_stdin.take() {
                command.stdin(stdin);
            }

            if !final_stage && stage.merge_stderr {
                let (reader, writer) = match pipe() {
                    Ok(pipe) => pipe,
                    Err(error) => {
                        cleanup_partial(&mut children, &mut captures);
                        return Err(format!("cannot create process pipe: {error}"));
                    }
                };
                let stdout_writer = match writer.try_clone() {
                    Ok(writer) => writer,
                    Err(error) => {
                        cleanup_partial(&mut children, &mut captures);
                        return Err(format!("cannot duplicate process pipe: {error}"));
                    }
                };
                command.stdout(Stdio::from(stdout_writer));
                command.stderr(Stdio::from(writer));
                previous_stdin = Some(Stdio::from(reader));
            } else {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
            }

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(error) => {
                    cleanup_partial(&mut children, &mut captures);
                    return Err(format!("cannot spawn `{}`: {error}", stage.program));
                }
            };

            if final_stage {
                let Some(stdout) = child.stdout.take() else {
                    terminate(&mut child);
                    let _ = child.wait();
                    cleanup_partial(&mut children, &mut captures);
                    return Err(format!("cannot capture stdout for `{}`", stage.program));
                };
                let Some(stderr) = child.stderr.take() else {
                    terminate(&mut child);
                    let _ = child.wait();
                    cleanup_partial(&mut children, &mut captures);
                    return Err(format!("cannot capture stderr for `{}`", stage.program));
                };
                let stdout_capture =
                    match spawn_capture(index, stdout, StreamKind::Stdout, sender.clone()) {
                        Ok(capture) => capture,
                        Err(error) => {
                            terminate(&mut child);
                            let _ = child.wait();
                            cleanup_partial(&mut children, &mut captures);
                            return Err(format!("cannot start stdout reader: {error}"));
                        }
                    };
                captures.push(ProcessCapture {
                    handle: stdout_capture,
                });
                let stderr_kind = if stage.merge_stderr {
                    StreamKind::Stdout
                } else {
                    StreamKind::Stderr
                };
                let stderr_capture = match spawn_capture(index, stderr, stderr_kind, sender.clone())
                {
                    Ok(capture) => capture,
                    Err(error) => {
                        terminate(&mut child);
                        let _ = child.wait();
                        cleanup_partial(&mut children, &mut captures);
                        return Err(format!("cannot start stderr reader: {error}"));
                    }
                };
                captures.push(ProcessCapture {
                    handle: stderr_capture,
                });
            } else if !stage.merge_stderr {
                let Some(stdout) = child.stdout.take() else {
                    terminate(&mut child);
                    let _ = child.wait();
                    cleanup_partial(&mut children, &mut captures);
                    return Err(format!("cannot connect stdout for `{}`", stage.program));
                };
                previous_stdin = Some(Stdio::from(stdout));
                let Some(stderr) = child.stderr.take() else {
                    terminate(&mut child);
                    let _ = child.wait();
                    cleanup_partial(&mut children, &mut captures);
                    return Err(format!("cannot capture stderr for `{}`", stage.program));
                };
                let stderr_capture =
                    match spawn_capture(index, stderr, StreamKind::Stderr, sender.clone()) {
                        Ok(capture) => capture,
                        Err(error) => {
                            terminate(&mut child);
                            let _ = child.wait();
                            cleanup_partial(&mut children, &mut captures);
                            return Err(format!("cannot start stderr reader: {error}"));
                        }
                    };
                captures.push(ProcessCapture {
                    handle: stderr_capture,
                });
            }
            children.push(child);
        }
        drop(sender);

        Ok(Self {
            children,
            captures,
            events: Some(receiver),
            cancellation,
        })
    }

    fn finish(mut self) -> Result<ProcessOutput, String> {
        let mut statuses = vec![None; self.children.len()];
        let mut termination_requested = false;
        while statuses.iter().any(Option::is_none) {
            if self.cancellation.load(Ordering::Acquire) && !termination_requested {
                termination_requested = true;
                for (child, status) in self.children.iter_mut().zip(&statuses) {
                    if status.is_none() {
                        terminate(child);
                    }
                }
            }
            for (child, status) in self.children.iter_mut().zip(&mut statuses) {
                if status.is_none() {
                    *status = child
                        .try_wait()
                        .map_err(|error| format!("cannot inspect child status: {error}"))?;
                }
            }
            if statuses.iter().any(Option::is_none) {
                thread::sleep(Duration::from_millis(2));
            }
        }

        let mut stdout = Vec::new();
        let mut stderr_by_stage = BTreeMap::<usize, Vec<u8>>::new();
        let mut combined = Vec::new();
        if let Some(events) = self.events.take() {
            for (stage, stream, chunk) in events {
                combined.extend_from_slice(&chunk);
                match stream {
                    StreamKind::Stdout => stdout.extend_from_slice(&chunk),
                    StreamKind::Stderr => stderr_by_stage
                        .entry(stage)
                        .or_default()
                        .extend_from_slice(&chunk),
                    StreamKind::Stdin => {
                        return Err("process capture produced an stdin event".into());
                    }
                }
            }
        }
        for capture in self.captures.drain(..) {
            capture
                .handle
                .join()
                .map_err(|_| "process capture reader panicked".to_owned())?
                .map_err(|error| format!("cannot read process output: {error}"))?;
        }
        let stderr = stderr_by_stage.into_values().flatten().collect();
        let statuses = statuses
            .into_iter()
            .map(|status| {
                let status = status.expect("status loop waits for every child");
                ExitStatus {
                    code: status.code(),
                    success: status.success(),
                    downstream_closed_pipe: downstream_closed_pipe(&status),
                }
            })
            .collect();
        Ok(ProcessOutput {
            stdout,
            stderr,
            combined,
            statuses,
        })
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        for child in &mut self.children {
            terminate(child);
        }
        for child in &mut self.children {
            let _ = child.wait();
        }
        for capture in self.captures.drain(..) {
            let _ = capture.handle.join();
        }
    }
}

fn cleanup_partial(children: &mut [Child], captures: &mut Vec<ProcessCapture>) {
    for child in children.iter_mut() {
        terminate(child);
    }
    for child in children.iter_mut() {
        let _ = child.wait();
    }
    for capture in captures.drain(..) {
        let _ = capture.handle.join();
    }
}

fn spawn_capture<R>(
    stage: usize,
    mut stream: R,
    kind: StreamKind,
    sender: mpsc::Sender<(usize, StreamKind, Vec<u8>)>,
) -> io::Result<JoinHandle<io::Result<()>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("tondo-process-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8 * 1024];
            loop {
                let count = stream.read(&mut buffer)?;
                if count == 0 {
                    return Ok(());
                }
                sender
                    .send((stage, kind, buffer[..count].to_vec()))
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "capture receiver closed")
                    })?;
            }
        })
}

fn shell_stage(text: &str) -> ProcessStage {
    #[cfg(unix)]
    {
        ProcessStage {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), text.into()],
            merge_stderr: false,
        }
    }
    #[cfg(windows)]
    {
        ProcessStage {
            program: "cmd.exe".into(),
            arguments: vec!["/C".into(), text.into()],
            merge_stderr: false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        ProcessStage {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), text.into()],
            merge_stderr: false,
        }
    }
}

fn native_file_name_bytes(entry: &std::fs::DirEntry) -> Vec<u8> {
    let name = entry.file_name();
    #[cfg(unix)]
    {
        name.into_vec()
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

fn check_succeeded(statuses: &[ExitStatus]) -> bool {
    let mut has_satisfactory_downstream = false;
    for status in statuses.iter().rev() {
        if status.success {
            has_satisfactory_downstream = true;
        } else if !(status.downstream_closed_pipe && has_satisfactory_downstream) {
            return false;
        }
    }
    has_satisfactory_downstream
}

fn downstream_closed_pipe(status: &std::process::ExitStatus) -> bool {
    #[cfg(unix)]
    {
        status.signal() == Some(13)
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        false
    }
}

fn configure_process_group(command: &mut OsCommand) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

fn terminate(child: &mut Child) {
    if child.try_wait().is_ok_and(|status| status.is_some()) {
        return;
    }
    #[cfg(unix)]
    {
        let group = format!("-{}", child.id());
        let killed = OsCommand::new("/bin/kill")
            .args(["-KILL", "--", group.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !killed {
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use super::*;

    fn runtime_json_error_kind(value: &RuntimeValue) -> Option<u32> {
        let RuntimeValue::Record { name, values } = value else {
            return None;
        };
        if name != "JsonError" {
            return None;
        }
        let [
            RuntimeValue::Variant {
                name,
                variant,
                values,
            },
            RuntimeValue::Record { name: location, .. },
            RuntimeValue::Newtype { name: path, .. },
        ] = values.as_slice()
        else {
            return None;
        };
        (name == "JsonErrorKind"
            && values.is_empty()
            && location == "JsonLocation"
            && path == "JsonPath")
            .then_some(*variant)
    }

    #[test]
    fn json_nominal_boundary_round_trips_every_event_and_error_kind() {
        let mut host = BootstrapHost::default();
        let events = vec![
            json::JsonEvent::StartArray(None),
            json::JsonEvent::EndArray,
            json::JsonEvent::StartObject(Some(2)),
            json::JsonEvent::EndObject,
            json::JsonEvent::Key("key".into()),
            json::JsonEvent::Null,
            json::JsonEvent::Bool(true),
            json::JsonEvent::Number(json::JsonNumber::parse("42").unwrap()),
            json::JsonEvent::String("value".into()),
        ];
        for event in events {
            let runtime = host.runtime_json_event(event.clone());
            assert_eq!(host.json_event(&runtime).unwrap(), event);
        }

        for (ordinal, kind) in [
            json::JsonErrorKind::InvalidUtf8,
            json::JsonErrorKind::InvalidSyntax,
            json::JsonErrorKind::UnexpectedEof,
            json::JsonErrorKind::InvalidEscape,
            json::JsonErrorKind::InvalidUnicodeScalar,
            json::JsonErrorKind::InvalidNumber,
            json::JsonErrorKind::DuplicateKey,
            json::JsonErrorKind::UnknownField,
            json::JsonErrorKind::MissingField,
            json::JsonErrorKind::TypeMismatch,
            json::JsonErrorKind::NumberRange,
            json::JsonErrorKind::LimitExceeded,
            json::JsonErrorKind::IoError,
            json::JsonErrorKind::TrailingData,
            json::JsonErrorKind::CanonicalizationError,
        ]
        .into_iter()
        .enumerate()
        {
            let error = host.json_error_parts(
                kind,
                json::JsonLocation {
                    offset: 7,
                    line: 2,
                    column: 3,
                },
                &json::JsonPath::root(),
            );
            assert_eq!(runtime_json_error_kind(&error), Some(ordinal as u32));
            let RuntimeValue::Record { values, .. } = error else {
                unreachable!("JSON errors use the nominal record boundary")
            };
            assert!(matches!(
                values.as_slice(),
                [
                    RuntimeValue::Variant { .. },
                    RuntimeValue::Record { values, .. },
                    RuntimeValue::Newtype { value, .. },
                ] if values == &[
                    RuntimeValue::Integer(7),
                    RuntimeValue::Integer(2),
                    RuntimeValue::Integer(3),
                ] && matches!(value.as_ref(), RuntimeValue::Array(segments) if segments.is_empty())
            ));
        }
    }

    #[test]
    fn json_nominal_boundary_rejects_forged_event_shapes() {
        let host = BootstrapHost::default();
        for value in [
            RuntimeValue::Unit,
            RuntimeValue::Variant {
                name: "Other".into(),
                variant: 1,
                values: Vec::new(),
            },
            RuntimeValue::Variant {
                name: "JsonEvent".into(),
                variant: 99,
                values: Vec::new(),
            },
            RuntimeValue::Variant {
                name: "JsonEvent".into(),
                variant: 0,
                values: vec![RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(
                    -1,
                )))],
            },
            RuntimeValue::Variant {
                name: "JsonEvent".into(),
                variant: 1,
                values: vec![RuntimeValue::Unit],
            },
            RuntimeValue::Variant {
                name: "JsonEvent".into(),
                variant: 4,
                values: vec![RuntimeValue::Integer(1)],
            },
            RuntimeValue::Variant {
                name: "JsonEvent".into(),
                variant: 7,
                values: vec![RuntimeValue::String("1".into())],
            },
        ] {
            assert!(matches!(host.json_event(&value), Err(VmError::Host(_))));
        }
    }

    #[test]
    fn console_println_uses_a_stable_lf_newline() {
        let mut host = BootstrapHost::default();
        assert_eq!(
            host.invoke(
                "std.console.println",
                &[RuntimeValue::String("hello".into())],
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(host.take_stdout(), b"hello\n");
        assert!(
            host.invoke("std.console.println", &[RuntimeValue::Integer(1)])
                .is_err()
        );
        assert_eq!(
            host.invoke("std.console.flush", &[]).unwrap(),
            RuntimeValue::Unit
        );
    }

    #[test]
    fn console_streams_preserve_partial_reads_and_separate_output_channels() {
        let mut host = BootstrapHost::with_stdin(b"uno\ndos\n".to_vec());
        let reader = ok(host.invoke("std.console.stdin", &[]).unwrap());
        assert_eq!(
            host.invoke("std.console.readLine", std::slice::from_ref(&reader))
                .unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(Box::new(
                RuntimeValue::String("uno".into()),
            ))))
        );
        assert_eq!(
            host.invoke("std.console.readLine", std::slice::from_ref(&reader))
                .unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(Box::new(
                RuntimeValue::String("dos".into()),
            ))))
        );
        assert_eq!(
            host.invoke("std.console.readLine", std::slice::from_ref(&reader))
                .unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))
        );

        let mut chunks = BootstrapHost::with_stdin(b"abcdef".to_vec());
        let reader = ok(chunks.invoke("std.console.stdin", &[]).unwrap());
        let first = ok(chunks
            .invoke(
                "std.io.Reader.read",
                &[reader.clone(), RuntimeValue::Integer(2)],
            )
            .unwrap());
        let RuntimeValue::OptionSome(first) = first else {
            panic!("bounded read must return data");
        };
        assert_eq!(chunks.bytes(&first).unwrap(), b"ab");
        let second = ok(chunks
            .invoke("std.io.Reader.read", &[reader, RuntimeValue::Integer(4)])
            .unwrap());
        let RuntimeValue::OptionSome(second) = second else {
            panic!("bounded read must return data");
        };
        assert_eq!(chunks.bytes(&second).unwrap(), b"cdef");

        let bytes = chunks.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"out".to_vec()),
        );
        let stdout = ok(chunks.invoke("std.console.stdout", &[]).unwrap());
        let stderr = ok(chunks.invoke("std.console.stderr", &[]).unwrap());
        assert_eq!(
            ok(chunks
                .invoke("std.io.Writer.write", &[stdout, bytes.clone()])
                .unwrap()),
            RuntimeValue::Integer(3)
        );
        assert_eq!(
            ok(chunks
                .invoke("std.io.Writer.write", &[stderr, bytes])
                .unwrap()),
            RuntimeValue::Integer(3)
        );
        assert_eq!(chunks.stdout, b"out");
        assert_eq!(chunks.stderr, b"out");

        let mut asynchronous = BootstrapHost::with_stdin(b"line\n".to_vec());
        let reader = ok(asynchronous.invoke("std.console.stdin", &[]).unwrap());
        let read = asynchronous
            .start_async("std.console.readLine", &[reader])
            .unwrap();
        let (_, value) = asynchronous.wait_async(&[read]).unwrap();
        assert_eq!(
            value,
            RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionSome(Box::new(
                RuntimeValue::String("line".into()),
            ))))
        );
        assert_eq!(
            asynchronous
                .invoke("std.console.print", &[RuntimeValue::String("ready".into())],)
                .unwrap(),
            RuntimeValue::Unit
        );
        let flush = asynchronous.start_async("std.console.flush", &[]).unwrap();
        let (_, value) = asynchronous.wait_async(&[flush]).unwrap();
        assert_eq!(value, RuntimeValue::Unit);
        assert_eq!(asynchronous.take_stdout(), b"ready");
    }

    #[test]
    fn console_failures_are_typed_atomic_and_redacted() {
        let mut host = BootstrapHost::with_stdin(vec![0xff, b'\n']);
        let reader = ok(host.invoke("std.console.stdin", &[]).unwrap());
        let invalid = host
            .invoke("std.console.readLine", std::slice::from_ref(&reader))
            .unwrap();
        let RuntimeValue::ResultErr(error) = &invalid else {
            panic!("invalid stdin must produce ConsoleError");
        };
        let RuntimeValue::Host {
            kind: RuntimeHostValueKind::ConsoleError,
            id,
        } = error.as_ref()
        else {
            panic!("invalid stdin must preserve the nominal ConsoleError kind");
        };
        assert!(matches!(
            host.values.get(id),
            Some(HostValue::ConsoleError { _message })
                if !_message.contains('/') && !_message.contains('\\')
        ));

        // UTF-8 rejection is atomic: retrying observes the same invalid byte
        // instead of silently advancing the reader past a failed line.
        let retry = host
            .invoke("std.console.readLine", std::slice::from_ref(&reader))
            .unwrap();
        assert!(matches!(
            retry,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ConsoleError,
                    ..
                })
        ));

        // A forged reader token for stdout is still a typed public error, not
        // a host panic or an accidental read from another stream. Safe Tondo
        // code cannot construct this token because the compiler fixes the
        // stream identity at the acquisition boundary.
        let wrong_reader = host.allocate(
            RuntimeHostValueKind::Reader,
            HostValue::Reader {
                stream: StreamKind::Stdout,
                offset: 0,
            },
        );
        let wrong_stream = host
            .invoke("std.console.readLine", std::slice::from_ref(&wrong_reader))
            .unwrap();
        assert!(matches!(
            wrong_stream,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host {
                    kind: RuntimeHostValueKind::ConsoleError,
                    ..
                })
        ));
    }

    #[test]
    fn io_limits_helpers_are_bounded_and_atomic_at_the_public_host_boundary() {
        let mut host = BootstrapHost::with_stdin(b"abcdef".to_vec());
        let reader = ok(host.invoke("std.console.stdin", &[]).unwrap());
        let limits = ok(host
            .invoke(
                "std.io.limits",
                &[RuntimeValue::Integer(3), RuntimeValue::Integer(2)],
            )
            .unwrap());
        assert!(matches!(
            limits,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::IoLimits,
                ..
            }
        ));
        let rejected = host
            .invoke("std.io.readAll", &[reader.clone(), limits])
            .unwrap();
        assert!(matches!(
            rejected,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::IoError, .. })
        ));
        let first = ok(host
            .invoke(
                "std.io.Reader.read",
                &[reader.clone(), RuntimeValue::Integer(2)],
            )
            .unwrap());
        let RuntimeValue::OptionSome(first) = first else {
            panic!("a rejected readAll must not consume the reader");
        };
        assert_eq!(host.bytes(&first).unwrap(), b"ab");

        let mut exact = BootstrapHost::with_stdin(b"abcdef".to_vec());
        let reader = ok(exact.invoke("std.console.stdin", &[]).unwrap());
        let limits = ok(exact
            .invoke(
                "std.io.limits",
                &[RuntimeValue::Integer(8), RuntimeValue::Integer(2)],
            )
            .unwrap());
        let result = ok(exact.invoke("std.io.readAll", &[reader, limits]).unwrap());
        assert_eq!(exact.bytes(&result).unwrap(), b"abcdef");

        let data = exact.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"tondo".to_vec()),
        );
        let output = ok(exact.invoke("std.console.stdout", &[]).unwrap());
        assert_eq!(
            exact.invoke("std.io.writeAll", &[output, data]).unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit))
        );
        assert_eq!(exact.take_stdout(), b"tondo");
        assert!(matches!(
            exact
                .invoke(
                    "std.io.limits",
                    &[RuntimeValue::Integer(0), RuntimeValue::Integer(1)],
                )
                .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::IoError, .. })
        ));

        let mut async_host = BootstrapHost::with_stdin(b"xy".to_vec());
        let reader = ok(async_host.invoke("std.console.stdin", &[]).unwrap());
        let call = async_host
            .start_async(
                "std.io.Reader.read",
                &[reader.clone(), RuntimeValue::Integer(1)],
            )
            .unwrap();
        let (_, completed) = async_host.wait_async(&[call]).unwrap();
        let completed = ok(completed);
        let RuntimeValue::OptionSome(completed) = completed else {
            panic!("async read must complete with data");
        };
        assert_eq!(async_host.bytes(&completed).unwrap(), b"x");

        let call = async_host
            .start_async("std.io.Reader.read", &[reader, RuntimeValue::Integer(1)])
            .unwrap();
        async_host.cancel_async(call).unwrap();
        let (_, cancelled) = async_host.wait_async(&[call]).unwrap();
        assert!(matches!(
            cancelled,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::IoError, .. })
        ));

        let mut cleanup_host = BootstrapHost::with_stdin(Vec::new());
        let reader = ok(cleanup_host.invoke("std.console.stdin", &[]).unwrap());
        cleanup_host.cleanup(&reader).unwrap();
        assert!(
            cleanup_host
                .invoke("std.io.Reader.read", &[reader, RuntimeValue::Integer(1)],)
                .is_err()
        );
    }

    #[test]
    fn filesystem_preserves_native_path_bytes_and_returns_typed_errors() {
        let mut host = BootstrapHost::default();
        let native = host.allocate(
            RuntimeHostValueKind::Path,
            HostValue::Path(path::Path::from_bytes(&[0xff, b'/', b'n']).unwrap()),
        );
        let result = host.invoke("std.fs.readAll", &[native]).unwrap();
        assert!(matches!(
            result,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::FsError, .. })
        ));
    }

    #[test]
    fn path_host_boundary_preserves_native_bytes_and_lexical_semantics() {
        let mut host = BootstrapHost::default();
        let native_bytes = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0xff, b'/', b'n']),
        );
        let native = ok(host
            .invoke(
                "std.path.Path.fromBytes",
                std::slice::from_ref(&native_bytes),
            )
            .unwrap());
        let round_trip = host
            .invoke("std.path.Path.toBytes", std::slice::from_ref(&native))
            .unwrap();
        assert_eq!(host.bytes(&round_trip).unwrap(), &[0xff, b'/', b'n']);
        assert!(matches!(
            host.invoke("std.path.Path.toString", std::slice::from_ref(&native))
                .unwrap(),
            RuntimeValue::ResultErr(error)
                if matches!(error.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::PathError, .. })
        ));

        let base = ok(host
            .invoke(
                "std.path.Path.fromString",
                &[RuntimeValue::String("a/../b.txt".into())],
            )
            .unwrap());
        assert_eq!(
            host.invoke("std.path.Path.kind", std::slice::from_ref(&base))
                .unwrap(),
            RuntimeValue::Bool(false)
        );
        let parent = match host
            .invoke("std.path.Path.parent", std::slice::from_ref(&base))
            .unwrap()
        {
            RuntimeValue::OptionSome(parent) => parent,
            other => panic!("expected parent path, got {other:?}"),
        };
        let parent_text = ok(host
            .invoke("std.path.Path.toString", std::slice::from_ref(&parent))
            .unwrap());
        assert_eq!(parent_text, RuntimeValue::String("a/..".into()));

        let joined = ok(host
            .invoke(
                "std.path.Path.join",
                &[base.clone(), RuntimeValue::String("next".into())],
            )
            .unwrap());
        let joined_text = ok(host
            .invoke("std.path.Path.toString", std::slice::from_ref(&joined))
            .unwrap());
        assert_eq!(joined_text, RuntimeValue::String("a/../b.txt/next".into()));
        assert!(matches!(
            host.invoke(
                "std.path.Path.join",
                &[base.clone(), RuntimeValue::String("bad/component".into())],
            )
            .unwrap(),
            RuntimeValue::ResultErr(error)
                if matches!(error.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::PathError, .. })
        ));
        let unchanged = ok(host
            .invoke("std.path.Path.toString", std::slice::from_ref(&base))
            .unwrap());
        assert_eq!(unchanged, RuntimeValue::String("a/../b.txt".into()));
    }

    #[test]
    fn filesystem_directory_operations_are_atomic_and_ordered() {
        let mut host = BootstrapHost::default();
        let root = host.allocate(
            RuntimeHostValueKind::Path,
            HostValue::Path(
                path::Path::from_string(&format!("target/tondo-fs-host-{}", std::process::id()))
                    .unwrap(),
            ),
        );
        let _ = host.invoke("std.fs.remove", std::slice::from_ref(&root));
        assert!(matches!(
            host.invoke(
                "std.fs.createDirectory",
                &[root.clone(), RuntimeValue::Bool(true)]
            )
            .unwrap(),
            RuntimeValue::ResultOk(_)
        ));
        let file = ok(host
            .invoke(
                "std.path.Path.join",
                &[root.clone(), RuntimeValue::String("one.bin".into())],
            )
            .unwrap());
        let bytes = bytes_ok(
            host.invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("ok".into())],
            )
            .unwrap(),
        );
        assert!(matches!(
            host.invoke("std.fs.atomicWrite", &[file.clone(), bytes])
                .unwrap(),
            RuntimeValue::ResultOk(_)
        ));
        let listed = ok(host
            .invoke("std.fs.list", std::slice::from_ref(&root))
            .unwrap());
        let RuntimeValue::Array(listed) = listed else {
            panic!("filesystem list did not return an array");
        };
        assert_eq!(listed.len(), 1);
        let renamed = ok(host
            .invoke(
                "std.path.Path.join",
                &[root.clone(), RuntimeValue::String("two.bin".into())],
            )
            .unwrap());
        assert!(matches!(
            host.invoke("std.fs.rename", &[file, renamed.clone()])
                .unwrap(),
            RuntimeValue::ResultOk(_)
        ));
        assert!(matches!(
            host.invoke("std.fs.remove", &[renamed]).unwrap(),
            RuntimeValue::ResultOk(_)
        ));
        assert!(matches!(
            host.invoke("std.fs.remove", &[root]).unwrap(),
            RuntimeValue::ResultOk(_)
        ));
    }

    #[test]
    fn filesystem_handles_are_typed_async_and_cleanup_is_affine() {
        let mut host = BootstrapHost::default();
        let root = host.allocate(
            RuntimeHostValueKind::Path,
            HostValue::Path(
                path::Path::from_string(&format!("target/tondo-fs-handle-{}", std::process::id()))
                    .unwrap(),
            ),
        );
        let _ = host.invoke("std.fs.remove", std::slice::from_ref(&root));
        ok(host
            .invoke(
                "std.fs.createDirectory",
                &[root.clone(), RuntimeValue::Bool(true)],
            )
            .unwrap());
        let file = ok(host
            .invoke(
                "std.path.Path.join",
                &[root.clone(), RuntimeValue::String("data.bin".into())],
            )
            .unwrap());
        let create = host.invoke("std.fs.OpenMode.Create", &[]).unwrap();
        let handle = ok(host.invoke("std.fs.open", &[file.clone(), create]).unwrap());
        let payload = bytes_ok(
            host.invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("hello".into())],
            )
            .unwrap(),
        );
        assert_eq!(
            ok(host
                .invoke("std.fs.File.write", &[handle.clone(), payload])
                .unwrap()),
            RuntimeValue::Integer(5)
        );
        ok(host
            .invoke("std.fs.File.flush", std::slice::from_ref(&handle))
            .unwrap());
        let metadata = ok(host
            .invoke("std.fs.metadata", std::slice::from_ref(&file))
            .unwrap());
        assert!(matches!(
            metadata,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::Metadata,
                ..
            }
        ));
        host.cleanup(&handle).unwrap();
        assert!(matches!(
            host.invoke("std.fs.File.flush", std::slice::from_ref(&handle)),
            Err(VmError::Host(message)) if message.contains("stale")
        ));

        let read_mode = host.invoke("std.fs.OpenMode.Read", &[]).unwrap();
        let reader = ok(host
            .invoke("std.fs.open", &[file.clone(), read_mode])
            .unwrap());
        let call = host
            .start_async(
                "std.fs.File.read",
                &[reader.clone(), RuntimeValue::Integer(32)],
            )
            .unwrap();
        let (_, completed) = host.wait_async(&[call]).unwrap();
        let RuntimeValue::OptionSome(bytes) = ok(completed) else {
            panic!("opened file should contain data");
        };
        assert_eq!(host.bytes(&bytes).unwrap(), b"hello");
        let directory = ok(host
            .invoke("std.fs.openDirectory", std::slice::from_ref(&root))
            .unwrap());
        let listed = ok(host
            .invoke("std.fs.Directory.list", std::slice::from_ref(&directory))
            .unwrap());
        assert!(matches!(listed, RuntimeValue::Array(values) if values.len() == 1));
        host.cleanup(&directory).unwrap();
        ok(host.invoke("std.fs.remove", &[root]).unwrap());
    }

    #[test]
    fn filesystem_handles_cover_modes_errors_and_cancellation() {
        let mut host = BootstrapHost::default();
        let root = host.allocate(
            RuntimeHostValueKind::Path,
            HostValue::Path(
                path::Path::from_string(&format!("target/tondo-fs-modes-{}", std::process::id()))
                    .unwrap(),
            ),
        );
        let fs_error = |value: RuntimeValue| {
            assert!(matches!(
                value,
                RuntimeValue::ResultErr(error)
                    if matches!(error.as_ref(), RuntimeValue::Host {
                        kind: RuntimeHostValueKind::FsError,
                        ..
                    })
            ));
        };
        let _ = host.invoke("std.fs.remove", std::slice::from_ref(&root));
        ok(host
            .invoke(
                "std.fs.createDirectory",
                &[root.clone(), RuntimeValue::Bool(true)],
            )
            .unwrap());
        let file = ok(host
            .invoke(
                "std.path.Path.join",
                &[root.clone(), RuntimeValue::String("data.bin".into())],
            )
            .unwrap());
        let missing = ok(host
            .invoke(
                "std.path.Path.join",
                &[root.clone(), RuntimeValue::String("missing.bin".into())],
            )
            .unwrap());

        let read = host.invoke("std.fs.OpenMode.Read", &[]).unwrap();
        assert!(matches!(
            read,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::OpenMode,
                ..
            }
        ));
        fs_error(host.invoke("std.fs.open", &[missing, read]).unwrap());

        let create_new = host.invoke("std.fs.OpenMode.CreateNew", &[]).unwrap();
        let handle = ok(host
            .invoke("std.fs.open", &[file.clone(), create_new])
            .unwrap());
        let payload = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"abc".to_vec()),
        );
        assert_eq!(
            ok(host
                .invoke("std.fs.File.write", &[handle.clone(), payload.clone()])
                .unwrap()),
            RuntimeValue::Integer(3)
        );
        ok(host
            .invoke("std.fs.File.flush", std::slice::from_ref(&handle))
            .unwrap());
        let duplicate_create_new = host.invoke("std.fs.OpenMode.CreateNew", &[]).unwrap();
        fs_error(
            host.invoke("std.fs.open", &[file.clone(), duplicate_create_new])
                .unwrap(),
        );

        let read_mode = host.invoke("std.fs.OpenMode.Read", &[]).unwrap();
        let reader = ok(host
            .invoke("std.fs.open", &[file.clone(), read_mode])
            .unwrap());
        let chunk = ok(host
            .invoke(
                "std.fs.File.read",
                &[reader.clone(), RuntimeValue::Integer(2)],
            )
            .unwrap());
        let RuntimeValue::OptionSome(chunk) = chunk else {
            panic!("read should return the first chunk");
        };
        assert_eq!(host.bytes(&chunk).unwrap(), b"ab");
        let eof = ok(host
            .invoke(
                "std.fs.File.read",
                &[reader.clone(), RuntimeValue::Integer(2)],
            )
            .unwrap());
        let RuntimeValue::OptionSome(eof) = eof else {
            panic!("read should return the final chunk");
        };
        assert_eq!(host.bytes(&eof).unwrap(), b"c");
        assert!(matches!(
            ok(host
                .invoke(
                    "std.fs.File.read",
                    &[reader.clone(), RuntimeValue::Integer(1)],
                )
                .unwrap()),
            RuntimeValue::OptionNone
        ));
        fs_error(
            host.invoke("std.fs.File.write", &[reader.clone(), payload.clone()])
                .unwrap(),
        );
        fs_error(
            host.invoke("std.fs.File.flush", std::slice::from_ref(&reader))
                .unwrap(),
        );
        fs_error(
            host.invoke(
                "std.fs.File.read",
                &[reader.clone(), RuntimeValue::Integer(0)],
            )
            .unwrap(),
        );
        fs_error(
            host.invoke(
                "std.fs.File.read",
                &[reader.clone(), RuntimeValue::Integer(-1)],
            )
            .unwrap(),
        );

        let write_mode = host.invoke("std.fs.OpenMode.Write", &[]).unwrap();
        let writer = ok(host
            .invoke("std.fs.open", &[file.clone(), write_mode])
            .unwrap());
        fs_error(
            host.invoke(
                "std.fs.File.read",
                &[writer.clone(), RuntimeValue::Integer(1)],
            )
            .unwrap(),
        );
        assert_eq!(
            ok(host
                .invoke("std.fs.File.write", &[writer.clone(), payload])
                .unwrap()),
            RuntimeValue::Integer(3)
        );
        ok(host
            .invoke("std.fs.File.flush", std::slice::from_ref(&writer))
            .unwrap());

        let read_mode = host.invoke("std.fs.OpenMode.Read", &[]).unwrap();
        fs_error(
            host.invoke("std.fs.open", &[root.clone(), read_mode])
                .unwrap(),
        );
        fs_error(
            host.invoke("std.fs.openDirectory", std::slice::from_ref(&file))
                .unwrap(),
        );

        let directory = ok(host
            .invoke("std.fs.openDirectory", std::slice::from_ref(&root))
            .unwrap());
        let call = host
            .start_async("std.fs.Directory.list", std::slice::from_ref(&directory))
            .unwrap();
        host.cancel_async(call).unwrap();
        let (_, cancelled) = host.wait_async(&[call]).unwrap();
        fs_error(cancelled);
        host.cleanup(&handle).unwrap();
        host.cleanup(&reader).unwrap();
        host.cleanup(&writer).unwrap();
        host.cleanup(&directory).unwrap();
        ok(host.invoke("std.fs.remove", &[root]).unwrap());
    }

    #[test]
    fn math_sqrt_uses_the_nominal_error_boundary() {
        let mut host = BootstrapHost::default();
        assert_eq!(
            host.invoke("std.math.sqrt", &[RuntimeValue::Float(9.0)])
                .unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Float(3.0)))
        );
        assert!(matches!(
            host.invoke("std.math.sqrt", &[RuntimeValue::Float(-1.0)])
                .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::MathError, .. })
        ));
    }

    fn command(host: &mut BootstrapHost, program: &str, arguments: &[&str]) -> RuntimeValue {
        host.invoke(
            "std.process.command",
            &[
                RuntimeValue::String(program.into()),
                RuntimeValue::Array(
                    arguments
                        .iter()
                        .map(|argument| RuntimeValue::String((*argument).into()))
                        .collect(),
                ),
            ],
        )
        .unwrap()
    }

    fn shell(host: &mut BootstrapHost, text: &str) -> RuntimeValue {
        host.invoke("std.process.shell", &[RuntimeValue::String(text.into())])
            .unwrap()
    }

    fn pipe(host: &mut BootstrapHost, left: RuntimeValue, right: RuntimeValue) -> RuntimeValue {
        host.invoke("std.process.pipe", &[left, right]).unwrap()
    }

    fn await_call(host: &mut BootstrapHost, name: &str, receiver: RuntimeValue) -> RuntimeValue {
        let call = host.start_async(name, &[receiver]).unwrap();
        let (completed, value) = host.wait_async(&[call]).unwrap();
        assert_eq!(completed, call);
        value
    }

    fn ok(value: RuntimeValue) -> RuntimeValue {
        let RuntimeValue::ResultOk(value) = value else {
            panic!("expected successful process result");
        };
        *value
    }

    fn json_limits(host: &mut BootstrapHost) -> RuntimeValue {
        host.invoke(
            "intrinsic.json.JsonLimits.construct",
            &[
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(64),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(4096),
                RuntimeValue::Integer(100_000),
                RuntimeValue::Integer(1 << 20),
            ],
        )
        .unwrap()
    }

    fn json_decode_options(host: &mut BootstrapHost) -> RuntimeValue {
        let limits = json_limits(host);
        let duplicate = host
            .invoke("intrinsic.json.JsonDuplicatePolicy.Reject", &[])
            .unwrap();
        let unknown = host
            .invoke("intrinsic.json.JsonUnknownFieldPolicy.Reject", &[])
            .unwrap();
        let numbers = host
            .invoke("intrinsic.json.JsonNumberPolicy.Exact", &[])
            .unwrap();
        host.invoke(
            "intrinsic.json.JsonDecodeOptions.construct",
            &[limits, duplicate, unknown, numbers],
        )
        .unwrap()
    }

    fn json_encode_options(host: &mut BootstrapHost) -> RuntimeValue {
        let limits = json_limits(host);
        host.invoke(
            "intrinsic.json.JsonEncodeOptions.construct",
            &[limits, RuntimeValue::Bool(false)],
        )
        .unwrap()
    }

    fn messagepack_limits(host: &mut BootstrapHost) -> RuntimeValue {
        host.invoke(
            "intrinsic.messagepack.MessagePackLimits.construct",
            &[
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(64),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(100_000),
                RuntimeValue::Integer(1 << 20),
            ],
        )
        .unwrap()
    }

    fn messagepack_decode_options(host: &mut BootstrapHost) -> RuntimeValue {
        let limits = messagepack_limits(host);
        let dynamic = host
            .invoke(
                "intrinsic.messagepack.MessagePackDuplicatePolicy.Preserve",
                &[],
            )
            .unwrap();
        let typed = host
            .invoke(
                "intrinsic.messagepack.MessagePackDuplicatePolicy.Reject",
                &[],
            )
            .unwrap();
        let nonminimal = host
            .invoke(
                "intrinsic.messagepack.MessagePackNonMinimalPolicy.Accept",
                &[],
            )
            .unwrap();
        let unknown = host
            .invoke(
                "intrinsic.messagepack.MessagePackUnknownExtensionPolicy.Preserve",
                &[],
            )
            .unwrap();
        host.invoke(
            "intrinsic.messagepack.MessagePackDecodeOptions.construct",
            &[limits, dynamic, typed, nonminimal, unknown],
        )
        .unwrap()
    }

    fn messagepack_encode_options(host: &mut BootstrapHost, deterministic: bool) -> RuntimeValue {
        let limits = messagepack_limits(host);
        host.invoke(
            "intrinsic.messagepack.MessagePackEncodeOptions.construct",
            &[limits, RuntimeValue::Bool(deterministic)],
        )
        .unwrap()
    }

    fn proto_limits(host: &mut BootstrapHost) -> RuntimeValue {
        host.invoke(
            "intrinsic.protobuf.ProtoLimits.construct",
            &[
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(64),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(64),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 16),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(1 << 20),
                RuntimeValue::Integer(10),
                RuntimeValue::Integer(100_000),
                RuntimeValue::Integer(1 << 20),
            ],
        )
        .unwrap()
    }

    fn proto_decode_options(host: &mut BootstrapHost) -> RuntimeValue {
        let limits = proto_limits(host);
        let wire = host
            .invoke(
                "intrinsic.protobuf.ProtoWireTypePolicy.PreserveUnknown",
                &[],
            )
            .unwrap();
        let unknown = host
            .invoke("intrinsic.protobuf.ProtoUnknownPolicy.Preserve", &[])
            .unwrap();
        host.invoke(
            "intrinsic.protobuf.ProtoDecodeOptions.construct",
            &[limits, wire, unknown, RuntimeValue::Bool(false)],
        )
        .unwrap()
    }

    fn proto_encode_options(host: &mut BootstrapHost, deterministic: bool) -> RuntimeValue {
        let limits = proto_limits(host);
        host.invoke(
            "intrinsic.protobuf.ProtoEncodeOptions.construct",
            &[limits, RuntimeValue::Bool(deterministic)],
        )
        .unwrap()
    }

    fn output_text(host: &mut BootstrapHost, output: RuntimeValue) -> String {
        let bytes = host
            .invoke("std.process.ProcessOutput.stdout", &[output])
            .unwrap();
        let text = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        let RuntimeValue::String(text) = ok(text) else {
            panic!("expected decoded process output");
        };
        text
    }

    fn output_stderr_text(host: &mut BootstrapHost, output: RuntimeValue) -> String {
        let bytes = host
            .invoke("std.process.ProcessOutput.stderr", &[output])
            .unwrap();
        let text = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        let RuntimeValue::String(text) = ok(text) else {
            panic!("expected decoded process stderr");
        };
        text
    }

    fn output_combined_bytes(host: &mut BootstrapHost, output: RuntimeValue) -> Vec<u8> {
        let bytes = host
            .invoke("std.process.ProcessOutput.combined", &[output])
            .unwrap();
        match bytes {
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::Bytes,
                id,
            } => match host.values.remove(&id) {
                Some(HostValue::Bytes(bytes)) => bytes,
                _ => panic!("expected live combined process bytes"),
            },
            _ => panic!("expected combined process bytes"),
        }
    }

    fn bytes_ok(value: RuntimeValue) -> RuntimeValue {
        let RuntimeValue::ResultOk(value) = value else {
            panic!("expected successful bytes result");
        };
        *value
    }

    #[test]
    fn text_owner_builds_scalars_and_rejects_invalid_boundaries_atomically() {
        let mut host = BootstrapHost::default();
        assert_eq!(
            host.invoke("std.text.String.empty", &[]).unwrap(),
            RuntimeValue::String(String::new())
        );
        let rebuilt = host
            .invoke(
                "std.text.String.fromChars",
                &[RuntimeValue::Array(vec![
                    RuntimeValue::Char('a'),
                    RuntimeValue::Char('ñ'),
                    RuntimeValue::Char('🙂'),
                ])],
            )
            .unwrap();
        let rebuilt = ok(rebuilt);
        assert_eq!(rebuilt, RuntimeValue::String("añ🙂".into()));
        assert_eq!(
            host.invoke("std.text.String.chars", std::slice::from_ref(&rebuilt))
                .unwrap(),
            rebuilt
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.text.String.slice",
                    &[
                        rebuilt.clone(),
                        RuntimeValue::Integer(1),
                        RuntimeValue::Integer(3),
                    ],
                )
                .unwrap()),
            RuntimeValue::String("ñ🙂".into())
        );
        for (start, end) in [(-1, 1), (0, 4), (3, 1)] {
            let result = host
                .invoke(
                    "std.text.String.slice",
                    &[
                        rebuilt.clone(),
                        RuntimeValue::Integer(start),
                        RuntimeValue::Integer(end),
                    ],
                )
                .unwrap();
            assert!(matches!(
                result,
                RuntimeValue::ResultErr(value)
                    if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::TextError, .. })
            ));
        }

        let mut limited = BootstrapHost::with_max_bytes(Vec::new(), 2);
        let result = limited
            .invoke(
                "std.text.String.fromChars",
                &[RuntimeValue::Array(vec![RuntimeValue::Char('🙂')])],
            )
            .unwrap();
        assert!(matches!(
            result,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::TextError, .. })
        ));
    }

    #[test]
    fn collection_host_owner_preserves_value_shapes_and_atomic_errors() {
        let mut host = BootstrapHost::default();
        assert_eq!(
            host.invoke("std.collections.Array.new[Int]", &[]).unwrap(),
            RuntimeValue::Array(Vec::new())
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.collections.Array.withCapacity[Int]",
                    &[RuntimeValue::Integer(2)],
                )
                .unwrap()),
            RuntimeValue::Array(Vec::new())
        );
        assert!(matches!(
            host.invoke(
                "std.collections.Array.withCapacity[Int]",
                &[RuntimeValue::Integer(-1)],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::CollectionError, .. })
        ));

        let array = RuntimeValue::Array(vec![
            RuntimeValue::Integer(10),
            RuntimeValue::Integer(20),
            RuntimeValue::Integer(30),
        ]);
        assert_eq!(
            host.invoke(
                "std.collections.Array.length[Int]",
                std::slice::from_ref(&array),
            )
            .unwrap(),
            RuntimeValue::Integer(3)
        );
        assert_eq!(
            host.invoke(
                "std.collections.Array.get[Int]",
                &[array.clone(), RuntimeValue::Integer(-1)],
            )
            .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(30)))
        );
        assert_eq!(
            host.invoke(
                "std.collections.Array.get[Int]",
                &[array.clone(), RuntimeValue::Integer(9)],
            )
            .unwrap(),
            RuntimeValue::OptionNone
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.collections.Array.slice[Int]",
                    &[
                        array.clone(),
                        RuntimeValue::Integer(1),
                        RuntimeValue::Integer(3),
                    ],
                )
                .unwrap()),
            RuntimeValue::Array(vec![RuntimeValue::Integer(20), RuntimeValue::Integer(30),])
        );
        assert!(matches!(
            host.invoke(
                "std.collections.Array.slice[Int]",
                &[
                    array,
                    RuntimeValue::Integer(3),
                    RuntimeValue::Integer(1),
                ],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::CollectionError, .. })
        ));

        let map = RuntimeValue::Map(vec![
            (RuntimeValue::String("one".into()), RuntimeValue::Integer(1)),
            (RuntimeValue::String("two".into()), RuntimeValue::Integer(2)),
        ]);
        assert_eq!(
            host.invoke(
                "std.collections.Map.get[String, Int]",
                &[map.clone(), RuntimeValue::String("two".into())],
            )
            .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(2)))
        );
        assert_eq!(
            host.invoke(
                "std.collections.Map.contains[String, Int]",
                &[map, RuntimeValue::String("missing".into())],
            )
            .unwrap(),
            RuntimeValue::Bool(false)
        );
        let set = RuntimeValue::Set(vec![RuntimeValue::String("tondo".into())]);
        assert_eq!(
            host.invoke(
                "std.collections.Set.contains[String]",
                &[set, RuntimeValue::String("tondo".into())],
            )
            .unwrap(),
            RuntimeValue::Bool(true)
        );
    }

    #[test]
    fn time_provider_is_monotonic_and_duration_arithmetic_is_checked() {
        let mut host = BootstrapHost::default();
        let first = ok(host.invoke("std.time.now", &[]).unwrap());
        let second = ok(host.invoke("std.time.now", &[]).unwrap());
        let (
            RuntimeValue::Host { kind, .. },
            RuntimeValue::Host {
                kind: next_kind, ..
            },
        ) = (first, second)
        else {
            panic!("now must return opaque Instant tokens");
        };
        assert_eq!(kind, RuntimeHostValueKind::Instant);
        assert_eq!(next_kind, RuntimeHostValueKind::Instant);
        let resolution = ok(host.invoke("std.time.resolution", &[]).unwrap());
        assert!(matches!(resolution, RuntimeValue::Integer(value) if value > 0));

        let overflow = host
            .invoke(
                "std.time.Duration.fromSeconds",
                &[RuntimeValue::Integer(i128::from(i64::MAX))],
            )
            .unwrap();
        assert!(matches!(
            overflow,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::DurationError, .. })
        ));
    }

    fn run_time_contract_corpus(host: &mut BootstrapHost, expected_resolution: Option<i128>) {
        let first = ok(host.invoke("std.time.now", &[]).unwrap());
        let second = ok(host.invoke("std.time.now", &[]).unwrap());
        assert!(matches!(
            host.invoke(
                "std.time.Instant.durationSince",
                &[second.clone(), first.clone()],
            )
            .unwrap(),
            RuntimeValue::ResultOk(value)
                if matches!(value.as_ref(), RuntimeValue::Integer(value) if *value >= 0)
        ));
        let resolution = ok(host.invoke("std.time.resolution", &[]).unwrap());
        assert!(matches!(resolution, RuntimeValue::Integer(value) if value > 0));
        if let Some(expected) = expected_resolution {
            assert_eq!(resolution, RuntimeValue::Integer(expected));
        }

        assert_eq!(
            host.invoke(
                "std.time.Duration.fromNanoseconds",
                &[RuntimeValue::Integer(-7)],
            )
            .unwrap(),
            RuntimeValue::Integer(-7)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.fromMicroseconds",
                    &[RuntimeValue::Integer(2)],
                )
                .unwrap()),
            RuntimeValue::Integer(2_000)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.add",
                    &[RuntimeValue::Integer(2), RuntimeValue::Integer(3)],
                )
                .unwrap()),
            RuntimeValue::Integer(5)
        );
        assert!(matches!(
            host.invoke(
                "std.time.Duration.add",
                &[RuntimeValue::Integer(i64::MAX as i128), RuntimeValue::Integer(1)],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::DurationError, .. })
        ));

        let deadline = ok(host
            .invoke("std.time.deadline", &[RuntimeValue::Integer(0)])
            .unwrap());
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Instant.durationSince",
                    &[deadline.clone(), deadline.clone()],
                )
                .unwrap()),
            RuntimeValue::Integer(0)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Instant.isBefore",
                    &[deadline.clone(), deadline.clone()],
                )
                .unwrap()),
            RuntimeValue::Bool(false)
        );

        let sleep = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(0)])
            .unwrap();
        assert!(matches!(
            host.poll_async(sleep).unwrap(),
            Some(RuntimeValue::ResultOk(value))
                if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let timer = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)])
            .unwrap());
        let wait = host.start_async("std.time.Timer.wait", &[timer]).unwrap();
        assert!(matches!(
            host.poll_async(wait).unwrap(),
            Some(RuntimeValue::ResultOk(value))
                if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let timer = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)])
            .unwrap());
        assert_eq!(
            host.invoke("std.time.Timer.cancel", &[timer]).unwrap(),
            RuntimeValue::Unit
        );
    }

    #[test]
    fn identical_time_contract_corpus_passes_on_real_and_virtual_providers() {
        let real_started = StdInstant::now();
        let mut real = BootstrapHost::default();
        run_time_contract_corpus(&mut real, None);
        assert!(
            real_started.elapsed() <= Duration::from_secs(30),
            "the real provider conformance corpus exceeded its operational tolerance"
        );

        let mut virtual_host = BootstrapHost::with_virtual_time(Vec::new(), 10).unwrap();
        run_time_contract_corpus(&mut virtual_host, Some(10));
    }

    #[test]
    fn time_domains_reject_foreign_instants_and_timers_and_tied_deadlines() {
        let mut source = BootstrapHost::with_virtual_time(Vec::new(), 10).unwrap();
        let source_domain = source.clock_domain;
        let mut host = BootstrapHost::with_virtual_time(Vec::new(), 10).unwrap();
        let foreign = host.allocate(
            RuntimeHostValueKind::Instant,
            HostValue::Instant {
                domain: source_domain,
                nanos: 0,
            },
        );
        for name in [
            "std.time.Instant.add",
            "std.time.Instant.subtract",
            "std.time.Instant.durationSince",
            "std.time.Instant.isBefore",
            "std.time.Instant.isAfter",
            "std.time.Timer.at",
        ] {
            let arguments = if name.ends_with("durationSince")
                || name.ends_with("isBefore")
                || name.ends_with("isAfter")
            {
                vec![foreign.clone(), foreign.clone()]
            } else if name.ends_with("Timer.at") {
                vec![foreign.clone()]
            } else {
                vec![foreign.clone(), RuntimeValue::Integer(1)]
            };
            let result = host.invoke(name, &arguments).unwrap();
            assert!(
                matches!(
                    result,
                    RuntimeValue::ResultErr(value)
                        if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
                ),
                "{name} must reject a foreign clock domain"
            );
        }

        let first = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(100)])
            .unwrap());
        let second = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(100)])
            .unwrap());
        let first_wait = host.start_async("std.time.Timer.wait", &[first]).unwrap();
        let second_wait = host.start_async("std.time.Timer.wait", &[second]).unwrap();
        assert!(host.poll_async(first_wait).unwrap().is_none());
        assert!(host.poll_async(second_wait).unwrap().is_none());
        host.advance_virtual_time(100).unwrap();
        let (completed, result) = host.wait_async(&[first_wait, second_wait]).unwrap();
        assert_eq!(completed, first_wait, "tied timers use creation order");
        assert_eq!(result, RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)));
        assert!(host.poll_async(second_wait).unwrap().is_some());
        assert!(source.advance_virtual_time(1).is_ok());
    }

    #[test]
    fn virtual_time_completes_timers_only_after_the_deadline_and_supports_cancel() {
        let mut host = BootstrapHost::with_virtual_time(Vec::new(), 10).unwrap();
        assert!(host.advance_virtual_time(-1).is_err());
        let delay = RuntimeValue::Integer(100);
        let timer = ok(host.invoke("std.time.Timer.after", &[delay]).unwrap());
        let call = host.start_async("std.time.Timer.wait", &[timer]).unwrap();
        assert_eq!(host.poll_async(call).unwrap(), None);
        host.advance_virtual_time(99).unwrap();
        assert_eq!(host.poll_async(call).unwrap(), None);
        host.advance_virtual_time(1).unwrap();
        assert_eq!(
            host.poll_async(call).unwrap(),
            Some(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
        );

        let timer = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(1_000)])
            .unwrap());
        let call = host.start_async("std.time.Timer.wait", &[timer]).unwrap();
        host.cancel_async(call).unwrap();
        assert_eq!(
            host.poll_async(call).unwrap(),
            Some(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
        );
    }

    #[test]
    fn time_rejects_negative_delays_without_starting_a_real_wait() {
        let mut host = BootstrapHost::with_virtual_time(Vec::new(), 1).unwrap();
        let call = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(-1)])
            .unwrap();
        let result = host.poll_async(call).unwrap().expect("immediate failure");
        assert!(matches!(
            result,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
        ));
    }

    #[test]
    fn time_operations_cover_checked_arithmetic_and_virtual_deadlines() {
        let mut host = BootstrapHost::with_virtual_time(Vec::new(), 1).unwrap();
        assert_eq!(
            host.invoke(
                "std.time.Duration.fromNanoseconds",
                &[RuntimeValue::Integer(-4)],
            )
            .unwrap(),
            RuntimeValue::Integer(-4)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.fromMicroseconds",
                    &[RuntimeValue::Integer(2)],
                )
                .unwrap()),
            RuntimeValue::Integer(2_000)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.fromMilliseconds",
                    &[RuntimeValue::Integer(-2)],
                )
                .unwrap()),
            RuntimeValue::Integer(-2_000_000)
        );
        assert_eq!(
            ok(host
                .invoke("std.time.Duration.fromSeconds", &[RuntimeValue::Integer(1)],)
                .unwrap()),
            RuntimeValue::Integer(NANOS_PER_SECOND)
        );
        assert_eq!(
            host.invoke(
                "std.time.Duration.toNanoseconds",
                &[RuntimeValue::Integer(-4)],
            )
            .unwrap(),
            RuntimeValue::Integer(-4)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.add",
                    &[RuntimeValue::Integer(2), RuntimeValue::Integer(3)],
                )
                .unwrap()),
            RuntimeValue::Integer(5)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.subtract",
                    &[RuntimeValue::Integer(2), RuntimeValue::Integer(3)],
                )
                .unwrap()),
            RuntimeValue::Integer(-1)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Duration.multiply",
                    &[RuntimeValue::Integer(2), RuntimeValue::Integer(-3)],
                )
                .unwrap()),
            RuntimeValue::Integer(-6)
        );
        assert_eq!(
            ok(host
                .invoke("std.time.Duration.negate", &[RuntimeValue::Integer(-4)])
                .unwrap()),
            RuntimeValue::Integer(4)
        );
        assert_eq!(
            host.invoke("std.time.Duration.isZero", &[RuntimeValue::Integer(0)],)
                .unwrap(),
            RuntimeValue::Bool(true)
        );
        assert_eq!(
            host.invoke("std.time.Duration.isNegative", &[RuntimeValue::Integer(-1)],)
                .unwrap(),
            RuntimeValue::Bool(true)
        );
        assert_eq!(
            host.invoke(
                "std.time.Duration.isLessThan",
                &[RuntimeValue::Integer(-1), RuntimeValue::Integer(0)],
            )
            .unwrap(),
            RuntimeValue::Bool(true)
        );
        assert!(matches!(
            host.invoke(
                "std.time.Duration.add",
                &[RuntimeValue::Integer(i64::MAX as i128), RuntimeValue::Integer(1)],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::DurationError, .. })
        ));

        let start = ok(host.invoke("std.time.now", &[]).unwrap());
        let past = ok(host
            .invoke("std.time.deadline", &[RuntimeValue::Integer(-1)])
            .unwrap());
        host.advance_virtual_time(10).unwrap();
        let later = ok(host.invoke("std.time.now", &[]).unwrap());
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Instant.durationSince",
                    &[later.clone(), start.clone()],
                )
                .unwrap()),
            RuntimeValue::Integer(10)
        );
        let added = ok(host
            .invoke(
                "std.time.Instant.add",
                &[start.clone(), RuntimeValue::Integer(5)],
            )
            .unwrap());
        assert_eq!(host.instant(&added).unwrap().1, 5);
        let subtracted = ok(host
            .invoke(
                "std.time.Instant.subtract",
                &[later.clone(), RuntimeValue::Integer(3)],
            )
            .unwrap());
        assert_eq!(host.instant(&subtracted).unwrap().1, 7);
        assert_eq!(
            ok(host
                .invoke("std.time.Instant.isBefore", &[start.clone(), later.clone()],)
                .unwrap()),
            RuntimeValue::Bool(true)
        );
        assert_eq!(
            ok(host
                .invoke("std.time.Instant.isAfter", &[later.clone(), start.clone()],)
                .unwrap()),
            RuntimeValue::Bool(true)
        );
        let timer = ok(host.invoke("std.time.Timer.at", &[past]).unwrap());
        let call = host.start_async("std.time.Timer.wait", &[timer]).unwrap();
        assert_eq!(
            host.poll_async(call).unwrap(),
            Some(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
        );
        let timer = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)])
            .unwrap());
        assert_eq!(
            host.invoke("std.time.Timer.cancel", &[timer]).unwrap(),
            RuntimeValue::Unit
        );
        assert!(matches!(
            host.invoke("std.time.Timer.after", &[RuntimeValue::Integer(-1)])
                .unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
        ));
    }

    #[test]
    fn time_base_matches_checked_duration_and_instant_models() {
        let mut host = BootstrapHost::with_virtual_time(Vec::new(), 1).unwrap();
        let duration_values = [INT_MIN, -3, -1, 0, 1, 3, INT_MAX];
        for left in duration_values {
            assert_eq!(
                host.invoke("std.time.Duration.isZero", &[RuntimeValue::Integer(left)])
                    .unwrap(),
                RuntimeValue::Bool(left == 0)
            );
            assert_eq!(
                host.invoke(
                    "std.time.Duration.isNegative",
                    &[RuntimeValue::Integer(left)]
                )
                .unwrap(),
                RuntimeValue::Bool(left < 0)
            );
            let negated = left
                .checked_neg()
                .filter(|value| (INT_MIN..=INT_MAX).contains(value));
            let actual = host
                .invoke("std.time.Duration.negate", &[RuntimeValue::Integer(left)])
                .unwrap();
            match negated {
                Some(expected) => assert_eq!(
                    actual,
                    RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(expected)))
                ),
                None => assert!(matches!(
                    actual,
                    RuntimeValue::ResultErr(value)
                        if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::DurationError, .. })
                )),
            }

            for right in duration_values {
                assert_eq!(
                    host.invoke(
                        "std.time.Duration.isLessThan",
                        &[RuntimeValue::Integer(left), RuntimeValue::Integer(right)]
                    )
                    .unwrap(),
                    RuntimeValue::Bool(left < right)
                );
                for (operation, expected) in [
                    ("std.time.Duration.add", left.checked_add(right)),
                    ("std.time.Duration.subtract", left.checked_sub(right)),
                    ("std.time.Duration.multiply", left.checked_mul(right)),
                ] {
                    let expected = expected.filter(|value| (INT_MIN..=INT_MAX).contains(value));
                    let actual = host
                        .invoke(
                            operation,
                            &[RuntimeValue::Integer(left), RuntimeValue::Integer(right)],
                        )
                        .unwrap();
                    match expected {
                        Some(expected) => assert_eq!(
                            actual,
                            RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(expected))),
                            "{operation}({left}, {right})"
                        ),
                        None => assert!(
                            matches!(
                                actual,
                                RuntimeValue::ResultErr(value)
                                    if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::DurationError, .. })
                            ),
                            "{operation}({left}, {right}) must overflow"
                        ),
                    }
                }
            }
        }

        let instant_values = [i128::MIN, INT_MIN, -1, 0, 1, INT_MAX, i128::MAX];
        for left in instant_values {
            for right in instant_values {
                let left_value = host.allocate_instant(left);
                let right_value = host.allocate_instant(right);
                for (operation, expected) in [
                    ("std.time.Instant.isBefore", left < right),
                    ("std.time.Instant.isAfter", left > right),
                ] {
                    assert_eq!(
                        ok(host
                            .invoke(operation, &[left_value.clone(), right_value.clone()])
                            .unwrap()),
                        RuntimeValue::Bool(expected),
                        "{operation}({left}, {right})"
                    );
                }
                let expected = left
                    .checked_sub(right)
                    .filter(|value| (INT_MIN..=INT_MAX).contains(value));
                let actual = host
                    .invoke("std.time.Instant.durationSince", &[left_value, right_value])
                    .unwrap();
                match expected {
                    Some(expected) => assert_eq!(
                        actual,
                        RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(expected)))
                    ),
                    None => assert!(matches!(
                        actual,
                        RuntimeValue::ResultErr(value)
                            if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
                    )),
                }
            }
        }

        for instant in instant_values {
            for duration in duration_values {
                for (operation, expected) in [
                    ("std.time.Instant.add", instant.checked_add(duration)),
                    ("std.time.Instant.subtract", instant.checked_sub(duration)),
                ] {
                    let receiver = host.allocate_instant(instant);
                    let actual = host
                        .invoke(operation, &[receiver, RuntimeValue::Integer(duration)])
                        .unwrap();
                    match expected {
                        Some(expected) => {
                            let value = ok(actual);
                            assert_eq!(host.instant(&value).unwrap().1, expected);
                        }
                        None => assert!(matches!(
                            actual,
                            RuntimeValue::ResultErr(value)
                                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
                        )),
                    }
                }
            }
        }

        assert!(BootstrapHost::with_virtual_time(Vec::new(), 0).is_err());
        assert!(BootstrapHost::with_virtual_time(Vec::new(), -1).is_err());
        let mut overflowing = BootstrapHost::with_virtual_time(Vec::new(), 1).unwrap();
        overflowing.advance_virtual_time(INT_MAX).unwrap();
        assert!(overflowing.advance_virtual_time(1).is_err());
    }

    #[test]
    fn time_resource_limits_are_atomic_and_released_by_cancel() {
        let mut host = BootstrapHost::with_max_time_resources(Vec::new(), 1);
        let first = ok(host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)])
            .unwrap());
        let second = host
            .invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)])
            .unwrap();
        assert!(matches!(
            second,
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
        ));
        assert_eq!(
            host.invoke("std.time.Timer.cancel", &[first]).unwrap(),
            RuntimeValue::Unit
        );
        assert!(matches!(
            host.invoke("std.time.Timer.after", &[RuntimeValue::Integer(0)],)
                .unwrap(),
            RuntimeValue::ResultOk(_)
        ));

        let mut host = BootstrapHost::with_max_time_resources(Vec::new(), 1);
        let pending = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(1_000_000_000)])
            .unwrap();
        let limited = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(1)])
            .unwrap();
        assert_eq!(host.time_resources, 1);
        assert!(host.time_jobs.get(&limited).unwrap().completion.is_some());
        assert!(matches!(
            host.poll_async(limited).unwrap(),
            Some(RuntimeValue::ResultErr(value))
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::ClockError, .. })
        ));
        host.cancel_async(pending).unwrap();
        assert_eq!(
            host.poll_async(pending).unwrap(),
            Some(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
        );
    }

    #[test]
    fn environment_snapshot_is_sealed_ordered_and_supports_text_and_raw_bytes() {
        let mut host = BootstrapHost::with_environment(
            vec!["program".into(), "á".into()],
            vec![
                (b"TEXT".to_vec(), b"hello".to_vec()),
                (b"RAW".to_vec(), vec![0xff]),
            ],
        );
        let first = ok(host.invoke("std.env.snapshot", &[]).unwrap());
        let second = ok(host.invoke("std.env.snapshot", &[]).unwrap());
        assert_eq!(first, second, "snapshot is sealed once per invocation");

        let arguments = host
            .invoke("std.env.Snapshot.arguments", std::slice::from_ref(&first))
            .unwrap();
        let RuntimeValue::Array(arguments) = arguments else {
            panic!("snapshot arguments must be an Array[Value]");
        };
        let RuntimeValue::Array(second_arguments) = host
            .invoke("std.env.Snapshot.arguments", std::slice::from_ref(&first))
            .unwrap()
        else {
            panic!("snapshot arguments must remain an Array[Value]");
        };
        assert_ne!(
            arguments, second_arguments,
            "each arguments result owns independent value handles"
        );
        assert_eq!(arguments.len(), 2);
        assert_eq!(
            host.invoke("std.env.Value.asText", &[arguments[0].clone()])
                .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::String("program".into())))
        );
        assert_eq!(
            host.invoke("std.env.Value.asText", &[arguments[1].clone()])
                .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::String("á".into())))
        );

        let text_name = ok(host
            .invoke(
                "std.env.Name.fromText",
                &[RuntimeValue::String("TEXT".into())],
            )
            .unwrap());
        let text_value = host
            .invoke("std.env.Snapshot.get", &[first.clone(), text_name])
            .unwrap();
        let RuntimeValue::ResultOk(value) = text_value else {
            panic!("present environment entry must not fail");
        };
        let RuntimeValue::OptionSome(value) = *value else {
            panic!("present environment entry must be Some");
        };
        assert_eq!(
            host.invoke("std.env.Value.asText", &[(*value).clone()])
                .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::String("hello".into())))
        );
        let text_bytes = host
            .invoke("std.env.Value.asBytes", &[(*value).clone()])
            .unwrap();
        let second_text_bytes = host
            .invoke("std.env.Value.asBytes", &[(*value).clone()])
            .unwrap();
        assert_ne!(
            text_bytes, second_text_bytes,
            "byte conversions must not alias mutable host storage"
        );
        assert_eq!(host.bytes(&text_bytes).unwrap(), b"hello");
        assert_eq!(host.bytes(&second_text_bytes).unwrap(), b"hello");

        let raw_name_bytes = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"RAW".to_vec()),
        );
        let raw_name = ok(host
            .invoke("std.env.Name.fromBytes", &[raw_name_bytes])
            .unwrap());
        let raw_value = host
            .invoke("std.env.Snapshot.get", &[first.clone(), raw_name])
            .unwrap();
        let RuntimeValue::ResultOk(value) = raw_value else {
            panic!("raw environment entry must not fail");
        };
        let RuntimeValue::OptionSome(value) = *value else {
            panic!("raw environment entry must be Some");
        };
        assert_eq!(
            host.invoke("std.env.Value.asText", &[(*value).clone()])
                .unwrap(),
            RuntimeValue::OptionNone
        );
        let raw_bytes = host
            .invoke("std.env.Value.asBytes", &[(*value).clone()])
            .unwrap();
        assert_eq!(host.bytes(&raw_bytes).unwrap(), &[0xff]);

        let missing_name = ok(host
            .invoke(
                "std.env.Name.fromText",
                &[RuntimeValue::String("MISSING".into())],
            )
            .unwrap());
        assert_eq!(
            host.invoke("std.env.Snapshot.get", &[first, missing_name])
                .unwrap(),
            RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone))
        );
    }

    #[test]
    fn environment_default_snapshot_is_empty_and_never_reads_ambient_host_state() {
        let mut host = BootstrapHost::default();
        let snapshot = ok(host.invoke("std.env.snapshot", &[]).unwrap());
        assert_eq!(
            host.invoke(
                "std.env.Snapshot.arguments",
                std::slice::from_ref(&snapshot)
            )
            .unwrap(),
            RuntimeValue::Array(Vec::new())
        );

        for ambient_name in ["PATH", "HOME", "TONDO_UNDECLARED"] {
            let name = ok(host
                .invoke(
                    "std.env.Name.fromText",
                    &[RuntimeValue::String(ambient_name.into())],
                )
                .unwrap());
            assert_eq!(
                host.invoke("std.env.Snapshot.get", &[snapshot.clone(), name])
                    .unwrap(),
                RuntimeValue::ResultOk(Box::new(RuntimeValue::OptionNone)),
                "the default adapter must not consult host variable {ambient_name}"
            );
        }
    }

    #[test]
    fn environment_rejects_invalid_names_unavailable_hosts_and_partial_limits() {
        let mut host = BootstrapHost::default();
        for invalid in ["", "A\0B", "A=B"] {
            let result = host
                .invoke(
                    "std.env.Name.fromText",
                    &[RuntimeValue::String(invalid.into())],
                )
                .unwrap();
            assert!(matches!(
                result,
                RuntimeValue::ResultErr(value)
                    if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvError, .. })
            ));
        }
        let invalid_bytes =
            host.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(vec![b'A', 0]));
        assert!(matches!(
            host.invoke("std.env.Name.fromBytes", &[invalid_bytes]).unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvError, .. })
        ));

        let mut unavailable = BootstrapHost::with_unavailable_environment(Vec::new());
        assert!(matches!(
            unavailable.invoke("std.env.snapshot", &[]).unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvError, .. })
        ));

        let mut limited = BootstrapHost::with_environment(
            vec!["program".into()],
            vec![(b"KEY".to_vec(), b"value".to_vec())],
        );
        limited.max_bytes = 2;
        assert!(matches!(
            limited.invoke("std.env.snapshot", &[]).unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvError, .. })
        ));
        assert!(limited.env_snapshot_id.is_none());
        limited.max_bytes = 64;
        assert!(matches!(
            limited.invoke("std.env.snapshot", &[]).unwrap(),
            RuntimeValue::ResultOk(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvSnapshot, .. })
        ));

        let mut invalid_provider = BootstrapHost::with_environment(
            Vec::new(),
            vec![(b"BAD=NAME".to_vec(), b"value".to_vec())],
        );
        assert!(matches!(
            invalid_provider.invoke("std.env.snapshot", &[]).unwrap(),
            RuntimeValue::ResultErr(value)
                if matches!(value.as_ref(), RuntimeValue::Host { kind: RuntimeHostValueKind::EnvError, .. })
        ));
    }

    #[test]
    fn bytes_are_immutable_values_with_copying_boundaries_and_stable_hashes() {
        let mut host = BootstrapHost::default();
        let source = bytes_ok(
            host.invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("abc".into())],
            )
            .unwrap(),
        );
        let copy = bytes_ok(
            host.invoke("std.bytes.Bytes.toArray", std::slice::from_ref(&source))
                .unwrap(),
        );
        assert_eq!(
            copy,
            RuntimeValue::Array(vec![
                RuntimeValue::Byte(b'a'),
                RuntimeValue::Byte(b'b'),
                RuntimeValue::Byte(b'c'),
            ])
        );
        let round_trip = bytes_ok(
            host.invoke("std.bytes.fromArray", std::slice::from_ref(&copy))
                .unwrap(),
        );
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.equal",
                &[source.clone(), round_trip.clone()]
            )
            .unwrap(),
            RuntimeValue::Bool(true)
        ));
        assert_eq!(
            host.invoke("std.bytes.Bytes.hash", std::slice::from_ref(&source))
                .unwrap(),
            host.invoke("std.bytes.Bytes.hash", std::slice::from_ref(&round_trip))
                .unwrap()
        );
        let slice = bytes_ok(
            host.invoke(
                "std.bytes.Bytes.slice",
                &[
                    source.clone(),
                    RuntimeValue::Integer(1),
                    RuntimeValue::Integer(3),
                ],
            )
            .unwrap(),
        );
        assert_eq!(
            ok(host.invoke("intrinsic.String.fromBytes", &[slice]).unwrap()),
            RuntimeValue::String("bc".into())
        );
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.get",
                &[source.clone(), RuntimeValue::Integer(-1)]
            )
            .unwrap(),
            RuntimeValue::OptionNone
        ));
        assert!(matches!(
            host.invoke(
                "std.bytes.Bytes.get",
                &[source.clone(), RuntimeValue::Integer(99)]
            )
            .unwrap(),
            RuntimeValue::OptionNone
        ));
    }

    #[test]
    fn bytes_reject_invalid_utf8_and_ranges_without_partial_values() {
        let mut host = BootstrapHost::default();
        let invalid = bytes_ok(
            host.invoke(
                "std.bytes.fromArray",
                &[RuntimeValue::Array(vec![RuntimeValue::Byte(0xff)])],
            )
            .unwrap(),
        );
        let text = host
            .invoke("intrinsic.String.fromBytes", std::slice::from_ref(&invalid))
            .unwrap();
        assert!(matches!(text, RuntimeValue::ResultErr(value) if matches!(
            *value,
            RuntimeValue::Host { kind: RuntimeHostValueKind::Utf8Error, .. }
        )));
        for (start, end) in [(-1, 0), (2, 1), (0, 4)] {
            let result = host
                .invoke(
                    "std.bytes.Bytes.slice",
                    &[
                        invalid.clone(),
                        RuntimeValue::Integer(start),
                        RuntimeValue::Integer(end),
                    ],
                )
                .unwrap();
            assert!(matches!(result, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            )));
        }
    }

    #[test]
    fn bytes_builder_is_mutable_only_through_its_host_token_and_obeys_limits() {
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), 3);
        let builder = bytes_ok(host.invoke("std.bytes.builder", &[]).unwrap());
        assert!(matches!(
            host.invoke(
                "std.bytes.BytesBuilder.appendArray",
                &[
                    builder.clone(),
                    RuntimeValue::Array(vec![
                        RuntimeValue::Byte(1),
                        RuntimeValue::Byte(2),
                        RuntimeValue::Byte(3),
                    ])
                ]
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if *value == RuntimeValue::Unit
        ));
        let rejected = host
            .invoke(
                "std.bytes.BytesBuilder.appendByte",
                &[builder.clone(), RuntimeValue::Byte(4)],
            )
            .unwrap();
        assert!(
            matches!(rejected, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            ))
        );
        assert_eq!(
            host.invoke(
                "std.bytes.BytesBuilder.length",
                std::slice::from_ref(&builder),
            )
            .unwrap(),
            RuntimeValue::Integer(3)
        );
        let finished = bytes_ok(
            host.invoke("std.bytes.BytesBuilder.finish", &[builder])
                .unwrap(),
        );
        assert!(matches!(
            host.invoke("std.bytes.Bytes.length", &[finished]).unwrap(),
            RuntimeValue::Integer(3)
        ));
        let mut limited = BootstrapHost::with_max_bytes(Vec::new(), 2);
        let rejected = limited
            .invoke(
                "intrinsic.Bytes.fromString",
                &[RuntimeValue::String("abc".into())],
            )
            .unwrap();
        assert!(
            matches!(rejected, RuntimeValue::ResultErr(value) if matches!(
                *value,
                RuntimeValue::Host { kind: RuntimeHostValueKind::BytesError, .. }
            ))
        );
    }

    #[test]
    fn format_builder_is_bounded_and_rejects_append_atomically() {
        let mut host = BootstrapHost::with_max_bytes(Vec::new(), 3);
        let builder = host.invoke("std.format.Builder.new", &[]).unwrap();
        assert!(matches!(
            host.invoke(
                "std.format.Builder.append",
                &[builder.clone(), RuntimeValue::String("ton".into())],
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if *value == RuntimeValue::Unit
        ));
        let rejected = host
            .invoke(
                "std.format.Builder.append",
                &[builder.clone(), RuntimeValue::String("!".into())],
            )
            .unwrap();
        assert!(matches!(
            rejected,
            RuntimeValue::ResultErr(value)
                if matches!(
                    *value,
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::FormatError,
                        ..
                    }
                )
        ));
        let finished = host
            .invoke("std.format.Builder.finish", &[builder])
            .unwrap();
        assert!(matches!(
            finished,
            RuntimeValue::ResultOk(value) if *value == RuntimeValue::String("ton".into())
        ));
    }

    #[test]
    fn format_builder_rejects_invalid_and_stale_receivers() {
        let mut host = BootstrapHost::default();
        for name in ["std.format.Builder.append", "std.format.Builder.finish"] {
            let arguments = if name.ends_with("append") {
                vec![
                    RuntimeValue::String("not-a-builder".into()),
                    RuntimeValue::String("x".into()),
                ]
            } else {
                vec![RuntimeValue::String("not-a-builder".into())]
            };
            assert!(matches!(
                host.invoke(name, &arguments),
                Err(VmError::Host(message)) if message.contains("receiver is invalid")
            ));
        }
        let stale = RuntimeValue::Host {
            kind: RuntimeHostValueKind::FormatBuilder,
            id: u64::MAX,
        };
        assert!(matches!(
            host.invoke(
                "std.format.Builder.append",
                &[stale.clone(), RuntimeValue::String("x".into())],
            ),
            Err(VmError::Host(message)) if message.contains("token is stale")
        ));
        assert!(matches!(
            host.invoke("std.format.Builder.finish", &[stale]),
            Err(VmError::Host(message)) if message.contains("token is stale")
        ));
    }

    #[test]
    fn plan_construction_is_inert_and_exact_arguments_bypass_the_shell() {
        let marker = std::env::temp_dir().join(format!(
            "tondo-inert-plan-{}-{}",
            std::process::id(),
            thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&marker);
        let mut host = BootstrapHost::default();
        let _plan = shell(&mut host, &format!("touch {}", marker.display()));
        assert!(!marker.exists());

        let exact = command(
            &mut host,
            "/usr/bin/printf",
            &["[%s][%s][%s]", "two words", "*", "$HOME"],
        );
        let output = ok(await_call(&mut host, "std.process.Command.output", exact));
        assert_eq!(output_text(&mut host, output), "[two words][*][$HOME]");
        assert!(!marker.exists());
    }

    #[test]
    fn process_output_preserves_separate_streams_and_terminal_combined_bytes() {
        let mut host = BootstrapHost::default();
        let plan = shell(&mut host, "printf out; printf err >&2");
        let output = ok(await_call(&mut host, "std.process.Command.output", plan));
        assert_eq!(output_text(&mut host, output.clone()), "out");
        assert_eq!(output_stderr_text(&mut host, output.clone()), "err");
        let mut combined = output_combined_bytes(&mut host, output);
        combined.sort_unstable();
        let mut expected = b"outerr".to_vec();
        expected.sort_unstable();
        assert_eq!(combined, expected);
    }

    #[test]
    fn merge_stderr_feeds_the_next_stage_like_bash_pipe_ampersand() {
        let mut host = BootstrapHost::default();
        let source = shell(&mut host, "printf out; printf err >&2");
        let merged = host
            .invoke("std.process.Command.mergeStderr", &[source])
            .unwrap();
        let sink = command(&mut host, "/bin/cat", &[]);
        let pipeline = pipe(&mut host, merged, sink);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.output",
            pipeline,
        ));
        assert_eq!(output_text(&mut host, output.clone()), "outerr");
        assert_eq!(output_stderr_text(&mut host, output.clone()), "");
        assert_eq!(output_combined_bytes(&mut host, output), b"outerr");
    }

    #[test]
    fn separate_pipeline_stderr_is_concatenated_by_stage_order() {
        let mut host = BootstrapHost::default();
        for _ in 0..8 {
            let source = shell(&mut host, "printf first >&2; printf data");
            let middle = shell(&mut host, "cat >/dev/null; printf second >&2");
            let sink = shell(&mut host, "cat >/dev/null; printf third >&2");
            let left = pipe(&mut host, source, middle);
            let pipeline = pipe(&mut host, left, sink);
            let output = ok(await_call(
                &mut host,
                "std.process.Pipeline.output",
                pipeline,
            ));
            assert_eq!(output_stderr_text(&mut host, output), "firstsecondthird");
        }
    }

    #[test]
    fn all_four_pipe_shapes_preserve_stage_order() {
        let mut host = BootstrapHost::default();
        for shape in 0..4 {
            let source = command(&mut host, "/usr/bin/printf", &["pipe"]);
            let cat_one = command(&mut host, "/bin/cat", &[]);
            let cat_two = command(&mut host, "/bin/cat", &[]);
            let cat_three = command(&mut host, "/bin/cat", &[]);
            let pipeline = match shape {
                0 => pipe(&mut host, source, cat_one),
                1 => {
                    let right = pipe(&mut host, cat_one, cat_two);
                    pipe(&mut host, source, right)
                }
                2 => {
                    let left = pipe(&mut host, source, cat_one);
                    pipe(&mut host, left, cat_two)
                }
                3 => {
                    let left = pipe(&mut host, source, cat_one);
                    let right = pipe(&mut host, cat_two, cat_three);
                    pipe(&mut host, left, right)
                }
                _ => unreachable!(),
            };
            let output = ok(await_call(
                &mut host,
                "std.process.Pipeline.output",
                pipeline,
            ));
            assert_eq!(output_text(&mut host, output), "pipe");
        }

        let source = shell(&mut host, "printf left >&2; printf pipe");
        let sink = shell(&mut host, "cat >/dev/null; printf right >&2");
        let pipeline = pipe(&mut host, source, sink);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.output",
            pipeline,
        ));
        assert_eq!(output_stderr_text(&mut host, output), "leftright");
    }

    #[test]
    fn kernel_pipes_drain_output_larger_than_their_backpressure_window() {
        let mut host = BootstrapHost::default();
        let producer = shell(&mut host, "head -c 1048576 /dev/zero");
        let consumer = command(&mut host, "/usr/bin/wc", &["-c"]);
        let pipeline = pipe(&mut host, producer, consumer);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.output",
            pipeline,
        ));
        assert_eq!(output_text(&mut host, output).trim(), "1048576");

        let producer = command(&mut host, "/usr/bin/yes", &["bounded"]);
        let consumer = command(&mut host, "/usr/bin/head", &["-n", "1"]);
        let pipeline = pipe(&mut host, producer, consumer);
        let output = ok(await_call(
            &mut host,
            "std.process.Pipeline.check",
            pipeline,
        ));
        assert_eq!(output_text(&mut host, output), "bounded\n");
    }

    #[test]
    fn exit_status_is_data_while_check_and_spawn_failures_are_typed_errors() {
        let mut host = BootstrapHost::default();
        assert!(!check_succeeded(&[ExitStatus {
            code: None,
            success: false,
            downstream_closed_pipe: true,
        }]));

        let exits_seven = shell(&mut host, "exit 7");
        let status = ok(await_call(
            &mut host,
            "std.process.Command.status",
            exits_seven,
        ));
        let RuntimeValue::Array(statuses) = status else {
            panic!("expected status array");
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(
            host.invoke("std.process.ExitStatus.code", &statuses)
                .unwrap(),
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(7)))
        );
        assert_eq!(
            host.invoke("std.process.ExitStatus.success", &statuses)
                .unwrap(),
            RuntimeValue::Bool(false)
        );

        let exits_seven = shell(&mut host, "exit 7");
        let checked = await_call(&mut host, "std.process.Command.check", exits_seven);
        assert!(matches!(
            checked,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessExitError,
                        ..
                    }
                )
        ));

        let succeeds = shell(&mut host, "exit 0");
        let exits_seven = shell(&mut host, "exit 7");
        let pipeline = pipe(&mut host, succeeds, exits_seven);
        let checked = await_call(&mut host, "std.process.Pipeline.check", pipeline);
        assert!(matches!(
            checked,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessExitError,
                        ..
                    }
                )
        ));

        let missing = command(&mut host, "/definitely/missing/tondo-process-test", &[]);
        let failed = await_call(&mut host, "std.process.Command.output", missing);
        assert!(matches!(
            failed,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::ProcessError,
                        ..
                    }
                )
        ));
    }

    #[test]
    fn bytes_to_string_rejects_invalid_utf8_without_replacement() {
        let mut host = BootstrapHost::default();
        let bytes = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0xf0, 0x28, 0x8c, 0x28]),
        );
        let decoded = host.invoke("intrinsic.String.fromBytes", &[bytes]).unwrap();
        assert!(matches!(
            decoded,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Utf8Error,
                        ..
                    }
                )
        ));
    }

    #[test]
    fn hosted_codecs_validate_and_canonicalize_without_partial_success() {
        let mut protobuf_path = protobuf::ProtoPath::root();
        protobuf_path.push(protobuf::ProtoPathSegment::Message("Envelope".into()));
        assert_eq!(protobuf_path.segments().len(), 1);
        assert_eq!(protobuf_path.to_string(), "$.Envelope");
        let mut unknown = protobuf::UnknownFields::default();
        unknown.push(protobuf::UnknownField {
            number: 7,
            wire_type: protobuf::ProtoWireType::Varint,
            tag_bytes: vec![0x38],
            payload_bytes: vec![1],
        });
        assert_eq!(unknown.count(), 1);
        unknown.discardUnknown();
        assert!(unknown.is_empty());

        let mut host = BootstrapHost::default();
        let json_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(br#"{"b":2,"a":1}"#.to_vec()),
        );
        let decode_options = json_decode_options(&mut host);
        assert!(matches!(
            host.invoke("std.json.validate", &[json_input.clone(), decode_options.clone()])
                .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let json_canonical = ok(host
            .invoke(
                "std.json.canonicalize",
                &[json_input, decode_options.clone()],
            )
            .unwrap());
        assert_eq!(host.bytes(&json_canonical).unwrap(), br#"{"a":1,"b":2}"#);

        let messagepack_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0x82, 0xa1, b'b', 0x02, 0xa1, b'a', 0x01]),
        );
        assert!(matches!(
            host.invoke(
                "std.messagepack.validate",
                std::slice::from_ref(&messagepack_input)
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let messagepack_canonical = ok(host
            .invoke(
                "std.messagepack.canonicalize",
                std::slice::from_ref(&messagepack_input),
            )
            .unwrap());
        assert_eq!(
            host.bytes(&messagepack_canonical).unwrap(),
            &[0x82, 0xa1, b'a', 0x01, 0xa1, b'b', 0x02]
        );

        let protobuf_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0x08, 0x01]),
        );
        assert!(matches!(
            host.invoke("std.protobuf.validate", std::slice::from_ref(&protobuf_input))
                .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));

        let invalid_json = host.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(Vec::new()));
        assert!(matches!(
            host.invoke("std.json.validate", &[invalid_json, decode_options])
                .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(2)
        ));

        for (operation, bytes) in [
            ("std.messagepack.validate", vec![0xc1]),
            ("std.protobuf.validate", vec![0x00]),
        ] {
            let invalid = host.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(bytes));
            assert!(matches!(
                host.invoke(operation, std::slice::from_ref(&invalid)).unwrap(),
                RuntimeValue::ResultErr(value) if match (operation, value.as_ref()) {
                    ("std.messagepack.validate", RuntimeValue::Record { name, .. }) if name == "MessagePackError" => true,
                    ("std.protobuf.validate", RuntimeValue::Record { name, .. }) if name == "ProtoError" => true,
                    _ => false,
                }
            ));
        }
    }

    #[test]
    fn codec_public_host_surfaces_execute_all_contract_routes() {
        let mut host = BootstrapHost::with_stdin(vec![0x01]);
        let messagepack_decode = messagepack_decode_options(&mut host);
        let messagepack_encode = messagepack_encode_options(&mut host, false);
        let messagepack_limits = messagepack_limits(&mut host);
        let messagepack_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0x92, 0x01, 0x02]),
        );

        assert!(matches!(
            host.invoke(
                "std.messagepack.validate",
                std::slice::from_ref(&messagepack_input),
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        assert!(matches!(
            host.invoke(
                "std.messagepack.validate",
                &[messagepack_input.clone(), messagepack_decode.clone()],
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let messagepack_value = ok(host
            .invoke(
                "std.messagepack.parse",
                &[messagepack_input.clone(), messagepack_decode.clone()],
            )
            .unwrap());
        assert!(matches!(
            messagepack_value,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::MessagePackValue,
                ..
            }
        ));
        let messagepack_view = ok(host
            .invoke(
                "std.messagepack.parseView",
                &[messagepack_input.clone(), messagepack_decode.clone()],
            )
            .unwrap());
        assert!(matches!(
            messagepack_view,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::MessagePackValueView,
                ..
            }
        ));
        let messagepack_typed_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![0x92, 0xd0, 0x01, 0xd0, 0x02]),
        );
        let decoded = ok(host
            .invoke(
                "std.messagepack.decode[Array[Int]]",
                &[messagepack_typed_input, messagepack_decode.clone()],
            )
            .unwrap());
        assert_eq!(
            decoded,
            RuntimeValue::Array(vec![RuntimeValue::Integer(1), RuntimeValue::Integer(2)])
        );
        let dynamic_encoded = ok(host
            .invoke(
                "std.messagepack.encode",
                &[messagepack_value.clone(), messagepack_encode.clone()],
            )
            .unwrap());
        assert_eq!(host.bytes(&dynamic_encoded).unwrap(), &[0x92, 0x01, 0x02]);
        let typed_encoded = ok(host
            .invoke(
                "std.messagepack.encode[Array[Int]]",
                &[
                    RuntimeValue::Array(vec![RuntimeValue::Integer(1), RuntimeValue::Integer(2)]),
                    messagepack_encode.clone(),
                ],
            )
            .unwrap());
        assert_eq!(host.bytes(&typed_encoded).unwrap(), &[0x92, 0x01, 0x02]);
        let deterministic_encoded = ok(host
            .invoke(
                "std.messagepack.encodeDeterministic",
                &[messagepack_value.clone(), messagepack_limits.clone()],
            )
            .unwrap());
        assert_eq!(
            host.bytes(&deterministic_encoded).unwrap(),
            &[0x92, 0x01, 0x02]
        );
        let raw = ok(host
            .invoke(
                "std.messagepack.raw",
                &[messagepack_input.clone(), messagepack_decode.clone()],
            )
            .unwrap());
        assert!(matches!(
            raw,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::MessagePackRaw,
                ..
            }
        ));
        let unchecked_raw = host
            .invoke(
                "std.messagepack.rawUnchecked",
                std::slice::from_ref(&messagepack_input),
            )
            .unwrap();
        assert!(matches!(
            unchecked_raw,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::MessagePackRaw,
                ..
            }
        ));

        let timestamp_ext = RuntimeValue::Record {
            name: "MessagePackExt".into(),
            values: vec![
                RuntimeValue::Integer(-1),
                host.allocate(
                    RuntimeHostValueKind::Bytes,
                    HostValue::Bytes(vec![0, 0, 0, 7]),
                ),
            ],
        };
        let timestamp = ok(host
            .invoke(
                "std.messagepack.MessagePackTimestamp.fromExt",
                &[timestamp_ext],
            )
            .unwrap());
        assert!(matches!(
            timestamp,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::MessagePackTimestamp,
                ..
            }
        ));
        let round_trip_ext = ok(host
            .invoke(
                "std.messagepack.MessagePackTimestamp.toExt",
                std::slice::from_ref(&timestamp),
            )
            .unwrap());
        assert!(
            matches!(round_trip_ext, RuntimeValue::Record { name, .. } if name == "MessagePackExt")
        );

        let reader = ok(host
            .invoke(
                "std.messagepack.MessagePackReader.fromBytes",
                &[messagepack_input.clone(), messagepack_decode.clone()],
            )
            .unwrap());
        let event = match ok(host
            .invoke(
                "std.messagepack.MessagePackReader.next",
                std::slice::from_ref(&reader),
            )
            .unwrap())
        {
            RuntimeValue::OptionSome(event) => *event,
            other => panic!("expected a MessagePack event, got {other:?}"),
        };
        assert_eq!(
            ok(host
                .invoke(
                    "std.messagepack.MessagePackReader.own",
                    &[reader.clone(), event.clone()],
                )
                .unwrap()),
            event
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.messagepack.MessagePackReader.finish",
                    std::slice::from_ref(&reader),
                )
                .unwrap()),
            RuntimeValue::Unit
        );
        let stdin = ok(host.invoke("std.console.stdin", &[]).unwrap());
        let reader_from_io = ok(host
            .invoke(
                "std.messagepack.MessagePackReader.fromReader",
                &[stdin, messagepack_decode.clone()],
            )
            .unwrap());
        assert_eq!(
            ok(host
                .invoke(
                    "std.messagepack.MessagePackReader.finish",
                    std::slice::from_ref(&reader_from_io),
                )
                .unwrap()),
            RuntimeValue::Unit
        );

        let stdout = ok(host.invoke("std.console.stdout", &[]).unwrap());
        let writer = ok(host
            .invoke(
                "std.messagepack.MessagePackWriter.toWriter",
                &[stdout, messagepack_encode.clone()],
            )
            .unwrap());
        let scalar_event = RuntimeValue::Variant {
            name: "MessagePackEvent".into(),
            variant: 2,
            values: vec![RuntimeValue::Integer(7)],
        };
        assert_eq!(
            ok(host
                .invoke(
                    "std.messagepack.MessagePackWriter.write",
                    &[writer.clone(), scalar_event],
                )
                .unwrap()),
            RuntimeValue::Unit
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.messagepack.MessagePackWriter.finish",
                    std::slice::from_ref(&writer),
                )
                .unwrap()),
            RuntimeValue::Unit
        );
        assert_eq!(host.take_stdout(), vec![7]);

        // Exercise every event conversion arm at the host boundary.  The
        // streaming readers above prove the public routes; these values keep
        // the nominal event bridge complete even for values not produced by a
        // single compact fixture document.
        let messagepack_events = [
            messagepack::MessagePackEvent::Nil,
            messagepack::MessagePackEvent::Bool(true),
            messagepack::MessagePackEvent::Int(-1),
            messagepack::MessagePackEvent::UInt(2),
            messagepack::MessagePackEvent::Float32(1.0_f32.to_bits()),
            messagepack::MessagePackEvent::Float64(2.0_f64.to_bits()),
            messagepack::MessagePackEvent::String("text".into()),
            messagepack::MessagePackEvent::Binary(vec![1, 2]),
            messagepack::MessagePackEvent::StartArray(Some(1)),
            messagepack::MessagePackEvent::EndArray,
            messagepack::MessagePackEvent::StartMap(Some(1)),
            messagepack::MessagePackEvent::MapKey,
            messagepack::MessagePackEvent::EndMap,
            messagepack::MessagePackEvent::Ext(messagepack::MessagePackExt {
                type_code: 3,
                payload: vec![4],
            }),
        ];
        for event in messagepack_events {
            let runtime = host.runtime_messagepack_event(event.clone());
            assert_eq!(host.messagepack_event(&runtime).unwrap(), event);
        }

        let mut proto_host = BootstrapHost::with_stdin(vec![0x08, 0x01]);
        let proto_decode = proto_decode_options(&mut proto_host);
        let proto_encode = proto_encode_options(&mut proto_host, false);
        let proto_limits = proto_limits(&mut proto_host);
        let proto_input = proto_host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(vec![
                0x08, 0x01, // field 1: varint
                0x11, 1, 2, 3, 4, 5, 6, 7, 8, // field 2: fixed64
                0x1a, 1, 0x41, // field 3: length-delimited
                0x25, 1, 2, 3, 4, // field 4: fixed32
            ]),
        );
        assert!(matches!(
            proto_host
                .invoke("std.protobuf.validate", std::slice::from_ref(&proto_input))
                .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        assert!(matches!(
            proto_host
                .invoke(
                    "std.protobuf.validate",
                    &[proto_input.clone(), proto_decode.clone()],
                )
                .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        let decoded = proto_host
            .invoke(
                "std.protobuf.decode[Unit]",
                &[proto_input.clone(), proto_decode.clone()],
            )
            .unwrap();
        assert!(matches!(
            decoded,
            RuntimeValue::ResultOk(_) | RuntimeValue::ResultErr(_)
        ));
        let encoded = proto_host
            .invoke(
                "std.protobuf.encode[Unit]",
                &[RuntimeValue::Unit, proto_encode.clone()],
            )
            .unwrap();
        assert!(matches!(
            encoded,
            RuntimeValue::ResultOk(_) | RuntimeValue::ResultErr(_)
        ));
        let deterministic = proto_host
            .invoke(
                "std.protobuf.encodeDeterministic[Unit]",
                &[RuntimeValue::Unit, proto_limits.clone()],
            )
            .unwrap();
        assert!(matches!(
            deterministic,
            RuntimeValue::ResultOk(_) | RuntimeValue::ResultErr(_)
        ));
        assert!(matches!(
            proto_host
                .invoke("std.protobuf.descriptor[Unit]", &[])
                .unwrap(),
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::ProtoDescriptor,
                ..
            }
        ));

        let reader = ok(proto_host
            .invoke(
                "std.protobuf.ProtoReader.fromBytes",
                &[proto_input.clone(), proto_decode.clone()],
            )
            .unwrap());
        loop {
            let item = ok(proto_host
                .invoke(
                    "std.protobuf.ProtoReader.next",
                    std::slice::from_ref(&reader),
                )
                .unwrap());
            if matches!(item, RuntimeValue::OptionNone) {
                break;
            }
        }
        assert_eq!(
            ok(proto_host
                .invoke(
                    "std.protobuf.ProtoReader.finish",
                    std::slice::from_ref(&reader),
                )
                .unwrap()),
            RuntimeValue::Unit
        );
        let stdin = ok(proto_host.invoke("std.console.stdin", &[]).unwrap());
        let reader_from_io = ok(proto_host
            .invoke(
                "std.protobuf.ProtoReader.fromReader",
                &[stdin, proto_decode.clone()],
            )
            .unwrap());
        assert_eq!(
            ok(proto_host
                .invoke(
                    "std.protobuf.ProtoReader.finish",
                    std::slice::from_ref(&reader_from_io),
                )
                .unwrap()),
            RuntimeValue::Unit
        );

        let stdout = ok(proto_host.invoke("std.console.stdout", &[]).unwrap());
        let writer = ok(proto_host
            .invoke(
                "std.protobuf.ProtoWriter.toWriter",
                &[stdout, proto_encode.clone()],
            )
            .unwrap());
        let wire_varint = RuntimeValue::Variant {
            name: "ProtoWireType".into(),
            variant: 0,
            values: Vec::new(),
        };
        let writer_events = [
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 0,
                values: vec![RuntimeValue::String("root".into())],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 2,
                values: vec![RuntimeValue::Integer(1), wire_varint],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 3,
                values: vec![RuntimeValue::Integer(1)],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 6,
                values: vec![RuntimeValue::Integer(2)],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 7,
                values: vec![
                    proto_host.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(vec![0x41])),
                ],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 8,
                values: Vec::new(),
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 9,
                values: vec![RuntimeValue::Integer(3)],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 7,
                values: vec![
                    proto_host.allocate(RuntimeHostValueKind::Bytes, HostValue::Bytes(vec![1, 2])),
                ],
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 10,
                values: Vec::new(),
            },
            RuntimeValue::Variant {
                name: "ProtoEvent".into(),
                variant: 1,
                values: Vec::new(),
            },
        ];
        for event in writer_events {
            assert_eq!(
                ok(proto_host
                    .invoke("std.protobuf.ProtoWriter.write", &[writer.clone(), event],)
                    .unwrap()),
                RuntimeValue::Unit
            );
        }
        assert_eq!(
            ok(proto_host
                .invoke(
                    "std.protobuf.ProtoWriter.finish",
                    std::slice::from_ref(&writer),
                )
                .unwrap()),
            RuntimeValue::Unit
        );
        assert_eq!(
            proto_host.take_stdout(),
            vec![0x08, 0x01, 0x12, 0x01, 0x41, 0x1a, 0x02, 0x01, 0x02]
        );

        let mut unknown = protobuf::UnknownFields::default();
        unknown.push(protobuf::UnknownField {
            number: 9,
            wire_type: protobuf::ProtoWireType::Varint,
            tag_bytes: vec![0x48],
            payload_bytes: vec![1],
        });
        let unknown = proto_host.allocate(
            RuntimeHostValueKind::UnknownFields,
            HostValue::UnknownFields(unknown),
        );
        assert_eq!(
            proto_host
                .invoke(
                    "std.protobuf.UnknownFields.count",
                    std::slice::from_ref(&unknown)
                )
                .unwrap(),
            RuntimeValue::Integer(1)
        );
        assert_eq!(
            proto_host
                .invoke(
                    "std.protobuf.UnknownFields.discard",
                    std::slice::from_ref(&unknown)
                )
                .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(
            proto_host
                .invoke(
                    "std.protobuf.UnknownFields.count",
                    std::slice::from_ref(&unknown)
                )
                .unwrap(),
            RuntimeValue::Integer(0)
        );

        let proto_events = [
            protobuf::ProtoEvent::StartMessage("root".into()),
            protobuf::ProtoEvent::EndMessage,
            protobuf::ProtoEvent::Field(1, protobuf::ProtoWireType::Varint),
            protobuf::ProtoEvent::Varint(1),
            protobuf::ProtoEvent::Fixed32(2),
            protobuf::ProtoEvent::Fixed64(3),
            protobuf::ProtoEvent::StartLengthDelimited(2),
            protobuf::ProtoEvent::Bytes(vec![4]),
            protobuf::ProtoEvent::EndLengthDelimited,
            protobuf::ProtoEvent::StartPacked(3),
            protobuf::ProtoEvent::EndPacked,
            protobuf::ProtoEvent::Unknown(protobuf::UnknownField {
                number: 4,
                wire_type: protobuf::ProtoWireType::Varint,
                tag_bytes: vec![0x20],
                payload_bytes: vec![1],
            }),
        ];
        for event in proto_events {
            let runtime = proto_host.runtime_proto_event(event.clone());
            if !matches!(event, protobuf::ProtoEvent::Unknown(_)) {
                assert_eq!(proto_host.proto_event(&runtime).unwrap(), event);
            }
        }
    }

    #[test]
    fn json_public_host_surface_is_typed_streaming_and_terminal() {
        let mut host = BootstrapHost::default();
        let decode_options = json_decode_options(&mut host);
        let encode_options = json_encode_options(&mut host);
        let limits = json_limits(&mut host);
        let object = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(br#"{"b":2,"a":1}"#.to_vec()),
        );

        let parsed = ok(host
            .invoke("std.json.parse", &[object.clone(), decode_options.clone()])
            .unwrap());
        assert!(matches!(
            parsed,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::JsonValue,
                ..
            }
        ));
        let view = ok(host
            .invoke(
                "std.json.parseView",
                &[object.clone(), decode_options.clone()],
            )
            .unwrap());
        assert!(matches!(
            view,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::JsonValueView,
                ..
            }
        ));
        let raw = ok(host
            .invoke("std.json.raw", std::slice::from_ref(&object))
            .unwrap());
        assert!(matches!(
            raw,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::JsonRaw,
                ..
            }
        ));
        let unchecked_input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"not-json".to_vec()),
        );
        assert!(matches!(
            host.invoke(
                "std.json.rawUnchecked",
                std::slice::from_ref(&unchecked_input),
            )
            .unwrap(),
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::JsonRaw,
                ..
            }
        ));
        let canonical = ok(host
            .invoke("std.json.encodeCanonical", &[parsed, limits])
            .unwrap());
        assert_eq!(host.bytes(&canonical).unwrap(), br#"{"a":1,"b":2}"#);

        let encoded = ok(host
            .invoke(
                "std.json.encode[Array[Int]]",
                &[
                    RuntimeValue::Array(vec![RuntimeValue::Integer(1), RuntimeValue::Integer(2)]),
                    encode_options.clone(),
                ],
            )
            .unwrap());
        assert_eq!(host.bytes(&encoded).unwrap(), b"[1,2]");
        let decoded = ok(host
            .invoke(
                "std.json.decode[Array[Int]]",
                &[encoded, decode_options.clone()],
            )
            .unwrap());
        assert_eq!(
            decoded,
            RuntimeValue::Array(vec![RuntimeValue::Integer(1), RuntimeValue::Integer(2)])
        );
        for (specialization, source, expected) in [
            ("Bool", b"true".as_slice(), RuntimeValue::Bool(true)),
            (
                "String",
                br#""text""#.as_slice(),
                RuntimeValue::String("text".into()),
            ),
            ("Unit", b"null".as_slice(), RuntimeValue::Unit),
            ("Byte", b"255".as_slice(), RuntimeValue::Byte(255)),
            ("Int16", b"-32768".as_slice(), RuntimeValue::Integer(-32768)),
            (
                "UInt32",
                b"4294967295".as_slice(),
                RuntimeValue::Integer(4_294_967_295),
            ),
            ("Float32", b"1.5".as_slice(), RuntimeValue::Float(1.5)),
            ("Float64", b"2.5".as_slice(), RuntimeValue::Float(2.5)),
            ("Int?", b"null".as_slice(), RuntimeValue::OptionNone),
            (
                "Int?",
                b"7".as_slice(),
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(7))),
            ),
            (
                "Map[String, Int]",
                br#"{"a":1}"#.as_slice(),
                RuntimeValue::Map(vec![(
                    RuntimeValue::String("a".into()),
                    RuntimeValue::Integer(1),
                )]),
            ),
        ] {
            let input = host.allocate(
                RuntimeHostValueKind::Bytes,
                HostValue::Bytes(source.to_vec()),
            );
            assert_eq!(
                ok(host
                    .invoke(
                        &format!("std.json.decode[{specialization}]"),
                        &[input, decode_options.clone()],
                    )
                    .unwrap()),
                expected
            );
        }
        let out_of_range = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"128".to_vec()),
        );
        assert!(matches!(
            host.invoke(
                "std.json.decode[Int8]",
                &[out_of_range, decode_options.clone()],
            )
                .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(10)
        ));
        for (specialization, source) in [
            ("Byte", b"256".as_slice()),
            ("Int", b"1.5".as_slice()),
            ("UInt64", b"-1".as_slice()),
            ("UInt32", b"4294967296".as_slice()),
            ("Float32", b"1e1000".as_slice()),
            ("Float64", b"1e10000".as_slice()),
        ] {
            let input = host.allocate(
                RuntimeHostValueKind::Bytes,
                HostValue::Bytes(source.to_vec()),
            );
            assert!(matches!(
                host.invoke(
                    &format!("std.json.decode[{specialization}]"),
                    &[input, decode_options.clone()],
                )
                .unwrap(),
                RuntimeValue::ResultErr(_)
            ));
        }

        for (specialization, value, expected) in [
            ("Unit", RuntimeValue::Unit, b"null".as_slice()),
            ("Bool", RuntimeValue::Bool(true), b"true".as_slice()),
            ("Float64", RuntimeValue::Float(1.5), b"1.5".as_slice()),
            ("Byte", RuntimeValue::Byte(7), b"7".as_slice()),
            (
                "String",
                RuntimeValue::String("text".into()),
                br#""text""#.as_slice(),
            ),
            (
                "Int?",
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(7))),
                b"7".as_slice(),
            ),
            (
                "Map[String, Int]",
                RuntimeValue::Map(vec![(
                    RuntimeValue::String("a".into()),
                    RuntimeValue::Integer(1),
                )]),
                br#"{"a":1}"#.as_slice(),
            ),
        ] {
            let encoded = ok(host
                .invoke(
                    &format!("std.json.encode[{specialization}]"),
                    &[value, encode_options.clone()],
                )
                .unwrap());
            assert_eq!(host.bytes(&encoded).unwrap(), expected);
        }
        let not_finite = host
            .invoke(
                "std.json.encode[Float64]",
                &[RuntimeValue::Float(f64::NAN), encode_options.clone()],
            )
            .unwrap();
        assert!(matches!(
            not_finite,
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(10)
        ));
        let invalid_map = host
            .invoke(
                "std.json.encode[Map[String, Int]]",
                &[
                    RuntimeValue::Map(vec![(RuntimeValue::Integer(1), RuntimeValue::Integer(2))]),
                    encode_options.clone(),
                ],
            )
            .unwrap();
        assert!(matches!(
            invalid_map,
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(9)
        ));

        let negative_limits = host
            .invoke(
                "intrinsic.json.JsonLimits.construct",
                &[
                    RuntimeValue::Integer(-1),
                    RuntimeValue::Integer(64),
                    RuntimeValue::Integer(65_536),
                    RuntimeValue::Integer(65_536),
                    RuntimeValue::Integer(1_048_576),
                    RuntimeValue::Integer(4_096),
                    RuntimeValue::Integer(100_000),
                    RuntimeValue::Integer(1_048_576),
                ],
            )
            .unwrap();
        let invalid_options = host
            .invoke(
                "intrinsic.json.JsonEncodeOptions.construct",
                &[negative_limits, RuntimeValue::Bool(false)],
            )
            .unwrap();
        assert!(matches!(
            host.invoke(
                "std.json.encode[Unit]",
                &[RuntimeValue::Unit, invalid_options],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(11)
        ));

        let number = ok(host
            .invoke(
                "std.json.JsonNumber.parse",
                &[RuntimeValue::String("42".into())],
            )
            .unwrap());
        assert_eq!(
            host.invoke("std.json.JsonNumber.text", std::slice::from_ref(&number),)
                .unwrap(),
            RuntimeValue::String("42".into())
        );
        assert_eq!(
            ok(host
                .invoke("std.json.JsonNumber.toInt", std::slice::from_ref(&number),)
                .unwrap()),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            ok(host
                .invoke("std.json.JsonNumber.toUInt", std::slice::from_ref(&number),)
                .unwrap()),
            RuntimeValue::Integer(42)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.json.JsonNumber.toFloat32",
                    std::slice::from_ref(&number),
                )
                .unwrap()),
            RuntimeValue::Float(42.0)
        );
        assert_eq!(
            ok(host
                .invoke(
                    "std.json.JsonNumber.toFloat64",
                    std::slice::from_ref(&number),
                )
                .unwrap()),
            RuntimeValue::Float(42.0)
        );

        let scalar = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"42".to_vec()),
        );
        let reader = ok(host
            .invoke(
                "std.json.JsonReader.fromBytes",
                &[scalar, decode_options.clone()],
            )
            .unwrap());
        let next = ok(host
            .invoke("std.json.JsonReader.next", std::slice::from_ref(&reader))
            .unwrap());
        let RuntimeValue::OptionSome(event) = next else {
            panic!("scalar JSON must publish one event")
        };
        assert!(matches!(
            event.as_ref(),
            RuntimeValue::Variant {
                name,
                variant: 7,
                values,
            } if name == "JsonEvent"
                && matches!(values.as_slice(), [RuntimeValue::Host {
                    kind: RuntimeHostValueKind::JsonNumber,
                    ..
                }])
        ));
        let owned = ok(host
            .invoke("std.json.JsonReader.own", &[reader.clone(), *event])
            .unwrap());
        assert!(matches!(
            &owned,
            RuntimeValue::Variant {
                name,
                variant: 7,
                values,
            } if name == "JsonEvent" && values.len() == 1
        ));
        assert!(matches!(
            ok(host
                .invoke("std.json.JsonReader.next", std::slice::from_ref(&reader),)
                .unwrap()),
            RuntimeValue::OptionNone
        ));
        assert!(matches!(
            host.invoke(
                "std.json.JsonReader.finish",
                std::slice::from_ref(&reader),
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        assert!(matches!(
            host.invoke("std.json.JsonReader.next", std::slice::from_ref(&reader))
                .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(1)
        ));

        let stdout = ok(host.invoke("std.console.stdout", &[]).unwrap());
        let wrong_reader = host.allocate(
            RuntimeHostValueKind::Reader,
            HostValue::Reader {
                stream: StreamKind::Stdout,
                offset: 0,
            },
        );
        assert!(matches!(
            host.invoke(
                "std.json.JsonReader.fromReader",
                &[wrong_reader, decode_options.clone()],
            )
            .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(12)
        ));
        let writer = ok(host
            .invoke("std.json.JsonWriter.toWriter", &[stdout, encode_options])
            .unwrap());
        assert!(matches!(
            host.invoke("std.json.JsonWriter.write", &[writer.clone(), owned])
                .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        assert!(matches!(
            host.invoke(
                "std.json.JsonWriter.finish",
                std::slice::from_ref(&writer),
            )
            .unwrap(),
            RuntimeValue::ResultOk(value) if matches!(value.as_ref(), RuntimeValue::Unit)
        ));
        assert_eq!(host.take_stdout(), b"42");
        assert!(matches!(
            host.invoke(
                "std.json.JsonWriter.finish",
                std::slice::from_ref(&writer),
            )
            .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(1)
        ));

        let invalid_number = host
            .invoke(
                "std.json.JsonNumber.parse",
                &[RuntimeValue::String("01".into())],
            )
            .unwrap();
        assert!(matches!(
            invalid_number,
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(5)
        ));
    }

    #[test]
    fn json_reader_from_reader_consumes_only_the_remaining_stream() {
        let mut host = BootstrapHost::with_stdin(b"true".to_vec());
        let reader = ok(host.invoke("std.console.stdin", &[]).unwrap());
        let options = json_decode_options(&mut host);
        let json_reader = ok(host
            .invoke("std.json.JsonReader.fromReader", &[reader.clone(), options])
            .unwrap());
        let event = ok(host
            .invoke("std.json.JsonReader.next", &[json_reader])
            .unwrap());
        assert!(matches!(event, RuntimeValue::OptionSome(_)));
        assert!(matches!(
            ok(host
                .invoke("std.io.Reader.read", &[reader, RuntimeValue::Integer(1)])
                .unwrap()),
            RuntimeValue::OptionNone
        ));
    }

    #[test]
    fn json_serialization_peek_is_non_consuming_and_tracks_terminal_input() {
        let mut host = BootstrapHost::default();
        let input = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"[null]".to_vec()),
        );
        let options = json_decode_options(&mut host);
        let reader = ok(host
            .invoke(
                "intrinsic.json.JsonReader.fromBytesSerialization",
                &[input, options],
            )
            .unwrap());

        for _ in 0..2 {
            let peeked = ok(host
                .invoke(
                    "intrinsic.json.JsonReader.peekSerialization",
                    std::slice::from_ref(&reader),
                )
                .unwrap());
            let RuntimeValue::OptionSome(peeked) = peeked else {
                panic!("serialization peek must expose the next event")
            };
            let consumed = ok(host
                .invoke(
                    "intrinsic.json.JsonReader.nextSerialization",
                    std::slice::from_ref(&reader),
                )
                .unwrap());
            assert_eq!(*peeked, consumed);
        }

        let closing = ok(host
            .invoke(
                "intrinsic.json.JsonReader.nextSerialization",
                std::slice::from_ref(&reader),
            )
            .unwrap());
        assert!(matches!(
            closing,
            RuntimeValue::Variant {
                name,
                variant: 9,
                values,
            } if name == "SerializationEvent" && values.is_empty()
        ));
        assert!(matches!(
            host.invoke(
                "intrinsic.json.JsonReader.nextSerialization",
                std::slice::from_ref(&reader),
            )
            .unwrap(),
            RuntimeValue::ResultErr(value) if runtime_json_error_kind(&value) == Some(1)
        ));
    }

    #[test]
    fn testing_float_tolerance_is_validated_once_and_used_by_float_widths() {
        let envelope = EnvelopeHandle::new(
            "float-tolerance",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let mut host = BootstrapHost::default();
        host.install_testing_envelope(envelope.clone());
        let tolerance = host
            .invoke(
                "std.testing.FloatTolerance.from",
                &[RuntimeValue::Float(0.01), RuntimeValue::Float(0.1)],
            )
            .unwrap();
        let RuntimeValue::ResultOk(value) = tolerance else {
            panic!("valid tolerance must return Ok")
        };
        assert!(matches!(
            value.as_ref(),
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::FloatTolerance,
                ..
            }
        ));
        host.invoke(
            "std.testing.assertFloatNear",
            &[
                RuntimeValue::Float(10.0),
                RuntimeValue::Float(10.5),
                (*value).clone(),
            ],
        )
        .unwrap();
        host.invoke(
            "std.testing.assertFloat32Near",
            &[
                RuntimeValue::Float(10.0),
                RuntimeValue::Float(10.5),
                (*value).clone(),
            ],
        )
        .unwrap();
        let invalid = host
            .invoke(
                "std.testing.FloatTolerance.from",
                &[RuntimeValue::Float(-1.0), RuntimeValue::Float(0.0)],
            )
            .unwrap();
        assert!(matches!(
            invalid,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::FloatToleranceError,
                        ..
                    }
                )
        ));
        assert!(envelope.report().unwrap().terminal().is_none());
    }

    #[test]
    fn testing_text_diff_is_bounded_and_rendered_without_host_paths() {
        let mut host = BootstrapHost::default();
        let diff = host
            .invoke(
                "std.testing.diffText",
                &[
                    RuntimeValue::String("old\n".into()),
                    RuntimeValue::String("new\n".into()),
                ],
            )
            .unwrap();
        assert!(matches!(
            &diff,
            RuntimeValue::Host {
                kind: RuntimeHostValueKind::TextDiff,
                ..
            }
        ));
        assert_eq!(
            host.invoke("std.testing.TextDiff.render", &[diff]).unwrap(),
            RuntimeValue::String("--- expected\n+++ actual\n-old\n+new\n".into())
        );
    }

    #[test]
    fn testing_temp_directory_is_prefix_validated_and_cleanup_is_bounded() {
        let mut host = BootstrapHost::default();
        let invalid = host
            .invoke(
                "std.testing.tempDirectory",
                &[RuntimeValue::String("bad/prefix".into())],
            )
            .unwrap();
        assert!(matches!(
            invalid,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::TempError,
                        ..
                    }
                )
        ));
        let directory = match host
            .invoke(
                "std.testing.tempDirectory",
                &[RuntimeValue::String("wave5".into())],
            )
            .unwrap()
        {
            RuntimeValue::ResultOk(value) => *value,
            other => panic!("unexpected temporary directory result: {other:?}"),
        };
        let path = host
            .invoke(
                "std.testing.TempDirectory.path",
                std::slice::from_ref(&directory),
            )
            .unwrap();
        let physical = host.filesystem_path(&path).unwrap();
        std::fs::write(physical.join("payload"), b"bounded").unwrap();
        assert!(physical.exists());
        host.invoke("std.testing.TempDirectory.cleanup", &[directory])
            .unwrap();
        assert!(!physical.exists());
    }

    #[test]
    fn testing_generator_is_replayable_and_returns_typed_bounds_errors() {
        let mut host = BootstrapHost::default();
        let make = |host: &mut BootstrapHost| {
            let result = host
                .invoke(
                    "std.testing.Generator.forCase",
                    &[RuntimeValue::Integer(7), RuntimeValue::Integer(3)],
                )
                .unwrap();
            assert!(matches!(
                result,
                RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Generator,
                    ..
                }
            ));
            result
        };
        let left = make(&mut host);
        let right = make(&mut host);
        let left_id = host
            .invoke("std.testing.Generator.id", std::slice::from_ref(&left))
            .unwrap();
        let right_id = host
            .invoke("std.testing.Generator.id", std::slice::from_ref(&right))
            .unwrap();
        let (RuntimeValue::Host { id: left_id, .. }, RuntimeValue::Host { id: right_id, .. }) =
            (&left_id, &right_id)
        else {
            panic!("generator id must be an opaque host value");
        };
        let Some(HostValue::GenerationId {
            seed: left_seed,
            case_index: left_case,
        }) = host.values.get(left_id)
        else {
            panic!("left generator id payload is missing");
        };
        let Some(HostValue::GenerationId {
            seed: right_seed,
            case_index: right_case,
        }) = host.values.get(right_id)
        else {
            panic!("right generator id payload is missing");
        };
        assert_eq!((left_seed, left_case), (right_seed, right_case));
        let left_first = host
            .invoke(
                "std.testing.Generator.nextUInt",
                std::slice::from_ref(&left),
            )
            .unwrap();
        let right_first = host
            .invoke(
                "std.testing.Generator.nextUInt",
                std::slice::from_ref(&right),
            )
            .unwrap();
        assert_eq!(left_first, right_first);
        let invalid = host
            .invoke(
                "std.testing.Generator.nextInt",
                &[
                    left.clone(),
                    RuntimeValue::Integer(4),
                    RuntimeValue::Integer(3),
                ],
            )
            .unwrap();
        assert!(matches!(
            invalid,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::GenerationError,
                        ..
                    }
                )
        ));
        let too_long = host
            .invoke(
                "std.testing.Generator.nextBytes",
                &[left, RuntimeValue::Integer(i128::MAX)],
            )
            .unwrap();
        assert!(matches!(
            too_long,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::GenerationError,
                        ..
                    }
                )
        ));
    }

    #[test]
    fn testing_shrink_is_deterministic_bounded_and_atomic() {
        let mut host = BootstrapHost::default();
        let first = host
            .invoke("std.testing.shrink", &[RuntimeValue::Integer(-13)])
            .unwrap();
        let second = host
            .invoke("std.testing.shrink", &[RuntimeValue::Integer(-13)])
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Array(vec![
                RuntimeValue::Integer(-6),
                RuntimeValue::Integer(-3),
                RuntimeValue::Integer(-1),
                RuntimeValue::Integer(0),
            ])))
        );

        let nested = RuntimeValue::Array(vec![
            RuntimeValue::Integer(8),
            RuntimeValue::String("ab".into()),
        ]);
        let RuntimeValue::ResultOk(candidates) = host
            .invoke("std.testing.shrink", std::slice::from_ref(&nested))
            .unwrap()
        else {
            panic!("shrink must return Ok for a built-in array")
        };
        let RuntimeValue::Array(candidates) = candidates.as_ref() else {
            panic!("shrink must return an Array payload")
        };
        assert!(!candidates.is_empty());
        assert!(candidates.len() <= MAX_SHRINK_CANDIDATES);
        assert!(shrink_runtime_value(&nested, MAX_SHRINK_CANDIDATES + 1, 0).is_err());

        let unsupported = host
            .invoke("std.testing.shrink", &[RuntimeValue::Map(Vec::new())])
            .unwrap();
        assert!(matches!(
            unsupported,
            RuntimeValue::ResultErr(value)
                if matches!(
                    value.as_ref(),
                    RuntimeValue::Host {
                        kind: RuntimeHostValueKind::GenerationError,
                        ..
                    }
                )
        ));
        assert!(
            shrink_runtime_value(&RuntimeValue::Float(f64::NAN), 16, 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn testing_host_records_typed_evidence_in_the_installed_envelope() {
        let envelope = EnvelopeHandle::new(
            "hosted-test",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .with_expected_snapshots(BTreeMap::from([("golden".into(), "value".into())]))
            .unwrap();
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let mut host = BootstrapHost::default();
        host.install_testing_envelope(envelope.clone());
        let bytes = host.allocate(
            RuntimeHostValueKind::Bytes,
            HostValue::Bytes(b"trace".to_vec()),
        );

        assert_eq!(
            host.invoke(
                "std.testing.log",
                &[RuntimeValue::String("from Tondo".into())]
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(
            host.invoke(
                "std.testing.assertTextEqual",
                &[
                    RuntimeValue::String("same\n".into()),
                    RuntimeValue::String("same\n".into()),
                ],
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        let tolerance = match host
            .invoke(
                "std.testing.FloatTolerance.from",
                &[RuntimeValue::Float(0.01), RuntimeValue::Float(0.1)],
            )
            .unwrap()
        {
            RuntimeValue::ResultOk(value) => *value,
            other => panic!("unexpected tolerance result: {other:?}"),
        };
        assert_eq!(
            host.invoke(
                "std.testing.assertFloatNear",
                &[
                    RuntimeValue::Float(10.0),
                    RuntimeValue::Float(10.5),
                    tolerance,
                ],
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        host.invoke(
            "std.testing.tags",
            &[RuntimeValue::Map(vec![(
                RuntimeValue::String("kind".into()),
                RuntimeValue::String("integration".into()),
            )])],
        )
        .unwrap();
        host.invoke(
            "std.testing.attach",
            &[
                RuntimeValue::String("trace".into()),
                RuntimeValue::String("text/plain".into()),
                bytes,
            ],
        )
        .unwrap();
        host.invoke(
            "std.testing.snapshot",
            &[
                RuntimeValue::String("golden".into()),
                RuntimeValue::String("value".into()),
            ],
        )
        .unwrap();

        envelope.close().unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.logs()[0].message(), "from Tondo");
        assert_eq!(
            report.tags().get("kind").map(String::as_str),
            Some("integration")
        );
        assert_eq!(report.artifacts()[0].bytes(), b"trace");
        assert!(matches!(
            report.snapshots()[0].outcome(),
            crate::test_control::SnapshotOutcome::Matched { .. }
        ));

        assert!(matches!(
            BootstrapHost::default()
                .invoke("std.testing.log", &[RuntimeValue::String("outside".into())]),
            Err(VmError::Host(_))
        ));
    }

    #[test]
    fn testing_host_assertions_cover_structural_values_and_terminal_failures() {
        let envelope = EnvelopeHandle::new(
            "assertions",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let mut host = BootstrapHost::default();
        host.install_testing_envelope(envelope.clone());

        assert_eq!(
            host.invoke(
                "std.testing.assertEqual[Int]",
                &[RuntimeValue::Integer(7), RuntimeValue::Integer(7)],
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(
            host.invoke(
                "std.testing.assertNotEqual[Int]",
                &[RuntimeValue::Integer(7), RuntimeValue::Integer(8)],
            )
            .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(
            host.invoke(
                "std.testing.assertSome[Int]",
                &[RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(9)))],
            )
            .unwrap(),
            RuntimeValue::Integer(9)
        );
        assert_eq!(
            host.invoke("std.testing.assertNone[Int]", &[RuntimeValue::OptionNone])
                .unwrap(),
            RuntimeValue::Unit
        );
        assert_eq!(
            host.invoke(
                "std.testing.assertOk[Int, String]",
                &[RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(11)))],
            )
            .unwrap(),
            RuntimeValue::Integer(11)
        );
        assert_eq!(
            host.invoke(
                "std.testing.assertErr[Int, String]",
                &[RuntimeValue::ResultErr(Box::new(RuntimeValue::String(
                    "bad".into(),
                )))],
            )
            .unwrap(),
            RuntimeValue::String("bad".into())
        );

        host.invoke(
            "std.testing.assertEqual[Int]",
            &[RuntimeValue::Integer(1), RuntimeValue::Integer(2)],
        )
        .unwrap();
        envelope.close().unwrap();
        let report = envelope.report().unwrap();
        assert!(matches!(
            report.terminal(),
            Some(crate::test_control::Terminal::FailNow { .. })
        ));
    }

    #[test]
    fn testing_host_virtual_time_is_lexical_sequential_and_reports_advances() {
        let envelope = EnvelopeHandle::new(
            "virtual-host",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let mut host = BootstrapHost::default();
        host.install_testing_envelope(envelope.clone());

        assert!(matches!(
            host.start_async("std.testing.VirtualTime.settle", &[]),
            Err(VmError::Host(message)) if message.contains("invalid argument list")
        ));
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.advance",
                &[RuntimeValue::Unit],
            ),
            Err(VmError::Host(message)) if message.contains("invalid argument list")
        ));
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.settle",
                &[RuntimeValue::Unit],
            ),
            Err(VmError::Host(message)) if message.contains("receiver is invalid")
        ));

        let production = ok(host.invoke("std.time.now", &[]).unwrap());
        let controller = host.begin_virtual_time().unwrap();
        assert!(matches!(
            host.begin_virtual_time(),
            Err(VmError::Host(message)) if message.starts_with("P2004:")
        ));
        let virtual_start = ok(host.invoke("std.time.now", &[]).unwrap());
        let sleep = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(10)])
            .unwrap();
        let settle = host
            .start_async(
                "std.testing.VirtualTime.settle",
                std::slice::from_ref(&controller),
            )
            .unwrap();
        let (completed, _) = host.wait_async(&[settle, sleep]).unwrap();
        assert_eq!(completed, sleep);
        assert_eq!(host.wait_async(&[settle]).unwrap().0, settle);
        let virtual_end = ok(host.invoke("std.time.now", &[]).unwrap());
        assert_eq!(
            ok(host
                .invoke(
                    "std.time.Instant.durationSince",
                    &[virtual_end, virtual_start],
                )
                .unwrap()),
            RuntimeValue::Integer(10)
        );
        host.finish_virtual_time(&controller).unwrap();
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.settle",
                std::slice::from_ref(&controller)
            ),
            Err(VmError::Host(message)) if message.contains("stale")
        ));
        let restored = ok(host.invoke("std.time.now", &[]).unwrap());
        assert!(matches!(
            host.invoke("std.time.Instant.durationSince", &[restored, production],)
                .unwrap(),
            RuntimeValue::ResultOk(_)
        ));

        let second = host.begin_virtual_time().unwrap();
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.advance",
                &[second.clone(), RuntimeValue::Integer(-1)]
            ),
            Err(VmError::Host(message)) if message.starts_with("P2005:")
        ));
        let maximal = host
            .start_async(
                "std.testing.VirtualTime.advance",
                &[second.clone(), RuntimeValue::Integer(INT_MAX)],
            )
            .unwrap();
        assert_eq!(host.poll_async(maximal).unwrap(), Some(RuntimeValue::Unit));
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.advance",
                &[second.clone(), RuntimeValue::Integer(1)]
            ),
            Err(VmError::Host(message)) if message.starts_with("P2005:")
        ));
        host.finish_virtual_time(&second).unwrap();

        let third = host.begin_virtual_time().unwrap();
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.advance",
                &[third.clone(), RuntimeValue::String("invalid".into())],
            ),
            Err(VmError::Host(message)) if message.contains("represented by an Int")
        ));
        assert!(matches!(
            host.start_async(
                "std.testing.VirtualTime.advance",
                &[third.clone(), RuntimeValue::Integer(INT_MAX + 1)],
            ),
            Err(VmError::Host(message)) if message.contains("outside the Int domain")
        ));
        let pending = host
            .start_async("std.time.sleep", &[RuntimeValue::Integer(1)])
            .unwrap();
        assert!(matches!(
            host.finish_virtual_time(&third),
            Err(VmError::Host(message)) if message.starts_with("P2003:")
        ));
        host.cancel_async(pending).unwrap();
        assert!(host.poll_async(pending).unwrap().is_some());

        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time().len(), 3);
        assert_eq!(report.virtual_time()[0].index(), 1);
        assert_eq!(report.virtual_time()[0].elapsed_ns(), 10);
        assert_eq!(report.virtual_time()[0].automatic_advances(), 1);
        assert_eq!(report.virtual_time()[0].settles(), 1);
        assert_eq!(report.virtual_time()[1].index(), 2);
        assert_eq!(report.virtual_time()[1].elapsed_ns(), INT_MAX);
        assert_eq!(report.virtual_time()[1].advances(), 1);
        assert_eq!(report.virtual_time()[2].index(), 3);
        assert_eq!(report.virtual_time()[2].elapsed_ns(), 0);
    }

    #[test]
    fn testing_host_virtual_settle_rejects_external_wait_without_sleeping() {
        let envelope = EnvelopeHandle::new(
            "virtual-external",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let mut host = BootstrapHost::default();
        host.install_testing_envelope(envelope.clone());
        let (sender, receiver) = mpsc::channel();
        let external = host.next_job_id().unwrap();
        host.jobs.insert(
            external,
            AsyncJob {
                receiver,
                cancellation: Arc::new(AtomicBool::new(false)),
                worker: None,
                mode: CompletionMode::Output,
            },
        );

        let controller = host.begin_virtual_time().unwrap();
        let settle = host
            .start_async(
                "std.testing.VirtualTime.settle",
                std::slice::from_ref(&controller),
            )
            .unwrap();
        assert!(matches!(
            host.wait_async(&[settle]),
            Err(VmError::Host(message)) if message.starts_with("P2003:")
        ));

        host.jobs.remove(&external);
        drop(sender);
        host.cancel_async(settle).unwrap();
        assert_eq!(host.poll_async(settle).unwrap(), Some(RuntimeValue::Unit));
        host.finish_virtual_time(&controller).unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time()[0].settles(), 0);
    }

    #[test]
    fn cancel_and_host_drop_reap_started_children() {
        let mut host = BootstrapHost::default();
        let plan = shell(&mut host, "exec sleep 30");
        let start = host
            .start_async("std.process.Command.start", &[plan])
            .unwrap();
        let (_, handle) = host.wait_async(&[start]).unwrap();
        let handle = ok(handle);
        let started = Instant::now();
        let _ = ok(await_call(
            &mut host,
            "std.process.ProcessHandle.cancel",
            handle,
        ));
        assert!(started.elapsed() < Duration::from_secs(5));

        let plan = shell(&mut host, "exec sleep 30");
        let handle = ok(host.invoke("std.process.Command.start", &[plan]).unwrap());
        let RuntimeValue::Host { id, .. } = handle else {
            panic!("expected process handle");
        };
        let pid = match host.values.get(&id) {
            Some(HostValue::ProcessHandle(group)) => group.children[0].id(),
            _ => panic!("expected live process group"),
        };
        drop(host);
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}
