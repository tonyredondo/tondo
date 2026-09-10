//! Execution engine and managed object model for verified Tondo bytecode.
//!
//! The bootstrap VM keeps values explicit, uses typed frame slots, and owns a
//! precise non-moving tracing heap. Bytecode is verified again at this trust
//! boundary even when it originated in the reference compiler.

use std::error::Error;
use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::bytecode::{BytecodeSpan, BytecodeVerificationError};

mod diagnostics;
mod dump;
mod execute;
mod heap;
mod leak;
mod race;
mod value;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use diagnostics::{
    DIAGNOSTIC_SCHEMA, DiagnosticConfig, DiagnosticEvent, DiagnosticHeapOperation,
    DiagnosticMemoryAccess, DiagnosticQuiescencePhase, DiagnosticRange, DiagnosticResource,
    DiagnosticResourceState, DiagnosticRootSnapshot, DiagnosticSchedulerOperation,
    DiagnosticSource, DiagnosticSynchronization, DiagnosticTaskState, DiagnosticThreadState,
    DiagnosticTrace,
};
pub use dump::{
    DUMP_EXTENSION, DUMP_SCHEMA, DumpAnalysis, DumpArtifact, DumpError, DumpIdentity, DumpOptions,
    DumpSection, DumpTermination, MAX_DUMP_BYTES, analyze_dump, capture_dump,
};
pub use execute::host_import::{
    PreparedHostImport as VmHostImportReservation, VmHostImportAdmission,
};
pub use execute::{
    RejectingHost, VmExecution, VmHost, VmOutcome, VmTestNodeKind, VmTestNodeOutcome, execute,
    execute_with_arguments, execute_with_diagnostics, execute_with_limits,
    execute_with_limits_and_copy_strategy, execute_with_limits_and_copy_strategy_and_diagnostics,
    execute_with_owned_request,
};
pub use leak::{
    LEAK_SCHEMA, LeakConfig, LeakFinding, LeakKind, LeakLimitation, LeakObject, LeakReport,
    LeakResource, LeakSnapshot, LeakStatus, detect_leaks, detect_leaks_with_config,
};
pub use race::{
    RACE_SCHEMA, RaceAccess, RaceConfig, RaceFinding, RaceLimitation, RaceLocation, RaceReport,
    RaceStatus, detect_races, detect_races_with_config,
};

/// Physical strategy used to realize source-level logical value copies.
///
/// Both modes have identical Tondo semantics. `Eager` remains available as a
/// reference implementation for differential validation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ValueCopyStrategy {
    Eager,
    #[default]
    CopyOnWrite,
}

/// Defensive limits for one VM execution request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VmLimits {
    pub max_verification_steps: u64,
    pub max_steps: u64,
    pub max_stack_depth: u32,
    pub max_heap_objects: u32,
    pub max_heap_bytes: u64,
    pub initial_gc_threshold: u32,
}

impl VmLimits {
    /// Reject an invalid execution profile before admitting its inputs.
    pub fn validate(self) -> Result<(), VmError> {
        for (name, value) in [
            ("max_verification_steps", self.max_verification_steps),
            ("max_steps", self.max_steps),
            ("max_stack_depth", u64::from(self.max_stack_depth)),
            ("max_heap_objects", u64::from(self.max_heap_objects)),
            ("max_heap_bytes", self.max_heap_bytes),
            ("initial_gc_threshold", u64::from(self.initial_gc_threshold)),
        ] {
            if value == 0 {
                return Err(VmError::InvalidLimits(name));
            }
        }
        Ok(())
    }
}

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_verification_steps: 32_000_000,
            max_steps: 100_000_000,
            max_stack_depth: 65_536,
            max_heap_objects: 1_000_000,
            max_heap_bytes: 1024 * 1024 * 1024,
            initial_gc_threshold: 1024,
        }
    }
}

/// Finite instruction allowance for the compiler-owned test entry, outside
/// every user phase. Ordinary executions continue to use `VmLimits::max_steps`.
pub const TEST_ENTRY_INSTRUCTIONS: u64 = 100_000_000;

/// Finite structural unwind allowance after a test phase exhausts a resource.
/// This does not authorize more user instructions or guarantee user defers.
pub const TEST_CONTAINMENT_INSTRUCTIONS: u64 = 1_000_000;

/// Logical storage charged before creating a test frame. These fixed portable
/// units describe frame metadata and slots, not Rust layout or allocator size.
pub const TEST_FRAME_BASE_BYTES: u64 = 128;
pub const TEST_FRAME_SLOT_BYTES: u64 = 32;
pub const TEST_FRAME_LOAN_BYTES: u64 = 32;

/// Logical descriptor size for a hosted byte buffer, in addition to its payload.
pub const TEST_HOST_BUFFER_BYTES: u64 = 32;
/// Portable storage for each detached value, excluding its variable payload.
pub const TEST_DETACHED_VALUE_BYTES: u64 = 32;
/// Host snapshot admission bounds data depth separately from VM call depth.
/// Traversal frames and iterative construction use portable workspace units.
pub const TEST_SNAPSHOT_MAX_DEPTH: usize = 256;
pub const TEST_SNAPSHOT_FRAME_BYTES: u64 = 64;
pub const TEST_SNAPSHOT_MEMORY_MODEL: &str = "value-32-workspace-32-frame-64-depth256/1";
/// Import moves admitted String storage and reserves only added heap metadata.
pub const TEST_STRING_IMPORT_MEMORY_MODEL: &str = "moved-string-payload-heap-growth/1";
pub const TEST_HOST_RETURN_MEMORY_MODEL: &str =
    "owned-host-pending-ready-value32-account-walk64-nominal-cancel/2";
pub const TEST_DETACHED_WALK_FRAME_BYTES: u64 = 64;
pub const TEST_HOST_IMPORT_FRAME_BYTES: u64 =
    std::mem::size_of::<execute::host_import::ImportFrame>() as u64;
pub const TEST_HOST_IMPORT_ADMISSION_BYTES: u64 = execute::host_import::HOST_IMPORT_ADMISSION_BYTES;
pub const TEST_WORKER_HOST_IMPORT_CONTEXT_BYTES: u64 = execute::worker_import::CONTEXT_BYTES;
pub const TEST_HOST_IMPORT_MEMORY_MODEL: &str =
    "typed-import-heap-pool-rust-frame-depth256-sync-async-worker-io-storage-bound/14";
/// Variable place-validation storage is admitted before resolving projections.
pub const TEST_PLACE_PATH_BYTES: u64 = 32;
pub const TEST_PLACE_COMPONENT_BYTES: u64 = 64;
pub const TEST_PLACE_INDEX_BYTES: u64 = 8;
pub const TEST_PLACE_MEMORY_MODEL: &str = "path-32-component-64-index-8-snapshot/1";
/// Birth-generation storage retained for each concurrent collection member.
pub const TEST_SYNC_GENERATION_BYTES: u64 = 8;
/// Hosted async job metadata, apart from its admitted argument snapshot.
pub const TEST_HOST_JOB_BYTES: u64 = 128;
pub const TEST_HOST_JOB_ARGUMENT_MODEL: &str = "pending-sync-argument-capacity/1";
pub const TEST_TEXT_MEASURE_MEMORY_MODEL: &str = "managed-length-and-byte-length/1";
pub const TEST_SLICE_MEMORY_MODEL: &str = "streamed-indices-preadmitted-heap-buffer/1";
pub const TEST_ASSERTION_DISPLAY_MODEL: &str = "static-display-utf8-prefix-1024-frame-64/1";
pub const TEST_ASSERTION_VALUE_BYTES: usize = 1024;
/// Hosted channel state, apart from endpoint descriptors and queue capacity.
pub const TEST_HOST_CHANNEL_BYTES: u64 = 256;

/// Logical scheduler storage: a task includes its fixed ready/completion and
/// parent-link entries. Variable captured values are charged separately.
pub const TEST_TASK_BYTES: u64 = 512;
pub const TEST_TASK_SCOPE_BYTES: u64 = 96;
pub const TEST_SCHEDULER_VALUE_BYTES: u64 = 32;
pub const TEST_SELECT_BASE_BYTES: u64 = 64;
pub const TEST_SELECT_ARM_BYTES: u64 = 128;
pub const TEST_CLEANUP_BASE_BYTES: u64 = 96;
pub const TEST_SCHEDULER_HANDLE_BYTES: u64 = 256;
pub const TEST_EXECUTOR_WORKER_BYTES: u64 = 256;

/// Shared accounting for live logical memory owned by one test participation.
/// A charge follows its allocation across worker and phase transitions. This
/// accounts declared payload sizes, not allocator overhead or process RSS.
#[derive(Debug, Clone)]
pub struct VmMemoryBudget {
    live: Arc<AtomicU64>,
    limit: u64,
}

impl VmMemoryBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            live: Arc::new(AtomicU64::new(0)),
            limit,
        }
    }

    pub fn live_bytes(&self) -> u64 {
        self.live.load(Ordering::Acquire)
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    pub(super) fn same_account(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.live, &other.live)
    }

    pub fn can_reserve(&self, bytes: u64) -> bool {
        self.live_bytes()
            .checked_add(bytes)
            .is_some_and(|next| next <= self.limit)
    }

    pub fn reserve(&self, bytes: u64) -> Result<VmMemoryCharge, VmError> {
        self.live
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                live.checked_add(bytes).filter(|next| *next <= self.limit)
            })
            .map_err(|_| VmError::ResourceLimit {
                resource: "memory",
                limit: self.limit,
            })?;
        Ok(VmMemoryCharge {
            budget: self.clone(),
            bytes,
        })
    }
}

/// Releases its charge when the allocation is reclaimed, including on error.
#[derive(Debug)]
pub struct VmMemoryCharge {
    budget: VmMemoryBudget,
    bytes: u64,
}

impl VmMemoryCharge {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    pub fn budget(&self) -> &VmMemoryBudget {
        &self.budget
    }

    /// Partition an admitted transaction without reserving its bytes again.
    /// Failure leaves both the charge and its shared counter unchanged.
    pub fn split_off(&mut self, bytes: u64) -> Result<Self, VmError> {
        let remaining = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| VmError::invariant("memory charge split exceeds its reservation"))?;
        self.bytes = remaining;
        Ok(Self {
            budget: self.budget.clone(),
            bytes,
        })
    }

    /// Transfer already admitted storage within one account. Moving a payload
    /// must neither release its live charge nor reserve a second copy of it.
    pub fn transfer_to(&mut self, destination: &mut Self, bytes: u64) -> Result<(), VmError> {
        if !self.budget.same_account(&destination.budget) {
            return Err(VmError::invariant(
                "memory transfer crosses owning accounts",
            ));
        }
        let remaining = self
            .bytes
            .checked_sub(bytes)
            .ok_or_else(|| VmError::invariant("memory transfer exceeds its reservation"))?;
        let combined = destination
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| VmError::invariant("memory transfer exceeds its destination"))?;
        self.bytes = remaining;
        destination.bytes = combined;
        Ok(())
    }

    pub fn resize(&mut self, bytes: u64) -> Result<(), VmError> {
        if bytes > self.bytes {
            let mut growth = self.budget.reserve(bytes - self.bytes)?;
            growth.bytes = 0;
        } else {
            self.budget
                .live
                .fetch_sub(self.bytes - bytes, Ordering::AcqRel);
        }
        self.bytes = bytes;
        Ok(())
    }
}

impl Drop for VmMemoryCharge {
    fn drop(&mut self) {
        self.budget.live.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Observable runtime value detached from the VM heap.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    Unit,
    Bool(bool),
    Integer(i128),
    Float(f64),
    Byte(u8),
    Char(char),
    String(String),
    Function {
        name: String,
        type_arguments: Vec<u32>,
    },
    Tuple(Vec<Self>),
    Array(Vec<Self>),
    Map(Vec<(Self, Self)>),
    Set(Vec<Self>),
    Closure {
        callable: u32,
        captures: Vec<Self>,
    },
    Newtype {
        name: String,
        value: Box<Self>,
    },
    /// Nominal record exchanged with a trusted host adapter in declaration order.
    ///
    /// Source member IDs are intentionally absent: they are local to one
    /// compiled program and therefore are not a stable host ABI.
    Record {
        name: String,
        values: Vec<Self>,
    },
    /// Nominal enum exchanged with a trusted host adapter. `variant` is the
    /// zero-based declaration ordinal and payload values retain declaration
    /// order, so the VM can bind them to the verified nominal descriptor.
    Variant {
        name: String,
        variant: u32,
        values: Vec<Self>,
    },
    OptionNone,
    OptionSome(Box<Self>),
    ResultOk(Box<Self>),
    ResultErr(Box<Self>),
    Union {
        member: u32,
        value: Box<Self>,
    },
    Range {
        inclusive: bool,
        start: Box<Self>,
        end: Box<Self>,
    },
    Ref(Option<Box<Self>>),
    Host {
        kind: RuntimeHostValueKind,
        id: u64,
    },
    /// Back-reference used only when snapshotting an identity graph with a cycle.
    Cycle(usize),
}

/// A host completion keeps per-call failures attached to their waiting task.
/// Errors outside this record describe failure of the wait operation itself.
/// An admitted response retains its reservation throughout the wait and delivery.
#[derive(Debug)]
pub struct VmHostCompletion {
    pub call: u64,
    pub result: Result<VmHostReturn, VmError>,
}

/// Borrowed result shape available before a host operation commits its state.
/// Wrappers describe storage without constructing or copying the returned value.
#[derive(Debug, Clone, Copy)]
pub enum VmHostReturnPreview<'a> {
    Value(&'a RuntimeValue),
    /// Reserve the componentwise storage maximum of 1..=16 typed outcomes.
    /// The eventual value must fit this envelope and its declared type. This
    /// is a storage bound, not a whitelist of values. Nested bounds are invalid.
    StorageBound(&'a [VmHostReturnPreview<'a>]),
    /// One array in logical order, including a wrapped queue's two slices.
    ArrayParts {
        first: &'a [RuntimeValue],
        second: &'a [RuntimeValue],
    },
    Variant(u32, &'a RuntimeValue),
    ResultOk(&'a RuntimeValue),
    OptionSome(&'a RuntimeValue),
    ResultOkOption(Option<&'a RuntimeValue>),
    ResultOkVariant(u32, &'a RuntimeValue),
    ResultOkVariantOption(u32, Option<&'a RuntimeValue>),
}

/// An owned host response and its live transport reservation.
/// The value must remain paired with this charge until import or discard.
/// Admission here covers transport of an already constructed result; a host
/// must separately preadmit allocations while constructing that result.
#[derive(Debug)]
pub struct VmHostReturn {
    pub value: RuntimeValue,
    pub memory: Option<VmMemoryCharge>,
}

impl VmHostReturn {
    pub fn admit(value: RuntimeValue, budget: Option<&VmMemoryBudget>) -> Result<Self, VmError> {
        let memory = budget
            .map(|budget| {
                let bytes = value.measure_retained_bytes(Some(budget))?;
                budget.reserve(bytes)
            })
            .transpose()?;
        Ok(Self { value, memory })
    }
}

/// Admission for one host reply, prepared from borrowed payloads before
/// cloning them or committing a mutation. Framing includes detached wrapper
/// descriptors and their names. Registry payloads keep their separate owner.
pub struct VmHostReturnBudget<'a> {
    budget: Option<&'a VmMemoryBudget>,
    memory: Option<VmMemoryCharge>,
}

impl<'a> VmHostReturnBudget<'a> {
    pub fn new(budget: Option<&'a VmMemoryBudget>) -> Self {
        Self {
            budget,
            memory: None,
        }
    }

    pub fn reserve<'b>(
        &mut self,
        framing: u64,
        payloads: impl IntoIterator<Item = &'b RuntimeValue>,
    ) -> Result<(), VmError> {
        let Some(budget) = self.budget else {
            return Ok(());
        };
        if self.memory.is_some() {
            return Err(VmError::invariant("host reply was admitted twice"));
        }
        let mut bytes = framing;
        for value in payloads {
            bytes = bytes
                .checked_add(value.measure_retained_bytes(Some(budget))?)
                .ok_or(VmError::ResourceLimit {
                    resource: "memory",
                    limit: budget.limit(),
                })?;
        }
        self.memory = Some(budget.reserve(bytes)?);
        Ok(())
    }

    /// Move the prepared charge into a pending response record. The caller
    /// must retain it until that exact response is consumed or discarded.
    pub fn into_reservation(self) -> Option<VmMemoryCharge> {
        self.memory
    }

    /// Admit additional descriptors before an incremental result builder adds
    /// them. The caller must not have published the result or committed effects.
    /// A rejected growth leaves the previous reservation intact.
    pub fn grow_before_construction(&mut self, bytes: u64) -> Result<(), VmError> {
        if self.budget.is_none() {
            return Ok(());
        }
        let memory = self.memory.as_mut().ok_or_else(|| {
            VmError::invariant("host reply growth requires an initial reservation")
        })?;
        memory.resize(bytes.max(memory.bytes()))
    }

    /// Closed fixed-size responses can relinquish unused optional descriptors.
    /// Growth here would be too late to preserve mutation atomicity.
    pub fn shrink(&mut self, bytes: u64) -> Result<(), VmError> {
        if let Some(memory) = &mut self.memory {
            if bytes > memory.bytes() {
                return Err(VmError::invariant(
                    "host reply exceeds its preadmitted size",
                ));
            }
            memory.resize(bytes)?;
        }
        Ok(())
    }

    /// The caller must construct exactly the reserved shape, or shrink a
    /// closed response before finishing. Unprepared replies receive only
    /// post-construction transport admission.
    pub fn finish(self, value: RuntimeValue) -> Result<VmHostReturn, VmError> {
        if self.memory.is_some() {
            Ok(VmHostReturn {
                value,
                memory: self.memory,
            })
        } else {
            // Unconverted/reference hosts still admit constructed transport.
            // This branch does not prove construction or mutation preflight.
            VmHostReturn::admit(value, self.budget)
        }
    }
}

/// Closed identities for opaque values exchanged with the hosted standard
/// library. The payload remains in the host registry; bytecode carries only a
/// typed run-local token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimeHostValueKind {
    /// Immutable artifact metadata; no external host registry owns this token.
    Reflection(crate::reflection::ReflectionDescriptorKind, [u8; 32]),
    Command,
    Pipeline,
    Bytes,
    BytesBuilder,
    BytesError,
    FormatBuilder,
    FormatError,
    TextError,
    CollectionError,
    Path,
    PathError,
    File,
    Directory,
    Metadata,
    OpenMode,
    FsError,
    MathError,
    FloatTolerance,
    TempDirectory,
    Generator,
    Reader,
    Writer,
    IoLimits,
    IoError,
    ConsoleError,
    ExitStatus,
    ProcessOutput,
    ProcessHandle,
    ProcessError,
    ProcessExitError,
    Utf8Error,
    Instant,
    Timer,
    DurationError,
    ClockError,
    EnvSnapshot,
    EnvName,
    EnvValue,
    EnvError,
    VirtualTime,
    JsonValue,
    JsonValueView,
    JsonRaw,
    JsonNumber,
    JsonReader,
    JsonWriter,
    YamlValue,
    YamlValueView,
    YamlReader,
    YamlWriter,
    EncodingBase64Encoder,
    EncodingBase64Decoder,
    EncodingHexEncoder,
    EncodingHexDecoder,
    MessagePackValue,
    MessagePackValueView,
    MessagePackRaw,
    MessagePackTimestamp,
    MessagePackReader,
    MessagePackWriter,
    ProtoDescriptor,
    ProtoLimits,
    ProtoDecodeOptions,
    ProtoEncodeOptions,
    ProtoWireTypePolicy,
    ProtoUnknownPolicy,
    ProtoReader,
    ProtoWriter,
    UnknownFields,
    Waiter,
    Completer,
    AlreadyCompleted,
    Group,
    Mutex,
    MutexGuard,
    RwLock,
    ReadGuard,
    WriteGuard,
    Condition,
    Semaphore,
    Permit,
    Once,
    Barrier,
    Atomic,
    SyncArray,
    SyncMap,
    SyncSet,
    SyncStack,
    SyncQueue,
    ChannelSender,
    ChannelReceiver,
    ExecutorPool,
    ExecutorBlockingPool,
    ExecutorActor,
    ExecutorActorRef,
}

/// Typed registry roots. Different VM and host resources may reuse an integer
/// identity; their kinds must remain part of the tracing identity.
pub type VmHostRoots = std::collections::BTreeSet<(RuntimeHostValueKind, u64)>;

/// A cursor per active parent keeps wide collections from allocating one
/// pending entry per child. No payload is copied and no Rust recursion is used.
struct DetachedValueWalk<'a> {
    current: Option<&'a RuntimeValue>,
    parents: Vec<(&'a RuntimeValue, usize)>,
    memory: Option<VmMemoryCharge>,
}

impl<'a> DetachedValueWalk<'a> {
    fn new(value: &'a RuntimeValue, budget: Option<&VmMemoryBudget>) -> Result<Self, VmError> {
        Ok(Self {
            current: Some(value),
            parents: Vec::new(),
            memory: budget.map(|budget| budget.reserve(0)).transpose()?,
        })
    }

    fn next(&mut self) -> Result<Option<&'a RuntimeValue>, VmError> {
        let Some(value) = self.current.take() else {
            return Ok(None);
        };
        if let Some(child) = value.child(0) {
            if let Some(memory) = &mut self.memory {
                let bytes = (self.parents.len() as u64 + 1)
                    .checked_mul(TEST_DETACHED_WALK_FRAME_BYTES)
                    .ok_or(VmError::ResourceLimit {
                        resource: "memory",
                        limit: memory.budget().limit(),
                    })?;
                memory.resize(bytes)?;
            }
            self.parents.push((value, 1));
            self.current = Some(child);
        } else {
            while let Some((parent, index)) = self.parents.last_mut() {
                if let Some(child) = parent.child(*index) {
                    *index += 1;
                    self.current = Some(child);
                    break;
                }
                self.parents.pop();
            }
            if let Some(memory) = &mut self.memory {
                memory.resize(self.parents.len() as u64 * TEST_DETACHED_WALK_FRAME_BYTES)?;
            }
        }
        Ok(Some(value))
    }
}

impl RuntimeValue {
    /// Logical owned storage of a detached value, including nested copies.
    /// Host tokens and cycle markers contain identities only; their referenced
    /// payload remains charged to the registry or heap that owns it.
    pub fn retained_bytes(&self) -> Option<u64> {
        self.measure_retained_bytes(None).ok()
    }

    fn measure_retained_bytes(&self, budget: Option<&VmMemoryBudget>) -> Result<u64, VmError> {
        let overflow = || VmError::ResourceLimit {
            resource: "memory",
            limit: budget.map_or(u64::MAX, VmMemoryBudget::limit),
        };
        let mut walk = DetachedValueWalk::new(self, budget)?;
        let mut bytes = 0_u64;
        while let Some(value) = walk.next()? {
            bytes = bytes
                .checked_add(TEST_DETACHED_VALUE_BYTES)
                .ok_or_else(overflow)?;
            match value {
                Self::String(text) => {
                    bytes = bytes.checked_add(text.len() as u64).ok_or_else(overflow)?
                }
                Self::Function {
                    name,
                    type_arguments,
                } => {
                    bytes = bytes
                        .checked_add(name.len() as u64)
                        .and_then(|bytes| {
                            bytes.checked_add((type_arguments.len() as u64).checked_mul(4)?)
                        })
                        .ok_or_else(overflow)?;
                }
                Self::Record { name, .. }
                | Self::Variant { name, .. }
                | Self::Newtype { name, .. } => {
                    bytes = bytes.checked_add(name.len() as u64).ok_or_else(overflow)?;
                }
                Self::Unit
                | Self::Bool(_)
                | Self::Integer(_)
                | Self::Float(_)
                | Self::Byte(_)
                | Self::Char(_)
                | Self::Tuple(_)
                | Self::Array(_)
                | Self::Map(_)
                | Self::Set(_)
                | Self::Closure { .. }
                | Self::OptionNone
                | Self::OptionSome(_)
                | Self::ResultOk(_)
                | Self::ResultErr(_)
                | Self::Union { .. }
                | Self::Range { .. }
                | Self::Ref(_)
                | Self::Host { .. }
                | Self::Cycle(_) => {}
            }
        }
        Ok(bytes)
    }

    /// Trace detached values without copying their payloads or recursing on
    /// the Rust stack. Cycle markers refer to values already visited in a
    /// detached snapshot and introduce no additional roots.
    pub fn trace_host_roots(&self, roots: &mut VmHostRoots) {
        let mut walk =
            DetachedValueWalk::new(self, None).expect("unaccounted walk cannot reject admission");
        while let Some(value) = walk
            .next()
            .expect("unaccounted walk cannot reject admission")
        {
            if let Self::Host { kind, id } = value {
                roots.insert((*kind, *id));
            }
        }
    }

    fn child(&self, index: usize) -> Option<&Self> {
        match self {
            Self::Tuple(values)
            | Self::Array(values)
            | Self::Set(values)
            | Self::Record { values, .. }
            | Self::Variant { values, .. }
            | Self::Closure {
                captures: values, ..
            } => values.get(index),
            Self::Map(entries) => entries
                .get(index / 2)
                .map(|(key, value)| if index.is_multiple_of(2) { key } else { value }),
            Self::Newtype { value, .. }
            | Self::OptionSome(value)
            | Self::ResultOk(value)
            | Self::ResultErr(value)
            | Self::Union { value, .. }
            | Self::Ref(Some(value)) => (index == 0).then_some(value),
            Self::Range { start, end, .. } => match index {
                0 => Some(start),
                1 => Some(end),
                _ => None,
            },
            Self::Unit
            | Self::Bool(_)
            | Self::Integer(_)
            | Self::Float(_)
            | Self::Byte(_)
            | Self::Char(_)
            | Self::String(_)
            | Self::Function { .. }
            | Self::OptionNone
            | Self::Ref(None)
            | Self::Host { .. }
            | Self::Cycle(_) => None,
        }
    }
}

/// Per-run counters useful for testing limits and collector behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmStatistics {
    pub steps: u64,
    pub allocations: u64,
    pub collections: u64,
    pub reclaimed_objects: u64,
    /// Logical `Array`, `Map`, or `Set` copies requested by verified bytecode.
    pub logical_collection_copies: u64,
    /// Top-level collection elements physically traversed while making copies.
    pub collection_elements_copied: u64,
    /// Collection buffers reused by copy-on-write instead of traversing elements.
    pub collection_buffer_shares: u64,
    /// Shared collection buffers separated before a write.
    pub collection_buffer_detaches: u64,
    pub peak_stack_depth: u32,
    pub peak_live_objects: u32,
    pub peak_live_bytes: u64,
    /// Number of selection commits performed by the cooperative scheduler.
    pub select_commits: u64,
    /// Number of arms registered in selection regions.
    pub select_registrations: u64,
    /// Number of selection regions that had to park their owner.
    pub select_waits: u64,
    /// Number of parked selection owners woken by a dependency completion.
    pub select_wakeups: u64,
    /// Number of arm table entries inspected by selection arbitration.
    pub select_arm_scans: u64,
    /// Number of runtime arm-table allocations created by `BeginSelect`.
    pub select_frame_allocations: u64,
    /// Peak bytes reserved for one runtime selection arm table.
    pub select_peak_frame_bytes: u64,
    /// Largest selection arm table observed in this run.
    pub select_peak_arms: u32,
    /// Number of Group children added to runtime state.
    pub group_adds: u64,
    /// Number of `Group.all` polls performed.
    pub group_all_operations: u64,
    /// Number of `Group.settle` polls performed.
    pub group_settle_operations: u64,
    /// Number of `Group.next` polls performed.
    pub group_next_operations: u64,
    /// Number of `Group.cancel` polls performed.
    pub group_cancel_operations: u64,
    /// Group child status entries inspected by an operation.
    pub group_child_scans: u64,
    /// Group waiters parked by the scheduler.
    pub group_waits: u64,
    /// Group waiters woken by child completion.
    pub group_wakeups: u64,
    /// Group-wide cancellation requests issued by `all` or `cancel`.
    pub group_cancellation_requests: u64,
    /// Runtime Group state records created.
    pub group_state_allocations: u64,
    /// Child-vector capacity growth events for Group state.
    pub group_child_buffer_grows: u64,
    /// Waiter-vector capacity growth events for Group state.
    pub group_waiter_buffer_grows: u64,
    /// Largest number of children retained by one Group in this run.
    pub group_peak_children: u32,
    /// Logical bytes reserved by one Group child/waiter state at peak.
    pub group_peak_state_bytes: u64,
}

/// Stable language panic identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicCode {
    Bounds,
    ZeroSliceStep,
    IntegerDivisionByZero,
    OverlappingBorrow,
    CheckedOverflow,
    ArrayShapeMismatch,
    AssertionFailed,
    ExplicitPanic,
    DuplicateDynamicMapKey,
    InvalidShiftCount,
    InvalidRepeatCount,
}

impl PanicCode {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Bounds => "P0001",
            Self::ZeroSliceStep => "P0002",
            Self::IntegerDivisionByZero => "P0003",
            Self::OverlappingBorrow => "P0004",
            Self::CheckedOverflow => "P0005",
            Self::ArrayShapeMismatch => "P0006",
            Self::AssertionFailed => "P0007",
            Self::ExplicitPanic => "P0008",
            Self::DuplicateDynamicMapKey => "P0009",
            Self::InvalidShiftCount => "P0010",
            Self::InvalidRepeatCount => "P0011",
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Bounds => "bounds",
            Self::ZeroSliceStep => "zero-slice-step",
            Self::IntegerDivisionByZero => "integer-division-by-zero",
            Self::OverlappingBorrow => "overlapping-borrow",
            Self::CheckedOverflow => "checked-overflow",
            Self::ArrayShapeMismatch => "array-shape-mismatch",
            Self::AssertionFailed => "assertion-failed",
            Self::ExplicitPanic => "explicit-panic",
            Self::DuplicateDynamicMapKey => "duplicate-dynamic-map-key",
            Self::InvalidShiftCount => "invalid-shift-count",
            Self::InvalidRepeatCount => "invalid-repeat-count",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmStackFrame {
    pub function: String,
    pub span: BytecodeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmPanic {
    pub code: PanicCode,
    pub message: String,
    pub span: BytecodeSpan,
    pub stack: Vec<VmStackFrame>,
    pub suppressed: Vec<VmPanic>,
    pub(super) from_test_control: bool,
}

impl VmPanic {
    /// Finds the first language panic, excluding the VM's private test-control
    /// unwind. A skip or budget terminal must not hide an actual cleanup panic.
    pub fn language_panic(&self) -> Option<&Self> {
        let mut pending = vec![self];
        while let Some(panic) = pending.pop() {
            if !panic.from_test_control {
                return Some(panic);
            }
            pending.extend(panic.suppressed.iter().rev());
        }
        None
    }
}

#[derive(Debug)]
pub enum VmError {
    /// Internal evaluator signal. The scheduler consumes it only when this
    /// task has a pending cancellation; it is never a language panic.
    CallbackInterrupted,
    InvalidBytecode(BytecodeVerificationError),
    InvalidLimits(&'static str),
    InvalidEntry(String),
    ResourceLimit {
        resource: &'static str,
        limit: u64,
    },
    OutOfMemory {
        live_objects: u32,
        live_bytes: u64,
    },
    UnsupportedHostCall(String),
    Host(String),
    Invariant(String),
}

impl VmError {
    pub(super) fn invariant(message: impl Into<String>) -> Self {
        Self::Invariant(message.into())
    }

    pub fn is_resource_limit(&self) -> bool {
        matches!(self, Self::ResourceLimit { .. } | Self::OutOfMemory { .. })
    }
}

impl fmt::Display for VmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CallbackInterrupted => {
                write!(formatter, "synchronous callback yielded for cancellation")
            }
            Self::InvalidBytecode(error) => write!(formatter, "invalid bytecode: {error}"),
            Self::InvalidLimits(limit) => write!(formatter, "invalid VM limit `{limit}`"),
            Self::InvalidEntry(message) => write!(formatter, "invalid VM entry: {message}"),
            Self::ResourceLimit { resource, limit } => {
                write!(formatter, "VM {resource} limit of {limit} exceeded")
            }
            Self::OutOfMemory {
                live_objects,
                live_bytes,
            } => write!(
                formatter,
                "VM heap exhausted with {live_objects} live objects and {live_bytes} live bytes"
            ),
            Self::UnsupportedHostCall(name) => {
                write!(formatter, "unsupported VM host call `{name}`")
            }
            Self::Host(message) => write!(formatter, "VM host failure: {message}"),
            Self::Invariant(message) => write!(formatter, "VM invariant failed: {message}"),
        }
    }
}

impl Error for VmError {}

impl From<BytecodeVerificationError> for VmError {
    fn from(error: BytecodeVerificationError) -> Self {
        Self::InvalidBytecode(error)
    }
}

#[cfg(test)]
mod tests {
    use crate::bytecode::{
        BytecodeCallableId, BytecodeIntrinsicType, BytecodeProgram, BytecodeTraceDescriptor,
        BytecodeType, BytecodeTypeId, BytecodeTypeKind, BytecodeVariant, BytecodeVariantPayload,
        verify_bytecode,
    };

    use super::heap::{Heap, HeapHandle, HeapObject, SharedBuffer};
    use super::value::{AggregatePayload, Value, snapshot_value};
    use super::*;

    #[test]
    fn host_reply_preflight_owns_framing_payload_and_measurement_failures() {
        let payload = RuntimeValue::Array(vec![RuntimeValue::String("x".repeat(1024))]);
        let bytes = 32 + 32 + 32 + 1024;
        for shortage in [1, 0] {
            let owner = VmMemoryBudget::new(bytes - shortage);
            let mut response = VmHostReturnBudget::new(Some(&owner));
            let result = response.reserve(32, [&payload]);
            if shortage == 1 {
                assert!(matches!(result, Err(VmError::ResourceLimit { .. })));
                assert_eq!(owner.live_bytes(), 0);
            } else {
                result.unwrap();
                assert_eq!(owner.live_bytes(), bytes);
                assert!(matches!(
                    response.reserve(0, []),
                    Err(VmError::Invariant(_))
                ));
                assert!(matches!(
                    response.shrink(bytes + 1),
                    Err(VmError::Invariant(_))
                ));
                assert_eq!(owner.live_bytes(), bytes);
                let reply = response
                    .finish(RuntimeValue::OptionSome(Box::new(payload.clone())))
                    .unwrap();
                assert_eq!(reply.value.retained_bytes(), Some(bytes));
                assert_eq!(owner.live_bytes(), bytes);
                drop(reply);
                assert_eq!(owner.live_bytes(), 0);
            }
        }
        let owner = VmMemoryBudget::new(63);
        let mut response = VmHostReturnBudget::new(Some(&owner));
        assert!(response.reserve(32, [&payload]).is_err());
        assert_eq!(owner.live_bytes(), 0, "failed cursor admission is atomic");
        assert!(response.reserve(u64::MAX, [&RuntimeValue::Unit]).is_err());
        assert_eq!(
            owner.live_bytes(),
            0,
            "framing overflow releases measurement workspace"
        );
        response.reserve(32, []).unwrap();
        response.shrink(0).unwrap();
        assert_eq!(owner.live_bytes(), 0);
    }

    #[test]
    fn incremental_host_reply_admission_preserves_previous_storage_on_rejection() {
        let owner = VmMemoryBudget::new(128);
        let mut response = VmHostReturnBudget::new(Some(&owner));
        assert!(matches!(
            response.grow_before_construction(32),
            Err(VmError::Invariant(_))
        ));
        assert_eq!(owner.live_bytes(), 0);
        response.reserve(32, []).unwrap();
        response.grow_before_construction(96).unwrap();
        response.grow_before_construction(64).unwrap();
        assert_eq!(
            owner.live_bytes(),
            96,
            "growth cannot silently release storage"
        );
        assert!(matches!(
            response.grow_before_construction(129),
            Err(VmError::ResourceLimit { .. })
        ));
        assert_eq!(owner.live_bytes(), 96);
        response.grow_before_construction(128).unwrap();
        response.shrink(96).unwrap();
        let reply = response
            .finish(RuntimeValue::Array(vec![RuntimeValue::Unit; 2]))
            .unwrap();
        assert_eq!(reply.value.retained_bytes(), Some(96));
        drop(reply);
        assert_eq!(owner.live_bytes(), 0);
    }

    #[test]
    fn detached_walk_bounds_workspace_by_depth_and_releases_rejected_admission() {
        let wide = RuntimeValue::Array(vec![RuntimeValue::Unit; 10_000]);
        let owner = VmMemoryBudget::new(64);
        let mut walk = DetachedValueWalk::new(&wide, Some(&owner)).unwrap();
        let mut count = 0;
        while walk.next().unwrap().is_some() {
            count += 1;
            assert!(walk.parents.len() <= 1);
            assert!(owner.live_bytes() <= 64);
        }
        assert_eq!(count, 10_001);
        assert_eq!(owner.live_bytes(), 0);
        assert_eq!(wide.retained_bytes(), Some(10_001 * 32));
        let short = VmMemoryBudget::new(63);
        let mut rejected = DetachedValueWalk::new(&wide, Some(&short)).unwrap();
        assert!(rejected.next().unwrap_err().is_resource_limit());
        assert!(rejected.parents.is_empty());
        assert_eq!(short.live_bytes(), 0);

        let mut deep = RuntimeValue::Unit;
        for _ in 0..20 {
            deep = RuntimeValue::OptionSome(Box::new(deep));
        }
        let owner = VmMemoryBudget::new(19 * 64);
        let mut walk = DetachedValueWalk::new(&deep, Some(&owner)).unwrap();
        for _ in 0..19 {
            assert!(walk.next().unwrap().is_some());
        }
        assert!(walk.next().unwrap_err().is_resource_limit());
        assert_eq!(owner.live_bytes(), 19 * 64);
        drop(walk);
        assert_eq!(owner.live_bytes(), 0);
        let owner = VmMemoryBudget::new(20 * 64);
        assert_eq!(deep.measure_retained_bytes(Some(&owner)).unwrap(), 21 * 32);
        assert_eq!(owner.live_bytes(), 0);
    }

    #[test]
    fn detached_walk_measures_names_and_payloads_and_traces_every_container_edge() {
        let host = |id| RuntimeValue::Host {
            kind: RuntimeHostValueKind::Bytes,
            id,
        };
        let value = RuntimeValue::Array(vec![
            RuntimeValue::Tuple(vec![host(1)]),
            RuntimeValue::Map(vec![(host(2), host(3))]),
            RuntimeValue::Set(vec![host(4)]),
            RuntimeValue::Closure {
                callable: 0,
                captures: vec![host(5)],
            },
            RuntimeValue::Newtype {
                name: "N".into(),
                value: Box::new(host(6)),
            },
            RuntimeValue::Record {
                name: "R".into(),
                values: vec![host(7)],
            },
            RuntimeValue::Variant {
                name: "V".into(),
                variant: 0,
                values: vec![host(8)],
            },
            RuntimeValue::OptionSome(Box::new(host(9))),
            RuntimeValue::ResultOk(Box::new(host(10))),
            RuntimeValue::ResultErr(Box::new(host(11))),
            RuntimeValue::Union {
                member: 0,
                value: Box::new(host(12)),
            },
            RuntimeValue::Range {
                inclusive: false,
                start: Box::new(host(13)),
                end: Box::new(host(14)),
            },
            RuntimeValue::Ref(Some(Box::new(host(15)))),
            RuntimeValue::Cycle(0),
            RuntimeValue::OptionNone,
            RuntimeValue::Ref(None),
            RuntimeValue::Function {
                name: "f".into(),
                type_arguments: vec![0, 1],
            },
            RuntimeValue::String("é".into()),
        ]);
        assert_eq!(value.retained_bytes(), Some(34 * 32 + 14));
        let mut roots = VmHostRoots::new();
        value.trace_host_roots(&mut roots);
        assert_eq!(
            roots,
            (1..=15)
                .map(|id| (RuntimeHostValueKind::Bytes, id))
                .collect()
        );
    }

    #[test]
    fn phase_memory_transfers_preserve_live_storage_and_owning_accounts() {
        let owner = VmMemoryBudget::new(10);
        let other = VmMemoryBudget::new(10);
        let mut source = owner.reserve(6).unwrap();
        let mut destination = owner.reserve(4).unwrap();
        let mut unrelated = other.reserve(1).unwrap();
        assert!(source.transfer_to(&mut unrelated, 1).is_err());
        assert!(source.transfer_to(&mut destination, 7).is_err());
        assert_eq!(
            (source.bytes(), destination.bytes(), unrelated.bytes()),
            (6, 4, 1)
        );
        source.transfer_to(&mut destination, 6).unwrap();
        assert_eq!((source.bytes(), destination.bytes()), (0, 10));
        assert_eq!(owner.live_bytes(), 10);
        drop(source);
        assert_eq!(owner.live_bytes(), 10);
        drop(destination);
        assert_eq!(owner.live_bytes(), 0);
        assert_eq!(other.live_bytes(), 1);
        drop(unrelated);
        assert_eq!(other.live_bytes(), 0);
    }

    #[test]
    fn phase_memory_charges_are_atomic_shared_and_released() {
        let budget = VmMemoryBudget::new(8);
        let mut charge = budget.reserve(4).unwrap();
        charge.resize(8).unwrap();
        assert!(charge.resize(9).is_err());
        assert!(budget.reserve(u64::MAX).is_err());
        assert_eq!(budget.live_bytes(), 8);
        charge.resize(3).unwrap();
        assert_eq!(budget.live_bytes(), 3);
        assert!(charge.split_off(4).is_err());
        let split = charge.split_off(2).unwrap();
        assert_eq!(budget.live_bytes(), 3);
        drop(split);
        assert_eq!(budget.live_bytes(), 1);
        drop(charge);
        assert_eq!(budget.live_bytes(), 0);

        let barrier = Arc::new(std::sync::Barrier::new(9));
        let workers = (0..8)
            .map(|_| {
                let budget = budget.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let reservation = budget.reserve(8);
                    barrier.wait();
                    barrier.wait();
                    reservation.is_ok()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        assert_eq!(budget.live_bytes(), 8);
        barrier.wait();
        let successes = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(successes, 1);
        assert_eq!(budget.live_bytes(), 0);
    }

    fn limits() -> VmLimits {
        VmLimits {
            max_heap_objects: 8,
            max_heap_bytes: 16 * 1024,
            initial_gc_threshold: 1,
            ..VmLimits::default()
        }
    }

    #[test]
    fn invalid_bytecode_preserves_the_verifier_diagnostic_at_the_vm_boundary() {
        let program = BytecodeProgram {
            reflection: Default::default(),
            types: vec![BytecodeType {
                name: "Array".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Array,
                    arguments: Vec::new(),
                },
            }],
            nominals: Vec::new(),
            callables: Vec::new(),
            constants: Vec::new(),
            functions: Vec::new(),
        };
        let verification = verify_bytecode(&program).unwrap_err();

        assert_eq!(verification.context(), "type#0");
        assert_eq!(verification.message(), "intrinsic type has the wrong arity");
        assert!(!verification.is_resource_limit());

        let error = VmError::from(verification);
        assert_eq!(
            error.to_string(),
            "invalid bytecode: bytecode invariant failed in type#0: intrinsic type has the wrong arity"
        );
    }

    #[test]
    fn collection_buffer_uniqueness_tracks_physical_owners() {
        let buffer = SharedBuffer::from(vec![1, 2, 3]);
        assert!(buffer.is_unique());
        let alias = buffer.clone();
        assert!(!buffer.is_unique());
        drop(alias);
        assert!(buffer.is_unique());

        let mut managed = SharedBuffer::from(vec![Some(Value::Integer(1))]);
        let original = managed.clone();
        managed[0] = Some(Value::Integer(2));
        assert_eq!(original[0], Some(Value::Integer(1)));
        assert_eq!(managed[0], Some(Value::Integer(2)));

        let values = original.clone().into_iter().collect::<Vec<_>>();
        assert_eq!(values, [Some(Value::Integer(1))]);
        let rebuilt = values.into_iter().collect::<SharedBuffer<_>>();
        assert_eq!(rebuilt[0], Some(Value::Integer(1)));

        let mut entries =
            SharedBuffer::from(vec![(Some(Value::Integer(3)), Some(Value::Integer(4)))]);
        let original_entries = entries.clone();
        entries[0].1 = Some(Value::Integer(5));
        assert_eq!(original_entries[0].1, Some(Value::Integer(4)));
        assert_eq!(entries[0].1, Some(Value::Integer(5)));
        assert_eq!(
            original_entries.clone().into_iter().collect::<Vec<_>>(),
            [(Some(Value::Integer(3)), Some(Value::Integer(4)))]
        );
    }

    fn heap() -> Heap {
        Heap::new(
            limits(),
            vec![
                BytecodeTraceDescriptor::String,
                BytecodeTraceDescriptor::Ref {
                    value: BytecodeTypeId::new(1),
                },
                BytecodeTraceDescriptor::Closure {
                    callable: BytecodeCallableId::new(7),
                    captures: vec![BytecodeTypeId::new(0)],
                },
            ],
        )
    }

    fn string_heap(limits: VmLimits) -> Heap {
        Heap::new(limits, vec![BytecodeTraceDescriptor::String])
    }

    fn reserved_string(capacity: usize, value: &str) -> HeapObject {
        let mut text = String::with_capacity(capacity);
        text.push_str(value);
        HeapObject::String(text)
    }

    struct MemoryTestAdapter {
        heap: Heap,
        roots: Vec<Value>,
        statistics: VmStatistics,
    }

    impl MemoryTestAdapter {
        fn new() -> Self {
            Self {
                heap: Heap::new(
                    limits(),
                    vec![
                        BytecodeTraceDescriptor::String,
                        BytecodeTraceDescriptor::Ref {
                            value: BytecodeTypeId::new(2),
                        },
                        BytecodeTraceDescriptor::Array {
                            element: BytecodeTypeId::new(3),
                        },
                        BytecodeTraceDescriptor::Closure {
                            callable: BytecodeCallableId::new(7),
                            captures: vec![BytecodeTypeId::new(1)],
                        },
                    ],
                ),
                roots: Vec::new(),
                statistics: VmStatistics::default(),
            }
        }

        fn active_roots(&self, temporary: &[HeapHandle]) -> Vec<Value> {
            self.roots
                .iter()
                .cloned()
                .chain(temporary.iter().copied().map(Value::Heap))
                .collect()
        }

        fn allocate(
            &mut self,
            descriptor: BytecodeTypeId,
            object: HeapObject,
            temporary: &[HeapHandle],
        ) -> HeapHandle {
            let roots = self.active_roots(temporary);
            self.heap
                .allocate(descriptor, object, &roots, &mut self.statistics)
                .unwrap()
        }

        fn replace(&mut self, handle: HeapHandle, object: HeapObject, temporary: &[HeapHandle]) {
            let roots = self.active_roots(temporary);
            self.heap
                .replace(handle, object, &roots, &mut self.statistics)
                .unwrap();
        }

        fn create_mixed_cycle(&mut self) -> [HeapHandle; 3] {
            let reference = self.allocate(BytecodeTypeId::new(1), HeapObject::Ref(None), &[]);
            let closure = self.allocate(
                BytecodeTypeId::new(3),
                HeapObject::Closure {
                    callable: BytecodeCallableId::new(7),
                    captures: vec![Some(Value::Heap(reference))],
                },
                &[reference],
            );
            let array = self.allocate(
                BytecodeTypeId::new(2),
                HeapObject::Array(vec![Some(Value::Heap(closure))].into()),
                &[reference, closure],
            );
            self.replace(
                reference,
                HeapObject::Ref(Some(Value::Heap(array))),
                &[reference, closure, array],
            );
            [reference, array, closure]
        }

        fn retain(&mut self, handle: HeapHandle) {
            self.roots.push(Value::Heap(handle));
        }

        fn release_all(&mut self) {
            self.roots.clear();
        }

        fn apply_pressure(&mut self, allocations: usize) {
            for index in 0..allocations {
                self.allocate(
                    BytecodeTypeId::new(0),
                    HeapObject::String(format!("pressure-{index}")),
                    &[],
                );
            }
        }

        fn is_live(&self, handle: HeapHandle) -> bool {
            self.heap.get(handle).is_ok()
        }
    }

    #[test]
    fn precise_heap_keeps_reachable_objects_and_reclaims_unreachable_cycles() {
        let mut heap = heap();
        let mut statistics = VmStatistics::default();
        let first = heap
            .allocate(
                BytecodeTypeId::new(1),
                HeapObject::Ref(None),
                &[],
                &mut statistics,
            )
            .unwrap();
        let second = heap
            .allocate(
                BytecodeTypeId::new(1),
                HeapObject::Ref(Some(Value::Heap(first))),
                &[Value::Heap(first)],
                &mut statistics,
            )
            .unwrap();
        heap.replace(
            first,
            HeapObject::Ref(Some(Value::Heap(second))),
            &[Value::Heap(first), Value::Heap(second)],
            &mut statistics,
        )
        .unwrap();

        heap.collect(&[Value::Heap(first)], &mut statistics)
            .unwrap();
        assert_eq!(heap.live_objects(), 2);

        heap.collect(&[], &mut statistics).unwrap();
        assert_eq!(heap.live_objects(), 0);
        assert_eq!(statistics.reclaimed_objects, 2);
    }

    #[test]
    fn private_memory_adapter_reclaims_mixed_cycles_under_sustained_pressure() {
        let mut memory = MemoryTestAdapter::new();
        let retained = memory.create_mixed_cycle();
        memory.retain(retained[0]);

        for _ in 0..32 {
            let garbage = memory.create_mixed_cycle();
            memory.apply_pressure(8);

            assert!(retained.iter().all(|handle| memory.is_live(*handle)));
            assert!(garbage.iter().all(|handle| !memory.is_live(*handle)));
        }
        assert_eq!(
            snapshot_value(&Value::Heap(retained[0]), &memory.heap, &[], &[]).unwrap(),
            RuntimeValue::Ref(Some(Box::new(RuntimeValue::Array(vec![
                RuntimeValue::Closure {
                    callable: 7,
                    captures: vec![RuntimeValue::Cycle(0)],
                },
            ]))))
        );

        let reclaimed_before_release = memory.statistics.reclaimed_objects;
        memory.release_all();
        memory.apply_pressure(8);

        assert!(retained.iter().all(|handle| !memory.is_live(*handle)));
        assert!(memory.statistics.collections > 32);
        assert!(
            memory.statistics.reclaimed_objects >= reclaimed_before_release + retained.len() as u64
        );
    }

    #[test]
    fn allocation_collects_once_before_object_limit_success_or_oom() {
        let limits = VmLimits {
            max_heap_objects: 2,
            max_heap_bytes: 16 * 1024,
            initial_gc_threshold: 2,
            ..VmLimits::default()
        };
        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let first = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("first".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let second = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("second".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let replacement = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("replacement".into()),
                &[],
                &mut statistics,
            )
            .unwrap();

        assert!(heap.get(first).is_err());
        assert!(heap.get(second).is_err());
        assert!(matches!(
            heap.get(replacement),
            Ok(HeapObject::String(value)) if value == "replacement"
        ));
        assert_eq!(heap.live_objects(), 1);
        assert_eq!(statistics.allocations, 3);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 2);

        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let first = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("first".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let second = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("second".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let error = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("rejected".into()),
                &[Value::Heap(first), Value::Heap(second)],
                &mut statistics,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            VmError::OutOfMemory {
                live_objects: 2,
                ..
            }
        ));
        assert!(heap.get(first).is_ok());
        assert!(heap.get(second).is_ok());
        assert_eq!(heap.live_objects(), 2);
        assert_eq!(statistics.allocations, 2);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 0);
    }

    #[test]
    fn allocation_collects_once_before_byte_limit_success_or_oom() {
        let [recoverable_old, recoverable_new, retained_old, rejected_new] = [
            reserved_string(32, "recoverable-old"),
            reserved_string(48, "recoverable-new"),
            reserved_string(64, "retained-old"),
            reserved_string(80, "rejected-new"),
        ];
        let max_heap_bytes = [
            &recoverable_old,
            &recoverable_new,
            &retained_old,
            &rejected_new,
        ]
        .into_iter()
        .map(HeapObject::estimated_bytes)
        .max()
        .unwrap();
        let limits = VmLimits {
            max_heap_objects: 8,
            max_heap_bytes,
            initial_gc_threshold: 8,
            ..VmLimits::default()
        };

        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let old = heap
            .allocate(
                BytecodeTypeId::new(0),
                recoverable_old,
                &[],
                &mut statistics,
            )
            .unwrap();
        let new = heap
            .allocate(
                BytecodeTypeId::new(0),
                recoverable_new,
                &[],
                &mut statistics,
            )
            .unwrap();

        assert!(heap.get(old).is_err());
        assert!(heap.get(new).is_ok());
        assert_eq!(statistics.allocations, 2);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 1);

        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let retained = heap
            .allocate(BytecodeTypeId::new(0), retained_old, &[], &mut statistics)
            .unwrap();
        let error = heap
            .allocate(
                BytecodeTypeId::new(0),
                rejected_new,
                &[Value::Heap(retained)],
                &mut statistics,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            VmError::OutOfMemory {
                live_objects: 1,
                ..
            }
        ));
        assert!(heap.get(retained).is_ok());
        assert_eq!(statistics.allocations, 1);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 0);
    }

    #[test]
    fn replacement_collection_protects_target_and_is_atomic_on_oom() {
        let target_object = reserved_string(8, "a");
        let garbage_object = reserved_string(8, "b");
        let replacement_object = reserved_string(64, "grown");
        let max_heap_bytes = target_object
            .estimated_bytes()
            .saturating_add(garbage_object.estimated_bytes())
            .max(replacement_object.estimated_bytes());
        let limits = VmLimits {
            max_heap_objects: 8,
            max_heap_bytes,
            initial_gc_threshold: 8,
            ..VmLimits::default()
        };
        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let target = heap
            .allocate(BytecodeTypeId::new(0), target_object, &[], &mut statistics)
            .unwrap();
        let garbage = heap
            .allocate(
                BytecodeTypeId::new(0),
                garbage_object,
                &[Value::Heap(target)],
                &mut statistics,
            )
            .unwrap();

        heap.replace(target, replacement_object, &[], &mut statistics)
            .unwrap();

        assert!(matches!(
            heap.get(target),
            Ok(HeapObject::String(value)) if value == "grown"
        ));
        assert!(heap.get(garbage).is_err());
        assert_eq!(statistics.allocations, 2);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 1);

        let target_object = reserved_string(8, "a");
        let blocker_object = reserved_string(8, "b");
        let rejected_object = reserved_string(64, "rejected");
        let max_heap_bytes = target_object
            .estimated_bytes()
            .saturating_add(blocker_object.estimated_bytes())
            .max(rejected_object.estimated_bytes());
        let limits = VmLimits {
            max_heap_objects: 8,
            max_heap_bytes,
            initial_gc_threshold: 8,
            ..VmLimits::default()
        };
        let mut heap = string_heap(limits);
        let mut statistics = VmStatistics::default();
        let target = heap
            .allocate(BytecodeTypeId::new(0), target_object, &[], &mut statistics)
            .unwrap();
        let blocker = heap
            .allocate(
                BytecodeTypeId::new(0),
                blocker_object,
                &[Value::Heap(target)],
                &mut statistics,
            )
            .unwrap();
        let error = heap
            .replace(
                target,
                rejected_object,
                &[Value::Heap(blocker)],
                &mut statistics,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            VmError::OutOfMemory {
                live_objects: 2,
                ..
            }
        ));
        assert!(matches!(
            heap.get(target),
            Ok(HeapObject::String(value)) if value == "a"
        ));
        assert!(matches!(
            heap.get(blocker),
            Ok(HeapObject::String(value)) if value == "b"
        ));
        assert_eq!(statistics.allocations, 2);
        assert_eq!(statistics.collections, 1);
        assert_eq!(statistics.reclaimed_objects, 0);
    }

    #[test]
    fn heap_handles_are_non_moving_and_generational() {
        let mut heap = heap();
        let mut statistics = VmStatistics::default();
        let old = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("old".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        heap.collect(&[], &mut statistics).unwrap();
        assert!(matches!(
            heap.descriptor(old),
            Err(VmError::Invariant(message))
                if message == "heap handle refers to a collected object"
        ));
        let new = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("new".into()),
                &[],
                &mut statistics,
            )
            .unwrap();

        assert_eq!(old.index(), new.index());
        assert!(heap.get(old).is_err());
        assert!(matches!(
            heap.descriptor(old),
            Err(VmError::Invariant(message))
                if message == "stale or invalid heap handle"
        ));
        assert!(matches!(heap.get(new), Ok(HeapObject::String(value)) if value == "new"));
    }

    #[test]
    fn closure_environments_trace_and_snapshot_managed_captures() {
        let mut heap = heap();
        let mut statistics = VmStatistics::default();
        let captured = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("captured".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let closure = heap
            .allocate(
                BytecodeTypeId::new(2),
                HeapObject::Closure {
                    callable: BytecodeCallableId::new(7),
                    captures: vec![Some(Value::Heap(captured))],
                },
                &[Value::Heap(captured)],
                &mut statistics,
            )
            .unwrap();

        heap.collect(&[Value::Heap(closure)], &mut statistics)
            .unwrap();
        assert_eq!(heap.live_objects(), 2);
        assert!(matches!(
            heap.get(captured),
            Ok(HeapObject::String(value)) if value == "captured"
        ));
        assert_eq!(
            snapshot_value(&Value::Heap(closure), &heap, &[], &[]).unwrap(),
            RuntimeValue::Closure {
                callable: 7,
                captures: vec![RuntimeValue::String("captured".into())],
            }
        );

        heap.collect(&[], &mut statistics).unwrap();
        assert_eq!(heap.live_objects(), 0);
    }

    #[test]
    fn heap_rejects_objects_that_do_not_match_their_verified_descriptor() {
        let mut heap = heap();
        let mut statistics = VmStatistics::default();

        let error = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::Ref(None),
                &[],
                &mut statistics,
            )
            .unwrap_err();
        assert!(
            matches!(error, VmError::Invariant(message) if message.contains("trace descriptor"))
        );
        assert_eq!(heap.live_objects(), 0);

        let error = heap
            .allocate(
                BytecodeTypeId::new(999),
                HeapObject::String("unknown".into()),
                &[],
                &mut statistics,
            )
            .unwrap_err();
        assert!(
            matches!(error, VmError::Invariant(message) if message.contains("unknown trace descriptor"))
        );
        assert_eq!(heap.live_objects(), 0);

        let string = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("kept".into()),
                &[],
                &mut statistics,
            )
            .unwrap();
        let error = heap
            .replace(
                string,
                HeapObject::Ref(None),
                &[Value::Heap(string)],
                &mut statistics,
            )
            .unwrap_err();
        assert!(
            matches!(error, VmError::Invariant(message) if message.contains("trace descriptor"))
        );
        assert!(matches!(
            heap.get(string),
            Ok(HeapObject::String(value)) if value == "kept"
        ));

        let mut variant_heap = Heap::new(
            limits(),
            vec![BytecodeTraceDescriptor::Variant {
                nominal: None,
                arguments: Vec::new(),
                variants: vec![BytecodeVariant {
                    member: 0,
                    payload: BytecodeVariantPayload::Unit,
                }],
            }],
        );
        let error = variant_heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::Variant {
                    variant: 0,
                    payload: AggregatePayload::Tuple(Vec::new()),
                },
                &[],
                &mut statistics,
            )
            .unwrap_err();
        assert!(matches!(
            error,
            VmError::Invariant(message)
                if message == "heap variant payload does not match its trace descriptor"
        ));
    }
}
