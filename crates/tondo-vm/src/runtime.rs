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
mod literal;
mod value;

pub use execute::{RejectingHost, VmExecution, VmHost, VmOutcome, execute, execute_with_limits};

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
    Record {
        name: String,
        fields: Vec<(u32, Self)>,
    },
    Variant {
        variant: u32,
        payload: Vec<(Option<u32>, Self)>,
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
    /// Back-reference used only when snapshotting an identity graph with a cycle.
    Cycle(usize),
}

/// Per-run counters useful for testing limits and collector behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VmStatistics {
    pub steps: u64,
    pub allocations: u64,
    pub collections: u64,
    pub reclaimed_objects: u64,
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
    use super::heap::{Heap, HeapObject};
    use super::value::{Value, snapshot_value};
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
    fn precise_heap_keeps_reachable_objects_and_reclaims_unreachable_cycles() {
        let mut heap = Heap::new(limits());
        let mut statistics = VmStatistics::default();
        let first = heap
            .allocate(HeapObject::Ref(None), &[], &mut statistics)
            .unwrap();
        let second = heap
            .allocate(
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
    fn heap_handles_are_non_moving_and_generational() {
        let mut heap = Heap::new(limits());
        let mut statistics = VmStatistics::default();
        let old = heap
            .allocate(HeapObject::String("old".into()), &[], &mut statistics)
            .unwrap();
        heap.collect(&[], &mut statistics).unwrap();
        let new = heap
            .allocate(HeapObject::String("new".into()), &[], &mut statistics)
            .unwrap();

        assert_eq!(old.index(), new.index());
        assert!(heap.get(old).is_err());
        assert!(matches!(heap.get(new), Ok(HeapObject::String(value)) if value == "new"));
    }

    #[test]
    fn closure_environments_trace_and_snapshot_managed_captures() {
        let mut heap = Heap::new(limits());
        let mut statistics = VmStatistics::default();
        let captured = heap
            .allocate(HeapObject::String("captured".into()), &[], &mut statistics)
            .unwrap();
        let closure = heap
            .allocate(
                HeapObject::Closure {
                    callable: crate::bytecode::BytecodeCallableId::new(7),
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
}
