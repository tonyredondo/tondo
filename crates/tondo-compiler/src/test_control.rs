//! Sealed execution envelope for test-only operations.
//!
//! The envelope is a runtime-owned Rust object, never a Tondo value.  A
//! clone is the inheritance link used by helpers and structured tasks; no
//! node identity, sink or policy is exposed through that link.  All writes
//! are linearized under one mutex so limits, tag merges and evidence names
//! remain atomic even when tasks move between host threads.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::artifact::sha256;
use crate::test_plan::TestSourceClass;

pub const TEST_CONTROL_FORMAT: &str = "tondo-test-control-draft/1";

/// Per-attempt limits enforced by the envelope before publishing a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvelopeLimits {
    output_bytes: u64,
    artifact_bytes: u64,
    snapshot_bytes: u64,
}

impl EnvelopeLimits {
    pub const fn new(output_bytes: u64, artifact_bytes: u64, snapshot_bytes: u64) -> Self {
        Self {
            output_bytes,
            artifact_bytes,
            snapshot_bytes,
        }
    }

    pub const fn output_bytes(self) -> u64 {
        self.output_bytes
    }

    pub const fn artifact_bytes(self) -> u64 {
        self.artifact_bytes
    }

    pub const fn snapshot_bytes(self) -> u64 {
        self.snapshot_bytes
    }
}

/// Lifecycle phase used for cleanup precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionPhase {
    Setup,
    Body,
    Cleanup,
    Closed,
}

impl ExecutionPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Setup => 0,
            Self::Body => 1,
            Self::Cleanup => 2,
            Self::Closed => 3,
        }
    }
}

/// Terminal state recorded by `failNow`, `skip`, or a cleanup failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    FailNow { code: &'static str, message: String },
    Skipped { reason: String },
    CleanupFailure { code: String, message: String },
    ResourceLimit { kind: &'static str },
}

/// Ordered log entry. The sequence is local to one envelope/attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRecord {
    sequence: u64,
    message: String,
}

impl LogRecord {
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Immutable artifact descriptor; bytes are kept in the envelope until the
/// coordinator copies them to its content-addressed store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactEvidence {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
    sha256: String,
}

impl ArtifactEvidence {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Snapshot result recorded for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Matched {
        expected_sha256: String,
        actual_sha256: String,
    },
    Missing {
        actual_sha256: String,
    },
    Mismatched {
        expected_sha256: String,
        actual_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEvidence {
    name: String,
    outcome: SnapshotOutcome,
}

impl SnapshotEvidence {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn outcome(&self) -> &SnapshotOutcome {
        &self.outcome
    }
}

/// Deterministic virtual-time observation for the current envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualTimeRecord {
    index: u32,
    elapsed_ns: i128,
    settles: u32,
    advances: u32,
}

impl VirtualTimeRecord {
    pub const fn index(&self) -> u32 {
        self.index
    }

    pub const fn elapsed_ns(&self) -> i128 {
        self.elapsed_ns
    }

    pub const fn settles(&self) -> u32 {
        self.settles
    }

    pub const fn advances(&self) -> u32 {
        self.advances
    }
}

/// A detached, report-ready copy of the envelope's private evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeReport {
    phase: ExecutionPhase,
    terminal: Option<Terminal>,
    logs: Vec<LogRecord>,
    tags: BTreeMap<String, String>,
    artifacts: Vec<ArtifactEvidence>,
    snapshots: Vec<SnapshotEvidence>,
    virtual_time: Vec<VirtualTimeRecord>,
    stdout: String,
    stderr: String,
}

impl EnvelopeReport {
    pub const fn phase(&self) -> ExecutionPhase {
        self.phase
    }

    pub fn terminal(&self) -> Option<&Terminal> {
        self.terminal.as_ref()
    }

    pub fn logs(&self) -> &[LogRecord] {
        &self.logs
    }

    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    pub fn artifacts(&self) -> &[ArtifactEvidence] {
        &self.artifacts
    }

    pub fn snapshots(&self) -> &[SnapshotEvidence] {
        &self.snapshots
    }

    pub fn virtual_time(&self) -> &[VirtualTimeRecord] {
        &self.virtual_time
    }

    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    Closed,
    PhaseRegression {
        current: ExecutionPhase,
        requested: ExecutionPhase,
    },
    VirtualTimeActive,
    VirtualTimeMissing,
    OutputLimit,
    ArtifactLimit,
    SnapshotLimit,
    TagConflict {
        key: String,
    },
    FailNow {
        message: String,
    },
    Skip {
        reason: String,
    },
    SkipDuringCleanup,
    CleanupFailure {
        code: String,
        message: String,
    },
    ArtifactConflict {
        name: String,
    },
    SnapshotConflict {
        name: String,
    },
    SnapshotMismatch {
        name: String,
    },
    InvalidName {
        value: String,
    },
    InvalidMediaType {
        value: String,
    },
    InvalidDuration,
    ProductionOperation {
        operation: &'static str,
    },
    Poisoned,
}

impl ControlError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Closed => "E3000",
            Self::PhaseRegression { .. } => "E3001",
            Self::VirtualTimeActive => "P2004",
            Self::VirtualTimeMissing => "E3002",
            Self::OutputLimit => "R0001",
            Self::ArtifactLimit => "R0002",
            Self::SnapshotLimit => "R0003",
            Self::TagConflict { .. } => "P2002",
            Self::FailNow { .. } => "P0007",
            Self::Skip { .. } => "P0008",
            Self::SkipDuringCleanup => "P2001",
            Self::CleanupFailure { .. } => "P0007",
            Self::ArtifactConflict { .. } => "P2006",
            Self::SnapshotConflict { .. } => "P2008",
            Self::SnapshotMismatch { .. } => "P2007",
            Self::InvalidName { .. } | Self::InvalidMediaType { .. } => "P2006",
            Self::InvalidDuration => "P2005",
            Self::ProductionOperation { .. } => "E2003",
            Self::Poisoned => "E3003",
        }
    }
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("test envelope is closed"),
            Self::PhaseRegression { current, requested } => {
                write!(
                    formatter,
                    "cannot move envelope from {current:?} to {requested:?}"
                )
            }
            Self::VirtualTimeActive => formatter.write_str("virtual time domain is already active"),
            Self::VirtualTimeMissing => formatter.write_str("virtual time domain is not active"),
            Self::OutputLimit => formatter.write_str("test output budget is exhausted"),
            Self::ArtifactLimit => formatter.write_str("test artifact budget is exhausted"),
            Self::SnapshotLimit => formatter.write_str("test snapshot budget is exhausted"),
            Self::TagConflict { key } => {
                write!(formatter, "test tag `{key}` has conflicting values")
            }
            Self::FailNow { message } => write!(formatter, "test failed immediately: {message}"),
            Self::Skip { reason } => write!(formatter, "test skipped: {reason}"),
            Self::SkipDuringCleanup => formatter.write_str("skip is not allowed during cleanup"),
            Self::CleanupFailure { code, message } => {
                write!(formatter, "cleanup failed with {code}: {message}")
            }
            Self::ArtifactConflict { name } => write!(formatter, "artifact `{name}` is duplicated"),
            Self::SnapshotConflict { name } => {
                write!(formatter, "snapshot `{name}` is duplicated or invalid")
            }
            Self::SnapshotMismatch { name } => {
                write!(formatter, "snapshot `{name}` does not match")
            }
            Self::InvalidName { value } => write!(formatter, "evidence name `{value}` is invalid"),
            Self::InvalidMediaType { value } => {
                write!(formatter, "media type `{value}` is invalid")
            }
            Self::InvalidDuration => {
                formatter.write_str("virtual duration is negative or overflows")
            }
            Self::ProductionOperation { operation } => {
                write!(
                    formatter,
                    "{operation} is test-only and cannot enter production"
                )
            }
            Self::Poisoned => formatter.write_str("test envelope lock is poisoned"),
        }
    }
}

impl Error for ControlError {}

#[derive(Debug)]
struct EnvelopeState {
    _node_id: String,
    limits: EnvelopeLimits,
    phase: ExecutionPhase,
    terminal: Option<Terminal>,
    sequence: u64,
    used_output: u64,
    used_artifacts: u64,
    used_snapshots: u64,
    logs: Vec<LogRecord>,
    tags: BTreeMap<String, String>,
    artifacts: BTreeMap<String, ArtifactEvidence>,
    snapshots: BTreeMap<String, SnapshotEvidence>,
    expected_snapshots: BTreeMap<String, String>,
    expected_installed: bool,
    virtual_time: Vec<VirtualTimeRecord>,
    virtual_time_active: bool,
    next_child: u64,
    child_skip: Option<(u64, String)>,
    stdout: String,
    stderr: String,
}

/// Private runtime link inherited by helpers and structured tasks.
#[derive(Debug, Clone)]
pub struct EnvelopeHandle {
    state: Arc<Mutex<EnvelopeState>>,
}

impl EnvelopeHandle {
    pub fn new(node_id: impl Into<String>, limits: EnvelopeLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(EnvelopeState {
                _node_id: node_id.into(),
                limits,
                phase: ExecutionPhase::Setup,
                terminal: None,
                sequence: 0,
                used_output: 0,
                used_artifacts: 0,
                used_snapshots: 0,
                logs: Vec::new(),
                tags: BTreeMap::new(),
                artifacts: BTreeMap::new(),
                snapshots: BTreeMap::new(),
                expected_snapshots: BTreeMap::new(),
                expected_installed: false,
                virtual_time: Vec::new(),
                virtual_time_active: false,
                next_child: 0,
                child_skip: None,
                stdout: String::new(),
                stderr: String::new(),
            })),
        }
    }

    /// Expected values are test-runner input and can only be installed before
    /// any snapshot check in this attempt.
    pub fn with_expected_snapshots(
        &self,
        expected: BTreeMap<String, String>,
    ) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.expected_installed || !state.snapshots.is_empty() {
            return Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into(),
            });
        }
        for name in expected.keys() {
            validate_evidence_name(name)
                .map_err(|_| ControlError::SnapshotConflict { name: name.clone() })?;
        }
        state.expected_snapshots = expected;
        state.expected_installed = true;
        Ok(())
    }

    pub fn set_phase(&self, phase: ExecutionPhase) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if phase.rank() < state.phase.rank() {
            return Err(ControlError::PhaseRegression {
                current: state.phase,
                requested: phase,
            });
        }
        state.phase = phase;
        Ok(())
    }

    pub fn phase(&self) -> Result<ExecutionPhase, ControlError> {
        Ok(self.lock()?.phase)
    }

    pub fn log(&self, message: impl Into<String>) -> Result<(), ControlError> {
        let message = message.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let delta = message.len() as u64;
        let limit = state.limits.output_bytes;
        reserve(
            &mut state.used_output,
            limit,
            delta,
            ControlError::OutputLimit,
        )?;
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        state.logs.push(LogRecord { sequence, message });
        Ok(())
    }

    pub fn tags(&self, values: BTreeMap<String, String>) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let mut delta = 0_u64;
        for (key, value) in &values {
            if let Some(previous) = state.tags.get(key) {
                if previous != value {
                    return Err(ControlError::TagConflict { key: key.clone() });
                }
            } else {
                delta = delta.saturating_add((key.len() + value.len()) as u64);
            }
        }
        let limit = state.limits.output_bytes;
        reserve(
            &mut state.used_output,
            limit,
            delta,
            ControlError::OutputLimit,
        )?;
        state.tags.extend(values);
        Ok(())
    }

    pub fn stdout(&self, text: impl Into<String>) -> Result<(), ControlError> {
        self.append_stream(text.into(), true)
    }

    pub fn stderr(&self, text: impl Into<String>) -> Result<(), ControlError> {
        self.append_stream(text.into(), false)
    }

    fn append_stream(&self, text: String, stdout: bool) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let limit = state.limits.output_bytes;
        reserve(
            &mut state.used_output,
            limit,
            text.len() as u64,
            ControlError::OutputLimit,
        )?;
        if stdout {
            state.stdout.push_str(&text);
        } else {
            state.stderr.push_str(&text);
        }
        Ok(())
    }

    pub fn fail_now(&self, message: impl Into<String>) -> Result<(), ControlError> {
        let message = message.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let error = ControlError::FailNow {
            message: message.clone(),
        };
        set_failure_terminal(
            &mut state,
            Terminal::FailNow {
                code: error.code(),
                message,
            },
        );
        Err(error)
    }

    pub fn skip(&self, reason: impl Into<String>) -> Result<(), ControlError> {
        let reason = reason.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.phase == ExecutionPhase::Cleanup {
            return Err(ControlError::SkipDuringCleanup);
        }
        if state.terminal.is_none() {
            state.terminal = Some(Terminal::Skipped {
                reason: reason.clone(),
            });
        }
        Err(ControlError::Skip { reason })
    }

    /// Record a failure discovered while cleanup is unwinding. Cleanup
    /// failures always supersede a previously recorded skip.
    pub fn cleanup_failure(
        &self,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), ControlError> {
        let code = code.into();
        let message = message.into();
        let mut state = self.lock()?;
        ensure_open(&state)?;
        state.terminal = Some(Terminal::CleanupFailure {
            code: code.clone(),
            message: message.clone(),
        });
        Err(ControlError::CleanupFailure { code, message })
    }

    pub fn attach(
        &self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), ControlError> {
        let name = name.into();
        let media_type = media_type.into();
        let bytes = bytes.into();
        validate_evidence_name(&name)?;
        validate_media_type(&media_type)?;
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.artifacts.contains_key(&name) {
            return Err(ControlError::ArtifactConflict { name });
        }
        let limit = state.limits.artifact_bytes;
        reserve(
            &mut state.used_artifacts,
            limit,
            bytes.len() as u64,
            ControlError::ArtifactLimit,
        )?;
        state.artifacts.insert(
            name.clone(),
            ArtifactEvidence {
                name,
                media_type,
                sha256: sha256(&bytes),
                bytes,
            },
        );
        Ok(())
    }

    pub fn snapshot(
        &self,
        name: impl Into<String>,
        actual: impl AsRef<str>,
    ) -> Result<SnapshotOutcome, ControlError> {
        let name = name.into();
        validate_evidence_name(&name)
            .map_err(|_| ControlError::SnapshotConflict { name: name.clone() })?;
        let actual = actual.as_ref();
        let actual_sha256 = sha256(actual.as_bytes());
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.snapshots.contains_key(&name) {
            return Err(ControlError::SnapshotConflict { name });
        }
        let limit = state.limits.snapshot_bytes;
        reserve(
            &mut state.used_snapshots,
            limit,
            actual.len() as u64,
            ControlError::SnapshotLimit,
        )?;
        let outcome = match state.expected_snapshots.get(&name) {
            Some(expected) => {
                let expected_sha256 = sha256(expected.as_bytes());
                if expected == actual {
                    SnapshotOutcome::Matched {
                        expected_sha256,
                        actual_sha256,
                    }
                } else {
                    SnapshotOutcome::Mismatched {
                        expected_sha256,
                        actual_sha256,
                    }
                }
            }
            None => SnapshotOutcome::Missing { actual_sha256 },
        };
        state.snapshots.insert(
            name.clone(),
            SnapshotEvidence {
                name: name.clone(),
                outcome: outcome.clone(),
            },
        );
        match outcome {
            SnapshotOutcome::Matched { .. } => Ok(outcome),
            SnapshotOutcome::Missing { .. } | SnapshotOutcome::Mismatched { .. } => {
                Err(ControlError::SnapshotMismatch { name })
            }
        }
    }

    pub fn with_virtual_time<T>(
        &self,
        operation: impl FnOnce(&VirtualTime<'_>) -> Result<T, ControlError>,
    ) -> Result<T, ControlError> {
        let controller = self.enter_virtual_time()?;
        let result = operation(&controller);
        let close_result = controller.close();
        match (result, close_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn enter_virtual_time(&self) -> Result<VirtualTime<'_>, ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.virtual_time_active {
            return Err(ControlError::VirtualTimeActive);
        }
        state.virtual_time_active = true;
        state.virtual_time.push(VirtualTimeRecord {
            index: 0,
            elapsed_ns: 0,
            settles: 0,
            advances: 0,
        });
        Ok(VirtualTime {
            envelope: self,
            closed: false,
        })
    }

    pub fn child(&self) -> Result<StructuredTask, ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        let ordinal = state.next_child;
        state.next_child = state.next_child.saturating_add(1);
        Ok(StructuredTask {
            envelope: self.clone(),
            ordinal,
        })
    }

    pub fn close(&self) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.virtual_time_active {
            return Err(ControlError::VirtualTimeActive);
        }
        state.phase = ExecutionPhase::Closed;
        Ok(())
    }

    pub fn report(&self) -> Result<EnvelopeReport, ControlError> {
        let state = self.lock()?;
        Ok(EnvelopeReport {
            phase: state.phase,
            terminal: state.terminal.clone(),
            logs: state.logs.clone(),
            tags: state.tags.clone(),
            artifacts: state.artifacts.values().cloned().collect(),
            snapshots: state.snapshots.values().cloned().collect(),
            virtual_time: state.virtual_time.clone(),
            stdout: state.stdout.clone(),
            stderr: state.stderr.clone(),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, EnvelopeState>, ControlError> {
        self.state.lock().map_err(|_| ControlError::Poisoned)
    }

    fn record_child_skip(&self, ordinal: u64, reason: String) -> Result<(), ControlError> {
        let mut state = self.lock()?;
        ensure_open(&state)?;
        if state.terminal.is_none() {
            state.child_skip = Some((ordinal, reason.clone()));
            state.terminal = Some(Terminal::Skipped { reason });
        } else if let Some((first, first_reason)) = &mut state.child_skip
            && ordinal < *first
        {
            *first = ordinal;
            *first_reason = reason.clone();
            state.terminal = Some(Terminal::Skipped { reason });
        }
        Err(ControlError::Skip {
            reason: "structured child requested skip".into(),
        })
    }
}

/// A virtual-time controller borrowed from one envelope. It exposes no node
/// identity or sink and is invalid after the closure returns.
pub struct VirtualTime<'a> {
    envelope: &'a EnvelopeHandle,
    closed: bool,
}

impl<'a> VirtualTime<'a> {
    pub fn settle(&self) -> Result<(), ControlError> {
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.settles = record.settles.saturating_add(1);
        Ok(())
    }

    pub fn advance(&self, duration_ns: i128) -> Result<(), ControlError> {
        if duration_ns < 0 {
            return Err(ControlError::InvalidDuration);
        }
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        let Some(record) = state.virtual_time.last_mut() else {
            return Err(ControlError::VirtualTimeMissing);
        };
        record.elapsed_ns = record
            .elapsed_ns
            .checked_add(duration_ns)
            .ok_or(ControlError::InvalidDuration)?;
        record.advances = record.advances.saturating_add(1);
        Ok(())
    }

    fn close(mut self) -> Result<(), ControlError> {
        if self.closed {
            return Ok(());
        }
        let mut state = self.envelope.lock()?;
        ensure_open(&state)?;
        state.virtual_time_active = false;
        self.closed = true;
        Ok(())
    }
}

impl Drop for VirtualTime<'_> {
    fn drop(&mut self) {
        if !self.closed {
            if let Ok(mut state) = self.envelope.state.lock() {
                state.virtual_time_active = false;
            }
            self.closed = true;
        }
    }
}

/// Inherited handle for one structured task. Its ordinal only chooses the
/// deterministic first child skip; it is never part of user-visible data.
#[derive(Debug, Clone)]
pub struct StructuredTask {
    envelope: EnvelopeHandle,
    ordinal: u64,
}

impl StructuredTask {
    pub fn log(&self, message: impl Into<String>) -> Result<(), ControlError> {
        self.envelope.log(message)
    }

    pub fn tags(&self, values: BTreeMap<String, String>) -> Result<(), ControlError> {
        self.envelope.tags(values)
    }

    pub fn attach(
        &self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), ControlError> {
        self.envelope.attach(name, media_type, bytes)
    }

    pub fn skip(&self, reason: impl Into<String>) -> Result<(), ControlError> {
        self.envelope.record_child_skip(self.ordinal, reason.into())
    }
}

fn ensure_open(state: &EnvelopeState) -> Result<(), ControlError> {
    if state.phase == ExecutionPhase::Closed {
        Err(ControlError::Closed)
    } else {
        Ok(())
    }
}

fn reserve(
    used: &mut u64,
    limit: u64,
    delta: u64,
    error: ControlError,
) -> Result<(), ControlError> {
    let next = used.checked_add(delta).ok_or(error.clone())?;
    if next > limit {
        return Err(error);
    }
    *used = next;
    Ok(())
}

fn set_failure_terminal(state: &mut EnvelopeState, terminal: Terminal) {
    if !matches!(state.terminal, Some(Terminal::CleanupFailure { .. })) {
        state.terminal = Some(terminal);
    }
}

fn validate_evidence_name(value: &str) -> Result<(), ControlError> {
    if value.is_empty()
        || value.contains(['/', '\\', '\n', '\r'])
        || value.chars().any(|character| character.is_control())
    {
        return Err(ControlError::InvalidName {
            value: value.into(),
        });
    }
    Ok(())
}

fn validate_media_type(value: &str) -> Result<(), ControlError> {
    let mut parts = value.split('/');
    let Some(major) = parts.next() else {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    };
    let Some(minor) = parts.next() else {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    };
    if parts.next().is_some()
        || major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(is_media_token)
        || !minor.bytes().all(is_media_token)
    {
        return Err(ControlError::InvalidMediaType {
            value: value.into(),
        });
    }
    Ok(())
}

fn is_media_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Production code cannot admit an intrinsic test operation.
pub fn admit_operation(
    source_class: TestSourceClass,
    operation: &'static str,
) -> Result<(), ControlError> {
    if source_class == TestSourceClass::Production {
        Err(ControlError::ProductionOperation { operation })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> EnvelopeLimits {
        EnvelopeLimits::new(1_000, 1_000, 1_000)
    }

    #[test]
    fn logs_tags_streams_and_report_are_isolated_and_ordered() {
        let envelope = EnvelopeHandle::new("hidden-node", limits());
        envelope.log("one").unwrap();
        envelope.log("two").unwrap();
        envelope
            .tags(BTreeMap::from([("suite".into(), "unit".into())]))
            .unwrap();
        envelope.stdout("out").unwrap();
        envelope.stderr("err").unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(
            report
                .logs()
                .iter()
                .map(LogRecord::sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(report.tags().get("suite"), Some(&"unit".into()));
        assert_eq!(report.stdout(), "out");
        assert_eq!(report.stderr(), "err");
    }

    #[test]
    fn tags_merge_atomically_and_choose_the_lexicographically_first_conflict() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .tags(BTreeMap::from([
                ("b".into(), "old".into()),
                ("z".into(), "old".into()),
            ]))
            .unwrap();
        let error = envelope
            .tags(BTreeMap::from([
                ("a".into(), "new".into()),
                ("b".into(), "different".into()),
                ("z".into(), "different".into()),
            ]))
            .unwrap_err();
        assert_eq!(error, ControlError::TagConflict { key: "b".into() });
        let report = envelope.report().unwrap();
        assert!(!report.tags().contains_key("a"));
        assert_eq!(report.tags().get("b"), Some(&"old".into()));
    }

    #[test]
    fn duplicate_equal_tags_are_idempotent_and_budgeted_once() {
        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(2, 0, 0));
        envelope
            .tags(BTreeMap::from([("a".into(), "b".into())]))
            .unwrap();
        envelope
            .tags(BTreeMap::from([("a".into(), "b".into())]))
            .unwrap();
        assert_eq!(
            envelope.tags(BTreeMap::from([("c".into(), "d".into())])),
            Err(ControlError::OutputLimit)
        );
    }

    #[test]
    fn fail_now_and_cleanup_failure_have_deterministic_precedence() {
        let envelope = EnvelopeHandle::new("node", limits());
        assert_eq!(
            envelope.skip("not applicable"),
            Err(ControlError::Skip {
                reason: "not applicable".into()
            })
        );
        envelope.set_phase(ExecutionPhase::Cleanup).unwrap();
        assert_eq!(envelope.skip("late"), Err(ControlError::SkipDuringCleanup));
        assert_eq!(
            envelope.cleanup_failure("P0007", "cleanup panic"),
            Err(ControlError::CleanupFailure {
                code: "P0007".into(),
                message: "cleanup panic".into()
            })
        );
        assert!(matches!(
            envelope.report().unwrap().terminal(),
            Some(Terminal::CleanupFailure { .. })
        ));

        let envelope = EnvelopeHandle::new("node", limits());
        assert_eq!(
            envelope.fail_now("stop"),
            Err(ControlError::FailNow {
                message: "stop".into()
            })
        );
        assert_eq!(
            envelope.report().unwrap().terminal().unwrap(),
            &Terminal::FailNow {
                code: "P0007",
                message: "stop".into()
            }
        );
    }

    #[test]
    fn artifacts_validate_names_media_and_atomic_limits() {
        let envelope = EnvelopeHandle::new("node", EnvelopeLimits::new(0, 3, 0));
        envelope.attach("a", "text/plain", b"abc".to_vec()).unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(report.artifacts()[0].sha256(), sha256(b"abc"));
        assert_eq!(
            envelope.attach("a", "text/plain", b"abc".to_vec()),
            Err(ControlError::ArtifactConflict { name: "a".into() })
        );
        assert_eq!(
            envelope.attach("b", "text/plain", b"x".to_vec()),
            Err(ControlError::ArtifactLimit)
        );
        assert_eq!(
            envelope.attach("bad/name", "text/plain", Vec::new()),
            Err(ControlError::InvalidName {
                value: "bad/name".into()
            })
        );
        assert_eq!(
            envelope.attach("bad", "plain", Vec::new()),
            Err(ControlError::InvalidMediaType {
                value: "plain".into()
            })
        );
    }

    #[test]
    fn snapshots_record_match_missing_mismatch_and_duplicate() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .with_expected_snapshots(BTreeMap::from([
                ("ok".into(), "value".into()),
                ("bad".into(), "expected".into()),
            ]))
            .unwrap();
        assert!(matches!(
            envelope.snapshot("ok", "value"),
            Ok(SnapshotOutcome::Matched { .. })
        ));
        assert_eq!(
            envelope.snapshot("bad", "actual"),
            Err(ControlError::SnapshotMismatch { name: "bad".into() })
        );
        assert_eq!(
            envelope.snapshot("missing", "new"),
            Err(ControlError::SnapshotMismatch {
                name: "missing".into()
            })
        );
        assert_eq!(
            envelope.snapshot("ok", "value"),
            Err(ControlError::SnapshotConflict { name: "ok".into() })
        );
        assert_eq!(envelope.report().unwrap().snapshots().len(), 3);
    }

    #[test]
    fn virtual_time_is_single_domain_and_revoked_after_closure() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .with_virtual_time(|time| {
                time.advance(10)?;
                time.settle()?;
                Ok(())
            })
            .unwrap();
        assert_eq!(envelope.report().unwrap().virtual_time().len(), 1);
        assert_eq!(
            envelope.with_virtual_time(|_| envelope.with_virtual_time(|_| Ok(()))),
            Err(ControlError::VirtualTimeActive)
        );
        assert_eq!(
            envelope.with_virtual_time(|time| time.advance(-1)),
            Err(ControlError::InvalidDuration)
        );
    }

    #[test]
    fn structured_child_shares_sinks_but_not_a_visible_context() {
        let envelope = EnvelopeHandle::new("node", limits());
        let child = envelope.child().unwrap();
        child.log("child").unwrap();
        child
            .tags(BTreeMap::from([("k".into(), "v".into())]))
            .unwrap();
        assert_eq!(envelope.report().unwrap().logs()[0].message(), "child");
        assert_eq!(
            envelope.report().unwrap().tags().get("k"),
            Some(&"v".into())
        );
        assert_eq!(
            child.skip("cancel"),
            Err(ControlError::Skip {
                reason: "structured child requested skip".into()
            })
        );
    }

    #[test]
    fn first_child_skip_wins_independent_of_completion_order() {
        let envelope = EnvelopeHandle::new("node", limits());
        let first = envelope.child().unwrap();
        let second = envelope.child().unwrap();
        second.skip("second").unwrap_err();
        first.skip("first").unwrap_err();
        assert_eq!(
            envelope.report().unwrap().terminal(),
            Some(&Terminal::Skipped {
                reason: "first".into()
            })
        );
    }

    #[test]
    fn phase_and_close_boundaries_are_strict() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.set_phase(ExecutionPhase::Setup).unwrap();
        assert_eq!(envelope.set_phase(ExecutionPhase::Body), Ok(()));
        assert_eq!(
            envelope.set_phase(ExecutionPhase::Setup),
            Err(ControlError::PhaseRegression {
                current: ExecutionPhase::Body,
                requested: ExecutionPhase::Setup
            })
        );
        envelope.close().unwrap();
        assert_eq!(envelope.log("late"), Err(ControlError::Closed));
        assert_eq!(envelope.close(), Err(ControlError::Closed));
    }

    #[test]
    fn production_intrinsics_are_rejected_at_the_admission_boundary() {
        assert_eq!(admit_operation(TestSourceClass::UnitTest, "log"), Ok(()));
        assert_eq!(
            admit_operation(TestSourceClass::Production, "log"),
            Err(ControlError::ProductionOperation { operation: "log" })
        );
    }

    #[test]
    fn evidence_name_and_media_type_grammar_is_closed() {
        assert!(validate_evidence_name("résumé").is_ok());
        assert!(validate_evidence_name("line\n").is_err());
        assert!(validate_media_type("application/json").is_ok());
        assert!(validate_media_type("text/plain; charset=utf-8").is_err());
        assert!(validate_media_type("/plain").is_err());
        assert!(validate_media_type("text/plain/extra").is_err());
    }

    #[test]
    fn report_orders_maps_without_reordering_observed_logs() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.attach("z", "text/plain", b"z".to_vec()).unwrap();
        envelope.attach("a", "text/plain", b"a".to_vec()).unwrap();
        envelope.log("first").unwrap();
        let report = envelope.report().unwrap();
        assert_eq!(
            report
                .artifacts()
                .iter()
                .map(|item| item.name())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(report.logs()[0].message(), "first");
    }

    #[test]
    fn expected_snapshots_install_only_before_first_check() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.with_expected_snapshots(BTreeMap::new()).unwrap();
        assert_eq!(
            envelope.with_expected_snapshots(BTreeMap::new()),
            Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into()
            })
        );
    }

    #[test]
    fn expected_snapshot_installation_closes_after_a_snapshot_check() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope.snapshot("first", "actual").unwrap_err();
        assert_eq!(
            envelope.with_expected_snapshots(BTreeMap::new()),
            Err(ControlError::SnapshotConflict {
                name: "expected-snapshots-already-installed".into()
            })
        );
    }

    #[test]
    fn hash_validation_helper_is_used_for_artifact_identity() {
        let envelope = EnvelopeHandle::new("node", limits());
        envelope
            .attach("x", "application/octet-stream", b"data".to_vec())
            .unwrap();
        crate::artifact::validate_sha256(envelope.report().unwrap().artifacts()[0].sha256())
            .unwrap();
    }
}
