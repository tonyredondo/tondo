//! Execution engine and managed object model for verified Tondo bytecode.
//!
//! The bootstrap VM keeps values explicit, uses typed frame slots, and owns a
//! precise non-moving tracing heap. Bytecode is verified again at this trust
//! boundary even when it originated in the reference compiler.

use std::error::Error;
use std::fmt;

use crate::bytecode::{BytecodeSpan, BytecodeVerificationError};

mod execute;
mod heap;
mod value;

#[cfg(feature = "conformance")]
pub mod conformance;

pub use execute::{
    RejectingHost, VmExecution, VmHost, VmOutcome, VmTestNodeKind, VmTestNodeOutcome, execute,
    execute_with_limits, execute_with_limits_and_copy_strategy,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmLimits {
    pub max_verification_steps: u64,
    pub max_steps: u64,
    pub max_stack_depth: u32,
    pub max_heap_objects: u32,
    pub max_heap_bytes: u64,
    pub initial_gc_threshold: u32,
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

/// Closed identities for opaque values exchanged with the hosted standard
/// library. The payload remains in the host registry; bytecode carries only a
/// typed run-local token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeHostValueKind {
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
    FloatToleranceError,
    TextDiff,
    TempDirectory,
    TempError,
    Generator,
    GenerationId,
    GenerationError,
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
}

#[derive(Debug)]
pub enum VmError {
    InvalidBytecode(BytecodeVerificationError),
    InvalidLimits(&'static str),
    InvalidEntry(String),
    ResourceLimit { resource: &'static str, limit: u64 },
    OutOfMemory { live_objects: u32, live_bytes: u64 },
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
