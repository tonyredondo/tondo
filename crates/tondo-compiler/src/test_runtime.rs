//! Isolated leaf-worker runtime for the test runner.
//!
//! This is the host-facing orchestration boundary, not a second compiler
//! frontend.  Every leaf receives a fresh bootstrap, envelope, resource
//! registry and worker identity.  A program may be run again, but no heap,
//! root, task, handle, output buffer or envelope is reused between runs.

#![allow(clippy::large_enum_variant, clippy::result_large_err)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::test_control::{
    ControlError, EnvelopeHandle, EnvelopeLimits, EnvelopeReport, ExecutionPhase, StructuredTask,
    Terminal, VirtualTime,
};

pub const TEST_RUNTIME_FORMAT: &str = "tondo-test-runtime-draft/1";

/// Runtime status projected into the result model for one leaf attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeStatus {
    Passed,
    Skipped,
    FailedError,
    FailedPanic,
    ResourceLimit,
    Timeout,
    Infrastructure,
    BlockedSetup,
}

/// The clock provider is selected once at the worker boundary. User bytecode
/// continues to call the same time intrinsics for either provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClockProvider {
    Monotonic,
    Virtual,
}

/// Immutable worker identity. IDs are fresh for every bootstrap and are not
/// copied into a Tondo value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerInfo {
    worker_id: u64,
    heap_id: u64,
    executor_id: u64,
    environment_empty: bool,
    clock: ClockProvider,
}

impl WorkerInfo {
    pub const fn worker_id(self) -> u64 {
        self.worker_id
    }

    pub const fn heap_id(self) -> u64 {
        self.heap_id
    }

    pub const fn executor_id(self) -> u64 {
        self.executor_id
    }

    pub const fn environment_empty(self) -> bool {
        self.environment_empty
    }

    pub const fn clock(self) -> ClockProvider {
        self.clock
    }
}

/// A host resource owned by one worker. Its ID is revoked when the worker is
/// torn down, even if a stale token is held by another Rust object.
pub struct ResourceHandle {
    id: u64,
    worker_id: u64,
    worker: Arc<WorkerState>,
}

impl std::fmt::Debug for ResourceHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResourceHandle")
            .field("id", &self.id)
            .field("worker_id", &self.worker_id)
            .finish_non_exhaustive()
    }
}

impl ResourceHandle {
    pub const fn id(&self) -> u64 {
        self.id
    }

    pub const fn worker_id(&self) -> u64 {
        self.worker_id
    }
}

impl Drop for ResourceHandle {
    fn drop(&mut self) {
        self.worker.release_resource(self.id);
    }
}

/// Errors returned by a leaf body or by worker setup/cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    Error { code: String, message: String },
    Panic { code: String, message: String },
    ResourceLimit { kind: String },
    Timeout,
    Infrastructure { message: String },
    Skip { reason: String },
    ForcedTermination { message: String },
    Control(ControlError),
}

impl RunError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Error { code, .. } | Self::Panic { code, .. } => Some(code),
            Self::Control(error) => Some(error.code()),
            Self::ResourceLimit { .. }
            | Self::Timeout
            | Self::Infrastructure { .. }
            | Self::Skip { .. }
            | Self::ForcedTermination { .. } => None,
        }
    }

    fn from_control(error: ControlError) -> Self {
        let code = error.code();
        match error {
            ControlError::Skip { reason } => Self::Skip { reason },
            ControlError::OutputLimit
            | ControlError::ArtifactLimit
            | ControlError::SnapshotLimit => Self::ResourceLimit {
                kind: error.code().into(),
            },
            ControlError::FailNow { message } => Self::Panic {
                code: code.into(),
                message,
            },
            ControlError::CleanupFailure { code, message } => Self::Panic { code, message },
            ControlError::SkipDuringCleanup
            | ControlError::SnapshotMismatch { .. }
            | ControlError::SnapshotConflict { .. }
            | ControlError::ArtifactConflict { .. }
            | ControlError::TagConflict { .. }
            | ControlError::InvalidName { .. }
            | ControlError::InvalidMediaType { .. }
            | ControlError::InvalidDuration
            | ControlError::VirtualDeadlock
            | ControlError::VirtualExternalWait { .. }
            | ControlError::VirtualLivelock { .. }
            | ControlError::VirtualClockRegression
            | ControlError::VirtualOverflow
            | ControlError::VirtualTask { .. } => Self::Panic {
                code: code.into(),
                message: error.to_string(),
            },
            other => Self::Control(other),
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error { code, message } => write!(formatter, "{code}: {message}"),
            Self::Panic { code, message } => write!(formatter, "{code}: {message}"),
            Self::ResourceLimit { kind } => write!(formatter, "resource limit: {kind}"),
            Self::Timeout => formatter.write_str("test worker timed out"),
            Self::Infrastructure { message } => write!(formatter, "infrastructure: {message}"),
            Self::Skip { reason } => write!(formatter, "skipped: {reason}"),
            Self::ForcedTermination { message } => {
                write!(formatter, "forced termination: {message}")
            }
            Self::Control(error) => error.fmt(formatter),
        }
    }
}

impl Error for RunError {}

/// Runtime configuration shared by every fresh worker in one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeConfig {
    jobs: usize,
    envelope_limits: EnvelopeLimits,
    max_resource_handles: usize,
    clock: ClockProvider,
    catch_panics: bool,
}

impl RuntimeConfig {
    pub fn new(jobs: usize, envelope_limits: EnvelopeLimits) -> Result<Self, RuntimeConfigError> {
        if jobs == 0 {
            return Err(RuntimeConfigError::ZeroJobs);
        }
        Ok(Self {
            jobs,
            envelope_limits,
            max_resource_handles: 1_024,
            clock: ClockProvider::Monotonic,
            catch_panics: true,
        })
    }

    pub const fn jobs(self) -> usize {
        self.jobs
    }

    pub const fn envelope_limits(self) -> EnvelopeLimits {
        self.envelope_limits
    }

    pub const fn max_resource_handles(self) -> usize {
        self.max_resource_handles
    }

    pub const fn clock(self) -> ClockProvider {
        self.clock
    }

    pub const fn catch_panics(self) -> bool {
        self.catch_panics
    }

    pub const fn with_max_resource_handles(mut self, max: usize) -> Self {
        self.max_resource_handles = max;
        self
    }

    pub const fn with_clock(mut self, clock: ClockProvider) -> Self {
        self.clock = clock;
        self
    }

    pub const fn with_catch_panics(mut self, catch: bool) -> Self {
        self.catch_panics = catch;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeConfigError {
    ZeroJobs,
    ZeroResourceHandles,
}

impl std::fmt::Display for RuntimeConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroJobs => formatter.write_str("runtime jobs must be greater than zero"),
            Self::ZeroResourceHandles => {
                formatter.write_str("runtime resource-handle budget must be greater than zero")
            }
        }
    }
}

impl Error for RuntimeConfigError {}

/// A test leaf body. The `Arc` makes retry/repeat possible without reusing a
/// worker or envelope.
pub struct LeafProgram {
    id: String,
    expected_snapshots: BTreeMap<String, String>,
    body: Arc<LeafBody>,
}

type LeafBody = dyn Fn(&WorkerContext) -> Result<(), RunError> + Send + Sync;

impl std::fmt::Debug for LeafProgram {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LeafProgram")
            .field("id", &self.id)
            .field("expected_snapshots", &self.expected_snapshots)
            .finish_non_exhaustive()
    }
}

impl Clone for LeafProgram {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            expected_snapshots: self.expected_snapshots.clone(),
            body: self.body.clone(),
        }
    }
}

impl LeafProgram {
    pub fn new(
        id: impl Into<String>,
        body: impl Fn(&WorkerContext) -> Result<(), RunError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            expected_snapshots: BTreeMap::new(),
            body: Arc::new(body),
        }
    }

    pub fn with_expected_snapshots(
        mut self,
        snapshots: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.expected_snapshots = snapshots.into_iter().collect();
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Public view passed to a leaf body. It forwards only sealed operations and
/// opaque worker allocation; there is no access to another worker or heap.
#[derive(Clone)]
pub struct WorkerContext {
    worker: Arc<WorkerState>,
}

impl std::fmt::Debug for WorkerContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerContext")
            .field("worker", &self.worker.info)
            .finish_non_exhaustive()
    }
}

impl WorkerContext {
    pub fn worker(&self) -> WorkerInfo {
        self.worker.info
    }

    pub fn log(&self, message: impl Into<String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .log(message)
            .map_err(RunError::from_control)
    }

    pub fn tags(&self, values: BTreeMap<String, String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .tags(values)
            .map_err(RunError::from_control)
    }

    pub fn stdout(&self, text: impl Into<String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .stdout(text)
            .map_err(RunError::from_control)
    }

    pub fn stderr(&self, text: impl Into<String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .stderr(text)
            .map_err(RunError::from_control)
    }

    pub fn fail_now(&self, message: impl Into<String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .fail_now(message)
            .map_err(RunError::from_control)
    }

    pub fn skip(&self, reason: impl Into<String>) -> Result<(), RunError> {
        self.worker
            .envelope
            .skip(reason)
            .map_err(RunError::from_control)
    }

    pub fn attach(
        &self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), RunError> {
        self.worker
            .envelope
            .attach(name, media_type, bytes)
            .map_err(RunError::from_control)
    }

    pub fn snapshot(
        &self,
        name: impl Into<String>,
        actual: impl AsRef<str>,
    ) -> Result<(), RunError> {
        self.worker
            .envelope
            .snapshot(name, actual)
            .map(|_| ())
            .map_err(RunError::from_control)
    }

    pub fn with_virtual_time<T>(
        &self,
        operation: impl FnOnce(&VirtualTime<'_>) -> Result<T, ControlError>,
    ) -> Result<T, RunError> {
        self.worker
            .envelope
            .with_virtual_time(operation)
            .map_err(RunError::from_control)
    }

    pub fn child(&self) -> Result<StructuredTask, RunError> {
        self.worker.envelope.child().map_err(RunError::from_control)
    }

    pub fn allocate_resource(&self) -> Result<ResourceHandle, RunError> {
        self.worker.allocate_resource()
    }

    pub fn defer(
        &self,
        cleanup: impl FnOnce(&WorkerContext) -> Result<(), RunError> + Send + 'static,
    ) -> Result<(), RunError> {
        self.worker
            .cleanup
            .lock()
            .map_err(|_| RunError::Infrastructure {
                message: "cleanup registry is poisoned".into(),
            })?
            .push(Box::new(cleanup));
        Ok(())
    }

    pub fn phase(&self) -> Result<ExecutionPhase, RunError> {
        self.worker.envelope.phase().map_err(RunError::from_control)
    }
}

type CleanupFn = Box<dyn FnOnce(&WorkerContext) -> Result<(), RunError> + Send>;

#[derive(Debug)]
struct ResourceRegistry {
    next: AtomicU64,
    active: Mutex<BTreeSet<u64>>,
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self {
            next: AtomicU64::new(1),
            active: Mutex::new(BTreeSet::new()),
        }
    }
}

impl ResourceRegistry {
    fn allocate(&self) -> Result<u64, RunError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.active
            .lock()
            .map_err(|_| RunError::Infrastructure {
                message: "resource registry is poisoned".into(),
            })?
            .insert(id);
        Ok(id)
    }

    fn release(&self, id: u64) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&id);
        }
    }

    fn count(&self) -> usize {
        self.active.lock().map_or(usize::MAX, |active| active.len())
    }
}

struct WorkerState {
    info: WorkerInfo,
    envelope: EnvelopeHandle,
    registry: Arc<ResourceRegistry>,
    resources: Mutex<BTreeSet<u64>>,
    max_resources: usize,
    revoked: AtomicBool,
    cleanup: Mutex<Vec<CleanupFn>>,
}

impl WorkerState {
    fn allocate_resource(self: &Arc<Self>) -> Result<ResourceHandle, RunError> {
        if self.revoked.load(Ordering::Acquire) {
            return Err(RunError::Infrastructure {
                message: "worker resources have been revoked".into(),
            });
        }
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| RunError::Infrastructure {
                message: "worker resource ledger is poisoned".into(),
            })?;
        if resources.len() >= self.max_resources {
            return Err(RunError::ResourceLimit {
                kind: "worker-resource-handles".into(),
            });
        }
        let id = self.registry.allocate()?;
        resources.insert(id);
        Ok(ResourceHandle {
            id,
            worker_id: self.info.worker_id,
            worker: self.clone(),
        })
    }

    fn release_resource(&self, id: u64) {
        if let Ok(mut resources) = self.resources.lock()
            && resources.remove(&id)
        {
            self.registry.release(id);
        }
    }

    fn revoke(&self) {
        if self.revoked.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut resources) = self.resources.lock() {
            for id in std::mem::take(&mut *resources) {
                self.registry.release(id);
            }
        }
    }
}

/// One-phase worker bootstrap/revocation protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapPhase {
    Fresh,
    Initialized,
    Revoked,
}

pub struct WorkerBootstrap {
    state: Arc<WorkerState>,
    phase: BootstrapPhase,
}

impl std::fmt::Debug for WorkerBootstrap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerBootstrap")
            .field("worker", &self.state.info)
            .field("phase", &self.phase)
            .finish()
    }
}

impl WorkerBootstrap {
    fn new(registry: Arc<ResourceRegistry>, config: RuntimeConfig, serial: u64) -> Self {
        let worker_id = serial.saturating_mul(3).saturating_add(1);
        let info = WorkerInfo {
            worker_id,
            heap_id: worker_id.saturating_add(1),
            executor_id: worker_id.saturating_add(2),
            environment_empty: true,
            clock: config.clock,
        };
        let state = Arc::new(WorkerState {
            info,
            envelope: EnvelopeHandle::new(format!("worker-{worker_id}"), config.envelope_limits),
            registry,
            resources: Mutex::new(BTreeSet::new()),
            max_resources: config.max_resource_handles,
            revoked: AtomicBool::new(false),
            cleanup: Mutex::new(Vec::new()),
        });
        Self {
            state,
            phase: BootstrapPhase::Fresh,
        }
    }

    pub fn phase(&self) -> BootstrapPhase {
        self.phase
    }

    pub fn info(&self) -> WorkerInfo {
        self.state.info
    }

    pub fn initialize(&mut self) -> Result<WorkerContext, RunError> {
        if self.phase != BootstrapPhase::Fresh {
            return Err(RunError::Infrastructure {
                message: "worker bootstrap was initialized twice".into(),
            });
        }
        self.phase = BootstrapPhase::Initialized;
        Ok(WorkerContext {
            worker: self.state.clone(),
        })
    }

    pub fn revoke(&mut self) -> Result<(), RunError> {
        if self.phase == BootstrapPhase::Revoked {
            return Err(RunError::Infrastructure {
                message: "worker bootstrap was revoked twice".into(),
            });
        }
        self.state.revoke();
        self.phase = BootstrapPhase::Revoked;
        Ok(())
    }
}

/// Result of one leaf execution. Evidence is detached from the worker before
/// the worker is revoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafResult {
    id: String,
    status: RuntimeStatus,
    worker: WorkerInfo,
    report: EnvelopeReport,
    error: Option<RunError>,
    cleanup_executed: bool,
    forced_termination: bool,
}

impl LeafResult {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn status(&self) -> RuntimeStatus {
        self.status
    }

    pub const fn worker(&self) -> WorkerInfo {
        self.worker
    }

    pub fn report(&self) -> &EnvelopeReport {
        &self.report
    }

    pub fn error(&self) -> Option<&RunError> {
        self.error.as_ref()
    }

    pub const fn cleanup_executed(&self) -> bool {
        self.cleanup_executed
    }

    pub const fn forced_termination(&self) -> bool {
        self.forced_termination
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    DuplicateLeaf(String),
    EmptyLeafId,
    InvalidConfig(RuntimeConfigError),
    WorkerJoin,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateLeaf(id) => write!(formatter, "leaf `{id}` is duplicated"),
            Self::EmptyLeafId => formatter.write_str("leaf identity cannot be empty"),
            Self::InvalidConfig(error) => error.fmt(formatter),
            Self::WorkerJoin => formatter.write_str("worker thread could not be joined"),
        }
    }
}

impl Error for RuntimeError {}

/// Deterministically ordered report for a runtime invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReport {
    leaves: Vec<LeafResult>,
    active_resources: usize,
}

impl RuntimeReport {
    pub fn leaves(&self) -> &[LeafResult] {
        &self.leaves
    }

    pub const fn active_resources(&self) -> usize {
        self.active_resources
    }
}

pub struct RuntimeRunner {
    config: RuntimeConfig,
    serial: AtomicU64,
}

impl std::fmt::Debug for RuntimeRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeRunner")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl RuntimeRunner {
    pub fn new(config: RuntimeConfig) -> Result<Self, RuntimeConfigError> {
        if config.max_resource_handles == 0 {
            return Err(RuntimeConfigError::ZeroResourceHandles);
        }
        Ok(Self {
            config,
            serial: AtomicU64::new(0),
        })
    }

    pub const fn config(&self) -> RuntimeConfig {
        self.config
    }

    pub fn run(&self, programs: Vec<LeafProgram>) -> Result<RuntimeReport, RuntimeError> {
        let mut seen = BTreeSet::new();
        for program in &programs {
            if program.id.trim().is_empty() {
                return Err(RuntimeError::EmptyLeafId);
            }
            if !seen.insert(program.id.clone()) {
                return Err(RuntimeError::DuplicateLeaf(program.id.clone()));
            }
        }
        let registry = Arc::new(ResourceRegistry::default());
        let config = self.config;
        let mut results = Vec::with_capacity(programs.len());
        thread::scope(|scope| {
            let mut pending: Vec<thread::ScopedJoinHandle<'_, LeafResult>> =
                Vec::with_capacity(config.jobs);
            for program in programs {
                if pending.len() == config.jobs {
                    for handle in pending.drain(..) {
                        results.push(handle.join().map_err(|_| RuntimeError::WorkerJoin)?);
                    }
                }
                let registry = registry.clone();
                let serial = self.serial.fetch_add(1, Ordering::Relaxed);
                pending.push(scope.spawn(move || run_leaf(program, config, registry, serial)));
            }
            for handle in pending {
                results.push(handle.join().map_err(|_| RuntimeError::WorkerJoin)?);
            }
            Ok::<(), RuntimeError>(())
        })?;
        results.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(RuntimeReport {
            leaves: results,
            active_resources: registry.count(),
        })
    }
}

fn run_leaf(
    program: LeafProgram,
    config: RuntimeConfig,
    registry: Arc<ResourceRegistry>,
    serial: u64,
) -> LeafResult {
    let mut bootstrap = WorkerBootstrap::new(registry, config, serial);
    let worker = bootstrap.info();
    let context = match bootstrap.initialize() {
        Ok(context) => context,
        Err(error) => {
            return LeafResult {
                id: program.id,
                status: RuntimeStatus::Infrastructure,
                worker,
                report: empty_report(),
                error: Some(error),
                cleanup_executed: false,
                forced_termination: false,
            };
        }
    };
    if let Err(error) = context
        .worker
        .envelope
        .with_expected_snapshots(program.expected_snapshots.clone())
        .map_err(RunError::from_control)
    {
        let _ = bootstrap.revoke();
        return LeafResult {
            id: program.id,
            status: RuntimeStatus::Infrastructure,
            worker,
            report: empty_report(),
            error: Some(error),
            cleanup_executed: false,
            forced_termination: false,
        };
    }
    let _ = context.worker.envelope.set_phase(ExecutionPhase::Body);
    let body_result = invoke_body(&program, &context, config.catch_panics);
    let forced = matches!(body_result, Err(RunError::ForcedTermination { .. }));
    let mut cleanup_executed = false;
    let mut final_error = body_result.err();
    if !forced {
        let _ = context.worker.envelope.set_phase(ExecutionPhase::Cleanup);
        let cleanup_error = run_cleanup(&context, config.catch_panics);
        cleanup_executed = true;
        if cleanup_error.is_some() {
            final_error = cleanup_error;
        }
    }
    let _ = context.worker.envelope.close();
    let report = context
        .worker
        .envelope
        .report()
        .unwrap_or_else(|_| empty_report());
    let status = classify(&final_error, report.terminal(), forced);
    let _ = bootstrap.revoke();
    LeafResult {
        id: program.id,
        status,
        worker,
        report,
        error: final_error,
        cleanup_executed,
        forced_termination: forced,
    }
}

fn invoke_body(
    program: &LeafProgram,
    context: &WorkerContext,
    catch_panics: bool,
) -> Result<(), RunError> {
    if catch_panics {
        match catch_unwind(AssertUnwindSafe(|| (program.body)(context))) {
            Ok(result) => result,
            Err(payload) => Err(RunError::Panic {
                code: "P0007".into(),
                message: panic_message(payload),
            }),
        }
    } else {
        (program.body)(context)
    }
}

fn run_cleanup(context: &WorkerContext, catch_panics: bool) -> Option<RunError> {
    loop {
        let cleanup = match context.worker.cleanup.lock() {
            Ok(mut cleanups) => cleanups.pop(),
            Err(_) => {
                return Some(RunError::Infrastructure {
                    message: "cleanup registry is poisoned".into(),
                });
            }
        };
        let cleanup = cleanup?;
        let result = if catch_panics {
            match catch_unwind(AssertUnwindSafe(|| cleanup(context))) {
                Ok(result) => result,
                Err(payload) => Err(RunError::Panic {
                    code: "P0007".into(),
                    message: panic_message(payload),
                }),
            }
        } else {
            cleanup(context)
        };
        if let Err(error) = result {
            return Some(error);
        }
    }
}

fn classify(error: &Option<RunError>, terminal: Option<&Terminal>, forced: bool) -> RuntimeStatus {
    if forced {
        return RuntimeStatus::Timeout;
    }
    if let Some(error) = error {
        return match error {
            RunError::Skip { .. } => RuntimeStatus::Skipped,
            RunError::ResourceLimit { .. } => RuntimeStatus::ResourceLimit,
            RunError::Timeout | RunError::ForcedTermination { .. } => RuntimeStatus::Timeout,
            RunError::Panic { .. }
            | RunError::Control(ControlError::FailNow { .. })
            | RunError::Control(ControlError::SnapshotMismatch { .. })
            | RunError::Control(ControlError::SnapshotConflict { .. })
            | RunError::Control(ControlError::TagConflict { .. })
            | RunError::Control(ControlError::CleanupFailure { .. }) => RuntimeStatus::FailedPanic,
            RunError::Error { .. } => RuntimeStatus::FailedError,
            RunError::Infrastructure { .. } | RunError::Control(_) => RuntimeStatus::Infrastructure,
        };
    }
    match terminal {
        Some(Terminal::Skipped { .. }) => RuntimeStatus::Skipped,
        Some(Terminal::FailNow { .. }) | Some(Terminal::CleanupFailure { .. }) => {
            RuntimeStatus::FailedPanic
        }
        Some(Terminal::ResourceLimit { .. }) => RuntimeStatus::ResourceLimit,
        None => RuntimeStatus::Passed,
    }
}

fn empty_report() -> EnvelopeReport {
    let envelope = EnvelopeHandle::new("empty", EnvelopeLimits::new(0, 0, 0));
    envelope.close().expect("fresh empty envelope closes");
    envelope.report().expect("closed envelope can be reported")
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload is not displayable".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> RuntimeConfig {
        RuntimeConfig::new(2, EnvelopeLimits::new(1_000, 1_000, 1_000)).unwrap()
    }

    #[test]
    fn every_leaf_gets_a_fresh_worker_heap_executor_and_empty_environment() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let first = LeafProgram::new("b", |context| {
            assert!(context.worker().environment_empty());
            context.log("first")
        });
        let second = LeafProgram::new("a", |context| {
            assert!(context.worker().environment_empty());
            context.log("second")
        });
        let report = runner.run(vec![first, second]).unwrap();
        assert_eq!(
            report
                .leaves()
                .iter()
                .map(LeafResult::id)
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_ne!(report.leaves()[0].worker(), report.leaves()[1].worker());
        assert_eq!(report.active_resources(), 0);
    }

    #[test]
    fn statuses_cover_return_skip_error_panic_resource_timeout_and_infrastructure() {
        let programs = vec![
            LeafProgram::new("passed", |_| Ok(())),
            LeafProgram::new("skipped", |context| context.skip("not applicable")),
            LeafProgram::new("error", |_| {
                Err(RunError::Error {
                    code: "E1".into(),
                    message: "expected error".into(),
                })
            }),
            LeafProgram::new("panic", |_| panic!("boom")),
            LeafProgram::new("timeout", |_| {
                Err(RunError::ForcedTermination {
                    message: "deadline".into(),
                })
            }),
            LeafProgram::new("resource", |context| {
                Err(context
                    .allocate_resource()
                    .err()
                    .unwrap_or(RunError::Infrastructure {
                        message: "unexpected allocation".into(),
                    }))
            }),
        ];
        let mut config = config();
        config.max_resource_handles = 0;
        let runner = RuntimeRunner {
            config,
            serial: AtomicU64::new(0),
        };
        let report = runner.run(programs).unwrap();
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "passed")
                .unwrap()
                .status(),
            RuntimeStatus::Passed
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "skipped")
                .unwrap()
                .status(),
            RuntimeStatus::Skipped
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "error")
                .unwrap()
                .status(),
            RuntimeStatus::FailedError
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "panic")
                .unwrap()
                .status(),
            RuntimeStatus::FailedPanic
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "timeout")
                .unwrap()
                .status(),
            RuntimeStatus::Timeout
        );
    }

    #[test]
    fn resources_are_revoked_even_when_a_stale_handle_survives_the_worker() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let stale = Arc::new(Mutex::new(None));
        let captured = stale.clone();
        let program = LeafProgram::new("resource", move |context| {
            let handle = context.allocate_resource()?;
            *captured.lock().unwrap() = Some(handle);
            Ok(())
        });
        let report = runner.run(vec![program]).unwrap();
        assert_eq!(report.active_resources(), 0);
        assert_eq!(
            stale.lock().unwrap().as_ref().unwrap().worker_id(),
            report.leaves()[0].worker().worker_id()
        );
    }

    #[test]
    fn cleanup_runs_lifo_after_error_and_is_skipped_for_forced_termination() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let first_order = order.clone();
        let second_source = order.clone();
        let first = LeafProgram::new("cleanup", move |context| {
            let first_order = first_order.clone();
            context.defer(move |_| {
                first_order.lock().unwrap().push("first");
                Ok(())
            })?;
            let second_order = second_source.clone();
            context.defer(move |_| {
                second_order.lock().unwrap().push("second");
                Ok(())
            })?;
            Err(RunError::Error {
                code: "E".into(),
                message: "body".into(),
            })
        });
        let forced = LeafProgram::new("forced", |_| {
            Err(RunError::ForcedTermination {
                message: "abort".into(),
            })
        });
        let report = runner.run(vec![first, forced]).unwrap();
        assert_eq!(*order.lock().unwrap(), ["second", "first"]);
        assert!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "cleanup")
                .unwrap()
                .cleanup_executed()
        );
        assert!(
            !report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "forced")
                .unwrap()
                .cleanup_executed()
        );
        assert!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "forced")
                .unwrap()
                .forced_termination()
        );
    }

    #[test]
    fn panic_in_body_is_caught_and_siblings_continue() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let report = runner
            .run(vec![
                LeafProgram::new("panic", |_| panic!("bad")),
                LeafProgram::new("ok", |_| Ok(())),
            ])
            .unwrap();
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "panic")
                .unwrap()
                .status(),
            RuntimeStatus::FailedPanic
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "ok")
                .unwrap()
                .status(),
            RuntimeStatus::Passed
        );
    }

    #[test]
    fn control_evidence_and_virtual_time_cross_the_worker_boundary() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let program = LeafProgram::new("evidence", |context| {
            context.tags(BTreeMap::from([("kind".into(), "integration".into())]))?;
            context.attach("trace", "text/plain", b"trace".to_vec())?;
            context.with_virtual_time(|time| {
                time.advance(4)?;
                time.settle()
            })?;
            Ok(())
        });
        let report = runner.run(vec![program]).unwrap();
        let leaf = &report.leaves()[0];
        assert_eq!(leaf.status(), RuntimeStatus::Passed);
        assert_eq!(
            leaf.report().tags().get("kind"),
            Some(&"integration".into())
        );
        assert_eq!(leaf.report().artifacts().len(), 1);
        assert_eq!(leaf.report().virtual_time()[0].elapsed_ns(), 4);
    }

    #[test]
    fn snapshot_failure_and_cleanup_failure_have_normative_precedence() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let snapshot = LeafProgram::new("snapshot", |context| context.snapshot("golden", "actual"));
        let cleanup = LeafProgram::new("cleanup", |context| {
            context.defer(|context| context.fail_now("cleanup"))
        });
        let report = runner.run(vec![snapshot, cleanup]).unwrap();
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "snapshot")
                .unwrap()
                .status(),
            RuntimeStatus::FailedPanic
        );
        assert_eq!(
            report
                .leaves()
                .iter()
                .find(|leaf| leaf.id() == "cleanup")
                .unwrap()
                .status(),
            RuntimeStatus::FailedPanic
        );
    }

    #[test]
    fn duplicate_and_empty_leaf_ids_are_rejected_before_workers_start() {
        let runner = RuntimeRunner::new(config()).unwrap();
        assert_eq!(
            runner.run(vec![LeafProgram::new("", |_| Ok(()))]),
            Err(RuntimeError::EmptyLeafId)
        );
        assert_eq!(
            runner.run(vec![
                LeafProgram::new("same", |_| Ok(())),
                LeafProgram::new("same", |_| Ok(()))
            ]),
            Err(RuntimeError::DuplicateLeaf("same".into()))
        );
    }

    #[test]
    fn bootstrap_has_one_initialization_and_one_revocation_phase() {
        let config = config();
        let registry = Arc::new(ResourceRegistry::default());
        let mut bootstrap = WorkerBootstrap::new(registry, config, 77);
        assert_eq!(bootstrap.phase(), BootstrapPhase::Fresh);
        let context = bootstrap.initialize().unwrap();
        assert_eq!(bootstrap.phase(), BootstrapPhase::Initialized);
        assert_eq!(context.worker().clock(), ClockProvider::Monotonic);
        assert!(bootstrap.initialize().is_err());
        bootstrap.revoke().unwrap();
        assert_eq!(bootstrap.phase(), BootstrapPhase::Revoked);
        assert!(bootstrap.revoke().is_err());
    }

    #[test]
    fn virtual_clock_selection_is_worker_local_and_configurable() {
        let config = config().with_clock(ClockProvider::Virtual);
        let runner = RuntimeRunner::new(config).unwrap();
        let report = runner
            .run(vec![LeafProgram::new("clock", |context| {
                assert_eq!(context.worker().clock(), ClockProvider::Virtual);
                Ok(())
            })])
            .unwrap();
        assert_eq!(report.leaves()[0].status(), RuntimeStatus::Passed);
    }

    #[test]
    fn runtime_config_and_resource_limits_are_closed() {
        assert_eq!(
            RuntimeConfig::new(0, EnvelopeLimits::new(1, 1, 1)),
            Err(RuntimeConfigError::ZeroJobs)
        );
        let config = RuntimeConfig::new(1, EnvelopeLimits::new(1, 1, 1))
            .unwrap()
            .with_max_resource_handles(0);
        assert!(matches!(
            RuntimeRunner::new(config),
            Err(RuntimeConfigError::ZeroResourceHandles)
        ));
        assert_eq!(config.envelope_limits().output_bytes(), 1);
    }

    #[test]
    fn retrying_a_program_uses_a_new_worker_identity() {
        let runner = RuntimeRunner::new(config()).unwrap();
        let program = LeafProgram::new("retry", |context| {
            assert!(context.worker().environment_empty());
            Ok(())
        });
        let first = runner.run(vec![program.clone()]).unwrap();
        let second = runner.run(vec![program]).unwrap();
        assert_ne!(first.leaves()[0].worker(), second.leaves()[0].worker());
        assert_eq!(first.active_resources(), 0);
        assert_eq!(second.active_resources(), 0);
    }

    #[test]
    fn runtime_status_projection_handles_terminal_envelope_states() {
        assert_eq!(
            classify(
                &None,
                Some(&Terminal::Skipped { reason: "x".into() }),
                false
            ),
            RuntimeStatus::Skipped
        );
        assert_eq!(
            classify(
                &None,
                Some(&Terminal::FailNow {
                    code: "P0007",
                    message: "x".into()
                }),
                false
            ),
            RuntimeStatus::FailedPanic
        );
        assert_eq!(
            classify(
                &None,
                Some(&Terminal::ResourceLimit { kind: "bytes" }),
                false
            ),
            RuntimeStatus::ResourceLimit
        );
        assert_eq!(classify(&None, None, true), RuntimeStatus::Timeout);
    }

    #[test]
    fn run_error_codes_and_control_mapping_are_stable() {
        assert_eq!(
            RunError::from_control(ControlError::OutputLimit).code(),
            None
        );
        assert_eq!(
            RunError::from_control(ControlError::FailNow {
                message: "x".into()
            })
            .code(),
            Some("P0007")
        );
        assert!(matches!(
            RunError::from_control(ControlError::Skip { reason: "x".into() }),
            RunError::Skip { .. }
        ));
    }
}
