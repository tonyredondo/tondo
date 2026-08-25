//! Deterministic, payload-free logical diagnostic dumps.
//!
//! The VM owns the logical trace, while the dump writer owns the stable
//! envelope consumed by the offline CLI analyzer.  This module intentionally
//! projects metadata instead of serializing `RuntimeValue` or heap payloads.
//! Native signal capture and platform unwind records remain adapters for the
//! native diagnostic block.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::bytecode::BytecodeSpan;

use super::diagnostics::{
    DiagnosticEvent, DiagnosticHeapOperation, DiagnosticResourceState,
    DiagnosticSchedulerOperation, DiagnosticSource, DiagnosticTaskState, DiagnosticThreadState,
    DiagnosticTrace,
};

pub const DUMP_SCHEMA: &str = "tondo-dump/1";
pub const DUMP_EXTENSION: &str = ".tdump";
pub const MAX_DUMP_BYTES: usize = 268_435_456;

const REQUIRED_SECTIONS: [&str; 9] = [
    "header",
    "termination",
    "identity",
    "stacks",
    "heap_summary",
    "resource_ledger",
    "scheduler_tail",
    "redaction",
    "limitations",
];
const OPTIONAL_SECTIONS: [&str; 3] = ["registers", "source_maps", "retainers"];

/// Identity attached to one diagnostic attempt.  All fields are logical and
/// intentionally do not contain host paths or user payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpIdentity {
    pub run_id: String,
    pub attempt_id: String,
    pub shard: String,
    pub profile: String,
    pub target: String,
    pub backend: String,
    pub toolchain: String,
    pub source_revision: String,
}

impl DumpIdentity {
    fn validate(&self) -> Result<(), DumpError> {
        let fields = [
            ("run_id", &self.run_id),
            ("attempt_id", &self.attempt_id),
            ("shard", &self.shard),
            ("profile", &self.profile),
            ("target", &self.target),
            ("backend", &self.backend),
            ("toolchain", &self.toolchain),
            ("source_revision", &self.source_revision),
        ];
        if let Some((name, _)) = fields.into_iter().find(|(_, value)| value.is_empty()) {
            return Err(DumpError::InvalidSection(format!(
                "identity field `{name}` must not be empty"
            )));
        }
        Ok(())
    }
}

/// Why the process stopped, without embedding panic messages or other
/// payload-bearing strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpTermination {
    pub reason: String,
    pub program_exit_status: Option<i32>,
    pub command_exit_status: Option<i32>,
}

impl DumpTermination {
    fn validate(&self) -> Result<(), DumpError> {
        const REASONS: [&str; 7] = [
            "panic",
            "fatal-signal",
            "abort",
            "returned",
            "cancelled",
            "timeout",
            "resource-limit",
        ];
        if !REASONS.contains(&self.reason.as_str()) {
            return Err(DumpError::InvalidSection(
                "termination reason must be a stable classification".into(),
            ));
        }
        Ok(())
    }
}

/// Writer limits for one dump.  The default is the contract's 256 MiB cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpOptions {
    pub max_dump_bytes: usize,
    pub include_source_maps: bool,
    pub include_retainers: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            max_dump_bytes: MAX_DUMP_BYTES,
            include_source_maps: true,
            include_retainers: true,
        }
    }
}

impl DumpOptions {
    fn validate(self) -> Result<Self, DumpError> {
        if self.max_dump_bytes == 0 || self.max_dump_bytes > MAX_DUMP_BYTES {
            return Err(DumpError::InvalidLimit {
                limit: self.max_dump_bytes,
            });
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DumpSection {
    pub name: String,
    pub value: Value,
}

/// A versioned logical dump.  It is encoded as canonical UTF-8 JSON so the
/// bootstrap toolchain remains dependency-light while still providing framed,
/// hash-checked sections and deterministic analyzer output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DumpArtifact {
    pub format: String,
    pub version: u8,
    pub content_sha256: String,
    pub sections: Vec<DumpSection>,
}

impl DumpArtifact {
    /// Projects a runtime trace into a payload-free dump with the default
    /// limits and optional metadata sections enabled.
    pub fn from_trace(
        trace: &DiagnosticTrace,
        identity: DumpIdentity,
        termination: DumpTermination,
    ) -> Result<Self, DumpError> {
        Self::from_trace_with_options(trace, identity, termination, DumpOptions::default())
    }

    pub fn from_trace_with_options(
        trace: &DiagnosticTrace,
        identity: DumpIdentity,
        termination: DumpTermination,
        options: DumpOptions,
    ) -> Result<Self, DumpError> {
        options.validate()?;
        if trace.format != super::diagnostics::DIAGNOSTIC_SCHEMA {
            return Err(DumpError::InvalidTrace(trace.format.to_owned()));
        }
        identity.validate()?;
        termination.validate()?;

        let mut sections = Vec::with_capacity(REQUIRED_SECTIONS.len() + 2);
        sections.push(section(
            "header",
            object([
                ("format", Value::String(DUMP_SCHEMA.into())),
                ("version", Value::from(1_u8)),
                ("content_address", Value::String("sha256".into())),
                ("user_payloads", Value::String("omitted-by-default".into())),
            ]),
        ));
        sections.push(section(
            "termination",
            serde_json::to_value(&termination).expect("termination is serializable"),
        ));
        sections.push(section(
            "identity",
            serde_json::to_value(&identity).expect("identity is serializable"),
        ));
        sections.push(section("stacks", stack_section(trace)));
        sections.push(section("heap_summary", heap_summary_section(trace)));
        sections.push(section("resource_ledger", resource_ledger_section(trace)));
        sections.push(section("scheduler_tail", scheduler_section(trace)));
        sections.push(section(
            "redaction",
            object([
                ("payloads", Value::String("omitted-by-default".into())),
                ("secrets", Value::String("never-emitted-by-default".into())),
                ("paths", Value::String("logical-only".into())),
                ("network_upload", Value::Bool(false)),
                ("executes_dump_code", Value::Bool(false)),
            ]),
        ));
        sections.push(section(
            "limitations",
            object([
                ("truncated", Value::Bool(trace.truncated)),
                (
                    "unavailable",
                    Value::Array(vec![
                        Value::String("registers".into()),
                        Value::String("native-unwind".into()),
                        Value::String("physical-paths".into()),
                    ]),
                ),
                ("events_seen", Value::from(trace.events_seen)),
            ]),
        ));

        if options.include_source_maps && !trace.source_maps.is_empty() {
            sections.push(section(
                "source_maps",
                source_maps_section(&trace.source_maps),
            ));
        }
        if options.include_retainers && trace.roots.iter().any(|root| !root.retainers.is_empty()) {
            sections.push(section("retainers", retainers_section(trace)));
        }

        let mut artifact = Self {
            format: DUMP_SCHEMA.into(),
            version: 1,
            content_sha256: String::new(),
            sections,
        };
        artifact.content_sha256 = artifact.calculate_content_hash()?;
        artifact.validate(options.max_dump_bytes)?;
        Ok(artifact)
    }

    /// Encodes the artifact and recomputes its content address.
    pub fn encode(&self) -> Result<Vec<u8>, DumpError> {
        let mut artifact = self.clone();
        artifact.content_sha256 = artifact.calculate_content_hash()?;
        artifact.validate(MAX_DUMP_BYTES)?;
        let bytes = canonical_bytes(&artifact)?;
        if bytes.len() > MAX_DUMP_BYTES {
            return Err(DumpError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_DUMP_BYTES,
            });
        }
        Ok(bytes)
    }

    /// Decodes and integrity-checks one canonical `.tdump` byte sequence.
    pub fn decode(bytes: &[u8]) -> Result<Self, DumpError> {
        if bytes.len() > MAX_DUMP_BYTES {
            return Err(DumpError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_DUMP_BYTES,
            });
        }
        let artifact: Self = serde_json::from_slice(bytes)
            .map_err(|error| DumpError::Corrupt(format!("invalid JSON envelope: {error}")))?;
        artifact.validate(MAX_DUMP_BYTES)?;
        if canonical_bytes(&artifact)? != bytes {
            return Err(DumpError::Corrupt(
                "dump is not in canonical encoding".into(),
            ));
        }
        let expected = artifact.calculate_content_hash()?;
        if artifact.content_sha256 != expected {
            return Err(DumpError::Integrity {
                expected,
                actual: artifact.content_sha256,
            });
        }
        Ok(artifact)
    }

    pub fn section(&self, name: &str) -> Option<&Value> {
        self.sections
            .iter()
            .find(|section| section.name == name)
            .map(|section| &section.value)
    }

    fn calculate_content_hash(&self) -> Result<String, DumpError> {
        let mut unsigned = self.clone();
        unsigned.content_sha256.clear();
        let bytes = canonical_bytes(&unsigned)?;
        Ok(hex_digest(&bytes))
    }

    fn validate(&self, max_bytes: usize) -> Result<(), DumpError> {
        if self.format != DUMP_SCHEMA || self.version != 1 {
            return Err(DumpError::WrongFormat {
                format: self.format.clone(),
                version: self.version,
            });
        }
        if self.content_sha256.len() != 64
            || !self
                .content_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DumpError::Corrupt(
                "content_sha256 must be a 64-character hexadecimal digest".into(),
            ));
        }
        let mut names = BTreeSet::new();
        for section in &self.sections {
            if !names.insert(section.name.as_str()) {
                return Err(DumpError::DuplicateSection(section.name.clone()));
            }
            if !REQUIRED_SECTIONS.contains(&section.name.as_str())
                && !OPTIONAL_SECTIONS.contains(&section.name.as_str())
            {
                return Err(DumpError::UnknownSection(section.name.clone()));
            }
            if !valid_section_shape(&section.name, &section.value) {
                return Err(DumpError::InvalidSection(format!(
                    "section `{}` has the wrong JSON shape",
                    section.name
                )));
            }
        }
        for required in REQUIRED_SECTIONS {
            if !names.contains(required) {
                return Err(DumpError::MissingSection(required));
            }
        }
        if self.sections.len() > REQUIRED_SECTIONS.len() + OPTIONAL_SECTIONS.len() {
            return Err(DumpError::Corrupt("too many sections".into()));
        }
        let bytes = canonical_bytes(self)?;
        if bytes.len() > max_bytes {
            return Err(DumpError::TooLarge {
                bytes: bytes.len(),
                limit: max_bytes,
            });
        }
        validate_header(self.section("header").expect("required header is present"))?;
        let identity: DumpIdentity = section_as(self, "identity")?;
        identity.validate()?;
        let termination: DumpTermination = section_as(self, "termination")?;
        termination.validate()?;
        let limitations = self
            .section("limitations")
            .expect("required limitations are present");
        if limitations
            .get("truncated")
            .and_then(Value::as_bool)
            .is_none()
            || limitations
                .get("unavailable")
                .and_then(Value::as_array)
                .is_none()
            || limitations
                .get("events_seen")
                .and_then(Value::as_u64)
                .is_none()
        {
            return Err(DumpError::InvalidSection(
                "limitations must contain truncated, unavailable and events_seen".into(),
            ));
        }
        let redaction = self
            .section("redaction")
            .expect("required redaction is present");
        if redaction.get("payloads").and_then(Value::as_str) != Some("omitted-by-default")
            || redaction.get("secrets").and_then(Value::as_str) != Some("never-emitted-by-default")
            || redaction.get("paths").and_then(Value::as_str) != Some("logical-only")
            || redaction.get("network_upload").and_then(Value::as_bool) != Some(false)
            || redaction.get("executes_dump_code").and_then(Value::as_bool) != Some(false)
        {
            return Err(DumpError::InvalidSection(
                "redaction policy is not the locked payload-free policy".into(),
            ));
        }
        Ok(())
    }
}

/// Stable, payload-free summary produced by the offline analyzer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpAnalysis {
    pub format: String,
    pub content_sha256: String,
    pub identity: DumpIdentity,
    pub termination: DumpTermination,
    pub sections: Vec<String>,
    pub task_count: u64,
    pub thread_count: u64,
    pub heap_object_count: u64,
    pub resource_count: u64,
    pub scheduler_event_count: u64,
    pub truncated: bool,
    pub unavailable: Vec<String>,
}

impl DumpAnalysis {
    pub fn from_artifact(artifact: &DumpArtifact) -> Result<Self, DumpError> {
        let identity: DumpIdentity = section_as(artifact, "identity")?;
        let termination: DumpTermination = section_as(artifact, "termination")?;
        let limitations = artifact
            .section("limitations")
            .ok_or(DumpError::MissingSection("limitations"))?;
        let truncated = limitations
            .get("truncated")
            .and_then(Value::as_bool)
            .ok_or_else(|| DumpError::InvalidSection("limitations.truncated".into()))?;
        let unavailable = limitations
            .get("unavailable")
            .and_then(Value::as_array)
            .ok_or_else(|| DumpError::InvalidSection("limitations.unavailable".into()))?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| DumpError::InvalidSection("limitations.unavailable".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let stacks = artifact
            .section("stacks")
            .and_then(Value::as_array)
            .ok_or_else(|| DumpError::InvalidSection("stacks".into()))?;
        let heap_objects = artifact
            .section("heap_summary")
            .and_then(|value| value.get("object_count"))
            .and_then(Value::as_u64)
            .ok_or_else(|| DumpError::InvalidSection("heap_summary.object_count".into()))?;
        let resources = artifact
            .section("resource_ledger")
            .and_then(Value::as_array)
            .ok_or_else(|| DumpError::InvalidSection("resource_ledger".into()))?;
        let scheduler_events = artifact
            .section("scheduler_tail")
            .and_then(Value::as_array)
            .ok_or_else(|| DumpError::InvalidSection("scheduler_tail".into()))?;
        Ok(Self {
            format: artifact.format.clone(),
            content_sha256: artifact.content_sha256.clone(),
            identity,
            termination,
            sections: artifact
                .sections
                .iter()
                .map(|section| section.name.clone())
                .collect(),
            task_count: stacks
                .iter()
                .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("task"))
                .count() as u64,
            thread_count: stacks
                .iter()
                .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("thread"))
                .count() as u64,
            heap_object_count: heap_objects,
            resource_count: resources.len() as u64,
            scheduler_event_count: scheduler_events.len() as u64,
            truncated,
            unavailable,
        })
    }

    pub fn to_json(&self) -> Result<String, DumpError> {
        serde_json::to_string(self).map_err(|error| DumpError::Serialization(error.to_string()))
    }

    pub fn render_human(&self) -> String {
        let unavailable = if self.unavailable.is_empty() {
            "none".to_owned()
        } else {
            self.unavailable.join(", ")
        };
        format!(
            "Tondo dump {format}\ncontent_sha256: {hash}\ntermination: {reason} (program={program:?}, command={command:?})\nidentity: run={run} attempt={attempt} shard={shard} profile={profile} target={target} backend={backend}\nobserved: tasks={tasks} threads={threads} heap_objects={heap} resources={resources} scheduler_tail={scheduler}\ntruncated: {truncated}\nunavailable: {unavailable}\n",
            format = self.format,
            hash = self.content_sha256,
            reason = self.termination.reason,
            program = self.termination.program_exit_status,
            command = self.termination.command_exit_status,
            run = self.identity.run_id,
            attempt = self.identity.attempt_id,
            shard = self.identity.shard,
            profile = self.identity.profile,
            target = self.identity.target,
            backend = self.identity.backend,
            tasks = self.task_count,
            threads = self.thread_count,
            heap = self.heap_object_count,
            resources = self.resource_count,
            scheduler = self.scheduler_event_count,
            truncated = self.truncated,
            unavailable = unavailable,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpError {
    WrongFormat { format: String, version: u8 },
    MissingSection(&'static str),
    DuplicateSection(String),
    UnknownSection(String),
    InvalidSection(String),
    InvalidTrace(String),
    InvalidLimit { limit: usize },
    TooLarge { bytes: usize, limit: usize },
    Corrupt(String),
    Integrity { expected: String, actual: String },
    Serialization(String),
}

impl fmt::Display for DumpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongFormat { format, version } => {
                write!(
                    formatter,
                    "unsupported dump format `{format}` version {version}"
                )
            }
            Self::MissingSection(section) => write!(formatter, "missing dump section `{section}`"),
            Self::DuplicateSection(section) => {
                write!(formatter, "duplicate dump section `{section}`")
            }
            Self::UnknownSection(section) => write!(formatter, "unknown dump section `{section}`"),
            Self::InvalidSection(message) => write!(formatter, "invalid dump section: {message}"),
            Self::InvalidTrace(format) => {
                write!(formatter, "unsupported diagnostic trace `{format}`")
            }
            Self::InvalidLimit { limit } => write!(formatter, "invalid dump byte limit {limit}"),
            Self::TooLarge { bytes, limit } => {
                write!(formatter, "dump is {bytes} bytes; limit is {limit}")
            }
            Self::Corrupt(message) => write!(formatter, "corrupt dump: {message}"),
            Self::Integrity { expected, actual } => {
                write!(
                    formatter,
                    "dump hash mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Serialization(message) => write!(formatter, "cannot serialize dump: {message}"),
        }
    }
}

impl Error for DumpError {}

pub fn capture_dump(
    trace: &DiagnosticTrace,
    identity: DumpIdentity,
    termination: DumpTermination,
) -> Result<Vec<u8>, DumpError> {
    DumpArtifact::from_trace(trace, identity, termination)?.encode()
}

pub fn analyze_dump(bytes: &[u8]) -> Result<DumpAnalysis, DumpError> {
    DumpAnalysis::from_artifact(&DumpArtifact::decode(bytes)?)
}

fn section(name: &str, value: Value) -> DumpSection {
    DumpSection {
        name: name.into(),
        value,
    }
}

fn object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.into(), value);
    }
    Value::Object(map)
}

fn source_value(source: &DiagnosticSource) -> Value {
    object([
        ("function", Value::String(source.function.clone())),
        ("span", span_value(source.span)),
    ])
}

fn span_value(span: BytecodeSpan) -> Value {
    object([
        ("file", Value::from(span.file)),
        ("start", Value::from(span.start)),
        ("end", Value::from(span.end)),
    ])
}

fn stack_frames(stack: &[DiagnosticSource]) -> Value {
    Value::Array(stack.iter().map(source_value).collect())
}

fn stack_section(trace: &DiagnosticTrace) -> Value {
    let mut entries = Vec::new();
    let mut seen_tasks = BTreeSet::new();
    let mut seen_threads = BTreeSet::new();
    for event in &trace.events {
        match event {
            DiagnosticEvent::Thread { id, state } if seen_threads.insert(*id) => {
                entries.push(object([
                    ("kind", Value::String("thread".into())),
                    ("id", Value::from(*id)),
                    ("state", Value::String(thread_state(*state).into())),
                    ("frames", Value::Array(Vec::new())),
                ]));
            }
            DiagnosticEvent::Task {
                id,
                parent,
                state,
                stack,
            } if seen_tasks.insert(*id) => {
                entries.push(object([
                    ("kind", Value::String("task".into())),
                    ("id", Value::from(*id)),
                    ("parent", parent.map_or(Value::Null, Value::from)),
                    ("state", Value::String(task_state(*state).into())),
                    ("frames", stack_frames(stack)),
                ]));
            }
            _ => {}
        }
    }
    Value::Array(entries)
}

fn heap_summary_section(trace: &DiagnosticTrace) -> Value {
    let mut objects: BTreeMap<u64, (u64, u64, u64)> = BTreeMap::new();
    let mut allocation_count = 0_u64;
    let mut replacement_count = 0_u64;
    let mut total_bytes = 0_u64;
    for event in &trace.events {
        let DiagnosticEvent::Heap {
            object_id,
            operation,
            bytes,
            ..
        } = event
        else {
            continue;
        };
        let entry = objects.entry(*object_id).or_insert((0, 0, 0));
        match operation {
            DiagnosticHeapOperation::Allocate => {
                allocation_count += 1;
                entry.1 += 1;
                total_bytes = total_bytes.saturating_add(*bytes);
            }
            DiagnosticHeapOperation::Replace => {
                replacement_count += 1;
                entry.2 += 1;
            }
        }
        entry.0 = entry.0.max(*bytes);
    }
    object([
        ("object_count", Value::from(objects.len() as u64)),
        ("allocation_count", Value::from(allocation_count)),
        ("replacement_count", Value::from(replacement_count)),
        ("allocated_bytes", Value::from(total_bytes)),
        (
            "objects",
            Value::Array(
                objects
                    .into_iter()
                    .map(|(id, (bytes, allocations, replacements))| {
                        object([
                            ("object_id", Value::from(id)),
                            ("max_bytes", Value::from(bytes)),
                            ("allocations", Value::from(allocations)),
                            ("replacements", Value::from(replacements)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

fn resource_ledger_section(trace: &DiagnosticTrace) -> Value {
    Value::Array(
        trace
            .resources
            .iter()
            .map(|resource| {
                object([
                    ("id", Value::from(resource.id)),
                    ("kind", Value::String(resource.kind.clone())),
                    ("owner_task", Value::from(resource.owner_task)),
                    (
                        "state",
                        Value::String(resource_state(resource.state).into()),
                    ),
                    ("first_event", Value::from(resource.first_event)),
                    ("last_event", Value::from(resource.last_event)),
                ])
            })
            .collect(),
    )
}

fn scheduler_section(trace: &DiagnosticTrace) -> Value {
    Value::Array(
        trace
            .scheduler_tail
            .iter()
            .filter_map(|event| match event {
                DiagnosticEvent::Scheduler {
                    task_id,
                    operation,
                    queue_len,
                } => Some(object([
                    ("task_id", Value::from(*task_id)),
                    (
                        "operation",
                        Value::String(scheduler_operation(*operation).into()),
                    ),
                    ("queue_len", Value::from(*queue_len)),
                ])),
                _ => None,
            })
            .collect(),
    )
}

fn source_maps_section(sources: &[DiagnosticSource]) -> Value {
    Value::Array(sources.iter().map(source_value).collect())
}

fn retainers_section(trace: &DiagnosticTrace) -> Value {
    Value::Array(
        trace
            .roots
            .iter()
            .flat_map(|root| {
                root.retainers.iter().map(move |retainer| {
                    object([
                        ("task_id", Value::from(root.task_id)),
                        ("object_id", Value::from(retainer.object_id)),
                        ("owner", Value::String(retainer.owner.clone())),
                    ])
                })
            })
            .collect(),
    )
}

fn valid_section_shape(name: &str, value: &Value) -> bool {
    match name {
        "stacks" | "resource_ledger" | "scheduler_tail" | "source_maps" | "retainers" => {
            value.is_array()
        }
        "header" | "termination" | "identity" | "heap_summary" | "redaction" | "limitations"
        | "registers" => value.is_object(),
        _ => false,
    }
}

fn validate_header(value: &Value) -> Result<(), DumpError> {
    if value.get("format").and_then(Value::as_str) != Some(DUMP_SCHEMA)
        || value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("content_address").and_then(Value::as_str) != Some("sha256")
        || value.get("user_payloads").and_then(Value::as_str) != Some("omitted-by-default")
    {
        return Err(DumpError::InvalidSection(
            "header does not describe tondo-dump/1".into(),
        ));
    }
    Ok(())
}

fn section_as<T: for<'de> Deserialize<'de>>(
    artifact: &DumpArtifact,
    name: &'static str,
) -> Result<T, DumpError> {
    let value = artifact
        .section(name)
        .ok_or(DumpError::MissingSection(name))?;
    serde_json::from_value(value.clone())
        .map_err(|error| DumpError::InvalidSection(format!("{name}: {error}")))
}

fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, DumpError> {
    let value =
        serde_json::to_value(value).map_err(|error| DumpError::Serialization(error.to_string()))?;
    serde_json::to_vec(&canonical_value(value))
        .map_err(|error| DumpError::Serialization(error.to_string()))
}

fn canonical_value(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, canonical_value(value));
            }
            Value::Object(canonical)
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_value).collect()),
        value => value,
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn thread_state(state: DiagnosticThreadState) -> &'static str {
    match state {
        DiagnosticThreadState::Started => "started",
        DiagnosticThreadState::Stopped => "stopped",
    }
}

fn task_state(state: DiagnosticTaskState) -> &'static str {
    match state {
        DiagnosticTaskState::Created => "created",
        DiagnosticTaskState::Runnable => "runnable",
        DiagnosticTaskState::Running => "running",
        DiagnosticTaskState::Waiting => "waiting",
        DiagnosticTaskState::CancelRequested => "cancel_requested",
        DiagnosticTaskState::Complete => "complete",
        DiagnosticTaskState::Consumed => "consumed",
    }
}

fn resource_state(state: DiagnosticResourceState) -> &'static str {
    match state {
        DiagnosticResourceState::Acquired => "acquired",
        DiagnosticResourceState::Released => "released",
    }
}

fn scheduler_operation(operation: DiagnosticSchedulerOperation) -> &'static str {
    match operation {
        DiagnosticSchedulerOperation::Enqueue => "enqueue",
        DiagnosticSchedulerOperation::Switch => "switch",
        DiagnosticSchedulerOperation::Park => "park",
        DiagnosticSchedulerOperation::Wake => "wake",
        DiagnosticSchedulerOperation::Complete => "complete",
    }
}

#[cfg(test)]
mod tests {
    use super::super::diagnostics::{
        DiagnosticConfig, DiagnosticEvent, DiagnosticHeapOperation, DiagnosticResource,
        DiagnosticRootSnapshot, DiagnosticSource,
    };
    use super::*;
    use crate::bytecode::BytecodeSpan;

    fn trace() -> DiagnosticTrace {
        let source = DiagnosticSource {
            function: "main".into(),
            span: BytecodeSpan {
                file: 1,
                start: 2,
                end: 3,
            },
        };
        DiagnosticTrace {
            format: super::super::diagnostics::DIAGNOSTIC_SCHEMA,
            config: DiagnosticConfig::default(),
            events: vec![
                DiagnosticEvent::Thread {
                    id: 0,
                    state: DiagnosticThreadState::Started,
                },
                DiagnosticEvent::Task {
                    id: 1,
                    parent: None,
                    state: DiagnosticTaskState::Complete,
                    stack: vec![source.clone()],
                },
                DiagnosticEvent::Heap {
                    object_id: 4,
                    operation: DiagnosticHeapOperation::Allocate,
                    bytes: 8,
                    owner_task: 1,
                    source: Some(source.clone()),
                    stack: vec![source.clone()],
                },
                DiagnosticEvent::Scheduler {
                    task_id: 1,
                    operation: DiagnosticSchedulerOperation::Complete,
                    queue_len: 0,
                },
            ],
            scheduler_tail: vec![DiagnosticEvent::Scheduler {
                task_id: 1,
                operation: DiagnosticSchedulerOperation::Complete,
                queue_len: 0,
            }],
            roots: vec![DiagnosticRootSnapshot {
                task_id: 1,
                object_ids: vec![4],
                retainers: vec![super::super::diagnostics::DiagnosticRetainer {
                    object_id: 4,
                    owner: "task:1".into(),
                }],
            }],
            resources: vec![DiagnosticResource {
                id: 9,
                kind: "File".into(),
                owner_task: 1,
                state: DiagnosticResourceState::Released,
                first_event: 1,
                last_event: 2,
            }],
            source_maps: vec![source],
            events_seen: 4,
            truncated: false,
        }
    }

    fn identity() -> DumpIdentity {
        DumpIdentity {
            run_id: "run-1".into(),
            attempt_id: "attempt-1".into(),
            shard: "0/1".into(),
            profile: "crash".into(),
            target: "linux-x86_64".into(),
            backend: "bytecode-vm".into(),
            toolchain: "tondo-0.1".into(),
            source_revision: "abc123".into(),
        }
    }

    fn termination() -> DumpTermination {
        DumpTermination {
            reason: "panic".into(),
            program_exit_status: Some(101),
            command_exit_status: Some(101),
        }
    }

    #[test]
    fn capture_round_trips_with_a_content_address_and_optional_sections() {
        let artifact = DumpArtifact::from_trace(&trace(), identity(), termination()).unwrap();
        assert_eq!(artifact.format, DUMP_SCHEMA);
        assert!(artifact.section("source_maps").is_some());
        assert!(artifact.section("retainers").is_some());
        let encoded = artifact.encode().unwrap();
        let decoded = DumpArtifact::decode(&encoded).unwrap();
        assert_eq!(decoded, artifact);
        assert_eq!(encoded, decoded.encode().unwrap());
    }

    #[test]
    fn analyzer_produces_stable_human_and_json_views() {
        let encoded = capture_dump(&trace(), identity(), termination()).unwrap();
        let analysis = analyze_dump(&encoded).unwrap();
        assert_eq!(analysis.task_count, 1);
        assert_eq!(analysis.thread_count, 1);
        assert_eq!(analysis.heap_object_count, 1);
        assert_eq!(analysis.resource_count, 1);
        assert_eq!(analysis.scheduler_event_count, 1);
        assert!(analysis.render_human().contains("termination: panic"));
        assert!(analysis.to_json().unwrap().contains("content_sha256"));
    }

    #[test]
    fn wrong_format_missing_duplicate_and_unknown_sections_are_rejected() {
        let artifact = DumpArtifact::from_trace(&trace(), identity(), termination()).unwrap();
        let mut wrong = artifact.clone();
        wrong.format = "other/1".into();
        assert!(matches!(wrong.encode(), Err(DumpError::WrongFormat { .. })));

        let mut missing = artifact.clone();
        missing.sections.retain(|section| section.name != "header");
        assert!(matches!(
            missing.encode(),
            Err(DumpError::MissingSection("header"))
        ));

        let mut duplicate = artifact.clone();
        duplicate.sections.push(duplicate.sections[0].clone());
        assert!(matches!(
            duplicate.encode(),
            Err(DumpError::DuplicateSection(_))
        ));

        let mut unknown = artifact;
        unknown
            .sections
            .push(section("payload", Value::Array(Vec::new())));
        assert!(matches!(
            unknown.encode(),
            Err(DumpError::UnknownSection(_))
        ));
    }

    #[test]
    fn tampering_and_noncanonical_encoding_are_rejected() {
        let encoded = capture_dump(&trace(), identity(), termination()).unwrap();
        let mut tampered: Value = serde_json::from_slice(&encoded).unwrap();
        tampered["sections"][0]["value"]["version"] = Value::from(9_u8);
        let tampered_bytes = serde_json::to_vec(&tampered).unwrap();
        assert!(matches!(
            DumpArtifact::decode(&tampered_bytes),
            Err(DumpError::Integrity { .. }) | Err(DumpError::InvalidSection(_))
        ));

        let pretty =
            serde_json::to_vec_pretty(&serde_json::from_slice::<Value>(&encoded).unwrap()).unwrap();
        assert!(matches!(
            DumpArtifact::decode(&pretty),
            Err(DumpError::Corrupt(_))
        ));
    }

    #[test]
    fn payloads_are_not_present_and_limits_fail_closed() {
        let artifact = DumpArtifact::from_trace_with_options(
            &trace(),
            identity(),
            termination(),
            DumpOptions {
                max_dump_bytes: 1,
                include_source_maps: false,
                include_retainers: false,
            },
        );
        assert!(matches!(artifact, Err(DumpError::TooLarge { .. })));
        let encoded = capture_dump(&trace(), identity(), termination()).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(!text.contains("diagnostic"));
        assert!(!text.contains("user-secret"));
        assert!(text.contains("omitted-by-default"));
    }

    #[test]
    fn invalid_identity_and_termination_are_rejected() {
        let mut invalid_identity = identity();
        invalid_identity.run_id.clear();
        assert!(matches!(
            DumpArtifact::from_trace(&trace(), invalid_identity, termination()),
            Err(DumpError::InvalidSection(_))
        ));
        let mut termination = termination();
        termination.reason.clear();
        assert!(matches!(
            DumpArtifact::from_trace(&trace(), identity(), termination),
            Err(DumpError::InvalidSection(_))
        ));
    }
}
