//! Hermetic loader and execution substrate for compile-time Tondo programs.
//!
//! Every call to [`MetaVmProgram::run`] enters the verified bytecode VM through
//! a new engine, heap, scheduler and rejecting host. The loaded program is
//! immutable and carries no host callback or capability slot.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use tondo_vm::bytecode::{
    BytecodeFunctionId, BytecodeInstructionKind, BytecodeIntrinsicType, BytecodeOperation,
    BytecodeOperationKind, BytecodeProgram, BytecodeTerminatorKind, BytecodeTypeKind,
};
use tondo_vm::runtime::{
    RejectingHost, RuntimeValue, VmError, VmLimits, VmOutcome, VmStatistics, execute_with_limits,
};

use crate::driver::{BuildTarget, CapabilityName, HostProfile};
use crate::meta::MetaLimits;

/// Untrusted compiled provider payload. Loading revalidates the complete
/// bytecode program under the orchestrator-owned target and limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaVmArtifact {
    program: BytecodeProgram,
    entry: BytecodeFunctionId,
}

impl MetaVmArtifact {
    pub fn new(program: BytecodeProgram, entry: BytecodeFunctionId) -> Self {
        Self { program, entry }
    }

    pub fn load(self, limits: MetaVmLimits) -> Result<MetaVmProgram, MetaVmError> {
        MetaVmProgram::load(
            &BuildTarget::tondo_meta(),
            HostProfile::Meta,
            &BTreeSet::new(),
            self.program,
            self.entry,
            limits,
        )
    }
}

/// Defensive budgets for one hermetic compile-time execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaVmLimits {
    pub max_verification_steps: u64,
    pub max_steps: u64,
    pub max_stack_depth: u32,
    pub max_heap_objects: u32,
    pub max_live_bytes: u64,
    pub initial_gc_threshold: u32,
    pub max_output_bytes: u64,
}

impl Default for MetaVmLimits {
    fn default() -> Self {
        Self {
            max_verification_steps: 8_000_000,
            max_steps: 10_000_000,
            max_stack_depth: 4_096,
            max_heap_objects: 100_000,
            max_live_bytes: 64 * 1024 * 1024,
            initial_gc_threshold: 256,
            max_output_bytes: 16 * 1024 * 1024,
        }
    }
}

impl MetaVmLimits {
    /// Apply the request's normative budgets while retaining closed structural
    /// VM limits that have no source-level representation.
    pub fn for_request(limits: MetaLimits) -> Self {
        Self {
            max_steps: limits.steps(),
            max_live_bytes: limits.memory_bytes(),
            max_output_bytes: limits.output_bytes(),
            ..Self::default()
        }
    }

    fn vm(self) -> VmLimits {
        VmLimits {
            max_verification_steps: self.max_verification_steps,
            max_steps: self.max_steps,
            max_stack_depth: self.max_stack_depth,
            max_heap_objects: self.max_heap_objects,
            max_heap_bytes: self.max_live_bytes,
            initial_gc_threshold: self.initial_gc_threshold,
        }
    }
}

/// Stable counters exposed to the meta orchestrator and conformance suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaVmCounters {
    pub steps: u64,
    pub peak_live_bytes: u64,
    pub output_bytes: u64,
}

/// One completed hermetic execution.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaVmExecution {
    pub outcome: VmOutcome,
    pub counters: MetaVmCounters,
    pub statistics: VmStatistics,
}

/// Verified immutable program admitted by the closed `tondo-meta/meta` loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaVmProgram {
    program: BytecodeProgram,
    entry: BytecodeFunctionId,
    limits: MetaVmLimits,
}

impl MetaVmProgram {
    pub fn load(
        target: &BuildTarget,
        profile: HostProfile,
        capabilities: &BTreeSet<CapabilityName>,
        program: BytecodeProgram,
        entry: BytecodeFunctionId,
        limits: MetaVmLimits,
    ) -> Result<Self, MetaVmError> {
        if target.name() != BuildTarget::tondo_meta().name() || profile != HostProfile::Meta {
            return Err(MetaVmError::WrongTarget {
                target: target.name().to_owned(),
                profile: profile.as_str(),
            });
        }
        if let Some(capability) = capabilities.first() {
            return Err(MetaVmError::Capability(capability.as_str().to_owned()));
        }
        validate_limits(limits)?;
        validate_program(&program)?;
        if program.function(entry).is_none() {
            return Err(MetaVmError::UnknownEntry(entry.index()));
        }
        Ok(Self {
            program,
            entry,
            limits,
        })
    }

    /// Runs with a fresh VM and a host that rejects every external operation.
    pub fn run(&self) -> Result<MetaVmExecution, MetaVmError> {
        self.run_with_output_meter(outcome_payload_bytes)
    }

    /// Runs hermetically while allowing the trusted orchestrator to measure a
    /// structured result by its semantic payload rather than wire overhead.
    pub fn run_with_output_meter(
        &self,
        meter: impl FnOnce(&VmOutcome) -> Result<u64, MetaVmError>,
    ) -> Result<MetaVmExecution, MetaVmError> {
        let mut host = RejectingHost;
        let execution =
            execute_with_limits(&self.program, self.entry, &mut host, self.limits.vm())?;
        let output_bytes = meter(&execution.outcome)?;
        if output_bytes > self.limits.max_output_bytes {
            return Err(MetaVmError::OutputLimit {
                limit: self.limits.max_output_bytes,
                actual: output_bytes,
            });
        }
        let statistics = execution.statistics;
        Ok(MetaVmExecution {
            outcome: execution.outcome,
            counters: MetaVmCounters {
                steps: statistics.steps,
                peak_live_bytes: statistics.peak_live_bytes,
                output_bytes,
            },
            statistics,
        })
    }
}

#[derive(Debug)]
pub enum MetaVmError {
    WrongTarget {
        target: String,
        profile: &'static str,
    },
    Capability(String),
    InvalidLimit(&'static str),
    ForbiddenType(&'static str),
    ForbiddenOperation(&'static str),
    UnknownEntry(u32),
    HostValue,
    OutputSizeOverflow,
    StructuredOutput(String),
    OutputLimit {
        limit: u64,
        actual: u64,
    },
    Vm(VmError),
}

impl fmt::Display for MetaVmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongTarget { target, profile } => write!(
                formatter,
                "meta bytecode requires target/profile `tondo-meta`/`meta`, found `{target}`/`{profile}`"
            ),
            Self::Capability(capability) => {
                write!(
                    formatter,
                    "tondo-meta cannot load capability `{capability}`"
                )
            }
            Self::InvalidLimit(limit) => write!(formatter, "invalid zero meta VM limit `{limit}`"),
            Self::ForbiddenType(kind) => {
                write!(formatter, "type `{kind}` is forbidden in tondo-meta")
            }
            Self::ForbiddenOperation(operation) => {
                write!(
                    formatter,
                    "operation `{operation}` is forbidden in tondo-meta"
                )
            }
            Self::UnknownEntry(entry) => write!(formatter, "unknown meta bytecode entry f{entry}"),
            Self::HostValue => formatter.write_str("tondo-meta returned an opaque host value"),
            Self::OutputSizeOverflow => {
                formatter.write_str("meta output size is not representable")
            }
            Self::StructuredOutput(error) => {
                write!(formatter, "invalid structured meta output: {error}")
            }
            Self::OutputLimit { limit, actual } => write!(
                formatter,
                "meta output requires {actual} bytes, exceeding the {limit}-byte limit"
            ),
            Self::Vm(error) => error.fmt(formatter),
        }
    }
}

impl Error for MetaVmError {}

impl From<VmError> for MetaVmError {
    fn from(error: VmError) -> Self {
        Self::Vm(error)
    }
}

fn validate_limits(limits: MetaVmLimits) -> Result<(), MetaVmError> {
    for (name, value) in [
        ("max_verification_steps", limits.max_verification_steps),
        ("max_steps", limits.max_steps),
        ("max_stack_depth", u64::from(limits.max_stack_depth)),
        ("max_heap_objects", u64::from(limits.max_heap_objects)),
        ("max_live_bytes", limits.max_live_bytes),
        (
            "initial_gc_threshold",
            u64::from(limits.initial_gc_threshold),
        ),
        ("max_output_bytes", limits.max_output_bytes),
    ] {
        if value == 0 {
            return Err(MetaVmError::InvalidLimit(name));
        }
    }
    Ok(())
}

fn validate_program(program: &BytecodeProgram) -> Result<(), MetaVmError> {
    for ty in &program.types {
        match &ty.kind {
            BytecodeTypeKind::Function(signature) if signature.is_async => {
                return Err(MetaVmError::ForbiddenType("async function"));
            }
            BytecodeTypeKind::Function(signature) if signature.is_unsafe => {
                return Err(MetaVmError::ForbiddenType("unsafe function"));
            }
            BytecodeTypeKind::Intrinsic { constructor, .. }
                if !matches!(
                    constructor,
                    BytecodeIntrinsicType::Array
                        | BytecodeIntrinsicType::Map
                        | BytecodeIntrinsicType::Set
                        | BytecodeIntrinsicType::Range
                        | BytecodeIntrinsicType::Ref
                        | BytecodeIntrinsicType::Bytes
                        | BytecodeIntrinsicType::BytesBuilder
                        | BytecodeIntrinsicType::BytesError
                        | BytecodeIntrinsicType::Path
                        | BytecodeIntrinsicType::PathError
                        | BytecodeIntrinsicType::Utf8Error
                        | BytecodeIntrinsicType::NumericConversionError
                        | BytecodeIntrinsicType::Duration
                        | BytecodeIntrinsicType::DurationError
                        | BytecodeIntrinsicType::MathError
                ) =>
            {
                return Err(MetaVmError::ForbiddenType(intrinsic_name(*constructor)));
            }
            _ => {}
        }
    }
    for function in &program.functions {
        for block in &function.blocks {
            for instruction in &block.instructions {
                if let BytecodeInstructionKind::RegisterDefer { action, .. } = &instruction.kind {
                    validate_operation(action)?;
                }
            }
            match &block.terminator.kind {
                BytecodeTerminatorKind::Invoke { operation, .. } => validate_operation(operation)?,
                BytecodeTerminatorKind::Await { .. } => {
                    return Err(MetaVmError::ForbiddenOperation("await"));
                }
                BytecodeTerminatorKind::Spawn { .. } => {
                    return Err(MetaVmError::ForbiddenOperation("spawn"));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn validate_operation(operation: &BytecodeOperation) -> Result<(), MetaVmError> {
    match &operation.kind {
        BytecodeOperationKind::BootstrapHostCall { function, .. } => {
            Err(MetaVmError::ForbiddenOperation(function.name()))
        }
        BytecodeOperationKind::Call {
            unsafe_call: true, ..
        } => Err(MetaVmError::ForbiddenOperation("unsafe call")),
        _ => Ok(()),
    }
}

fn intrinsic_name(kind: BytecodeIntrinsicType) -> &'static str {
    match kind {
        BytecodeIntrinsicType::Pointer => "Pointer",
        BytecodeIntrinsicType::Join => "Join",
        BytecodeIntrinsicType::Command => "Command",
        BytecodeIntrinsicType::Pipeline => "Pipeline",
        BytecodeIntrinsicType::ExitStatus => "ExitStatus",
        BytecodeIntrinsicType::ProcessOutput => "ProcessOutput",
        BytecodeIntrinsicType::ProcessHandle => "ProcessHandle",
        BytecodeIntrinsicType::ProcessError => "ProcessError",
        BytecodeIntrinsicType::ProcessExitError => "ProcessExitError",
        BytecodeIntrinsicType::Instant => "Instant",
        BytecodeIntrinsicType::Timer => "Timer",
        BytecodeIntrinsicType::ClockError => "ClockError",
        BytecodeIntrinsicType::EnvSnapshot => "EnvSnapshot",
        BytecodeIntrinsicType::EnvName => "EnvName",
        BytecodeIntrinsicType::EnvValue => "EnvValue",
        BytecodeIntrinsicType::EnvError => "EnvError",
        _ => "unknown meta intrinsic",
    }
}

fn outcome_payload_bytes(outcome: &VmOutcome) -> Result<u64, MetaVmError> {
    let VmOutcome::Returned(root) = outcome else {
        return Ok(0);
    };
    let mut total = 0_u64;
    let mut pending = vec![root];
    while let Some(value) = pending.pop() {
        let direct = match value {
            RuntimeValue::Unit | RuntimeValue::OptionNone => 0,
            RuntimeValue::Bool(_) | RuntimeValue::Byte(_) => 1,
            RuntimeValue::Integer(_) => 16,
            RuntimeValue::Float(_) | RuntimeValue::Cycle(_) => 8,
            RuntimeValue::Char(_) => 4,
            RuntimeValue::String(value) => value.len() as u64,
            RuntimeValue::Function {
                name,
                type_arguments,
            } => (name.len() as u64)
                .checked_add((type_arguments.len() as u64).saturating_mul(4))
                .ok_or(MetaVmError::OutputSizeOverflow)?,
            RuntimeValue::Tuple(values)
            | RuntimeValue::Array(values)
            | RuntimeValue::Set(values) => {
                pending.extend(values);
                0
            }
            RuntimeValue::Map(entries) => {
                for (key, value) in entries {
                    pending.push(key);
                    pending.push(value);
                }
                0
            }
            RuntimeValue::Closure { captures, .. } => {
                pending.extend(captures);
                4
            }
            RuntimeValue::Newtype { name, value } => {
                pending.push(value);
                name.len() as u64
            }
            RuntimeValue::Record { name, fields } => {
                for (_, value) in fields {
                    pending.push(value);
                }
                (name.len() as u64).saturating_add((fields.len() as u64).saturating_mul(4))
            }
            RuntimeValue::Variant { payload, .. } => {
                for (_, value) in payload {
                    pending.push(value);
                }
                4_u64.saturating_add((payload.len() as u64).saturating_mul(4))
            }
            RuntimeValue::OptionSome(value)
            | RuntimeValue::ResultOk(value)
            | RuntimeValue::ResultErr(value)
            | RuntimeValue::Ref(Some(value)) => {
                pending.push(value);
                1
            }
            RuntimeValue::Union { value, .. } => {
                pending.push(value);
                4
            }
            RuntimeValue::Range { start, end, .. } => {
                pending.push(start);
                pending.push(end);
                1
            }
            RuntimeValue::Ref(None) => 1,
            RuntimeValue::Host { .. } => return Err(MetaVmError::HostValue),
        };
        total = total
            .checked_add(direct)
            .ok_or(MetaVmError::OutputSizeOverflow)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tondo_vm::bytecode::{
        BytecodeBlock, BytecodeBlockId, BytecodeBlockKind, BytecodeCallable, BytecodeCallableId,
        BytecodeConstant, BytecodeFunction, BytecodeFunctionType, BytecodeInstruction,
        BytecodeInstructionKind, BytecodeOperand, BytecodeOperandKind, BytecodePlace,
        BytecodeRvalue, BytecodeRvalueKind, BytecodeScalarType, BytecodeSlot, BytecodeSlotId,
        BytecodeSlotKind, BytecodeSpan, BytecodeSpanId, BytecodeTerminator, BytecodeType,
        BytecodeTypeId,
    };

    fn unit_program() -> BytecodeProgram {
        let unit = BytecodeTypeId::new(0);
        let function_type = BytecodeTypeId::new(1);
        let span = BytecodeSpan {
            file: 0,
            start: 0,
            end: 1,
        };
        let place = BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty: unit,
            projections: Vec::new(),
            source_loan: None,
        };
        BytecodeProgram {
            types: vec![
                BytecodeType {
                    name: "Unit".into(),
                    kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
                },
                BytecodeType {
                    name: "fn(): Unit".into(),
                    kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                        is_async: false,
                        is_unsafe: false,
                        parameters: Vec::new(),
                        variadic: None,
                        outcome: unit,
                    }),
                },
            ],
            nominals: Vec::new(),
            callables: vec![BytecodeCallable {
                name: "meta_main".into(),
                generic_arity: 0,
                parameters: Vec::new(),
                outcome: unit,
                function_type,
                implementation: Some(BytecodeFunctionId::new(0)),
                closure: None,
            }],
            constants: Vec::new(),
            functions: vec![BytecodeFunction {
                callable: BytecodeCallableId::new(0),
                source: span,
                types: vec![unit, function_type],
                spans: vec![span],
                slots: vec![BytecodeSlot {
                    ty: unit,
                    span: BytecodeSpanId::new(0),
                    kind: BytecodeSlotKind::Return,
                }],
                loans: Vec::new(),
                parameters: Vec::new(),
                return_slot: BytecodeSlotId::new(0),
                entry: BytecodeBlockId::new(0),
                unwind: BytecodeBlockId::new(1),
                blocks: vec![
                    BytecodeBlock {
                        kind: BytecodeBlockKind::Normal,
                        instructions: vec![BytecodeInstruction {
                            span: BytecodeSpanId::new(0),
                            kind: BytecodeInstructionKind::Store {
                                destination: place,
                                value: BytecodeRvalue {
                                    ty: unit,
                                    kind: BytecodeRvalueKind::Use(BytecodeOperand {
                                        ty: unit,
                                        kind: BytecodeOperandKind::Constant(BytecodeConstant::Unit),
                                    }),
                                },
                            },
                        }],
                        terminator: BytecodeTerminator {
                            span: BytecodeSpanId::new(0),
                            kind: BytecodeTerminatorKind::Return,
                        },
                    },
                    BytecodeBlock {
                        kind: BytecodeBlockKind::Cleanup,
                        instructions: Vec::new(),
                        terminator: BytecodeTerminator {
                            span: BytecodeSpanId::new(0),
                            kind: BytecodeTerminatorKind::ResumePanic,
                        },
                    },
                ],
            }],
        }
    }

    fn load(program: BytecodeProgram, limits: MetaVmLimits) -> Result<MetaVmProgram, MetaVmError> {
        MetaVmProgram::load(
            &BuildTarget::tondo_meta(),
            HostProfile::Meta,
            &BTreeSet::new(),
            program,
            BytecodeFunctionId::new(0),
            limits,
        )
    }

    #[test]
    fn target_is_closed_to_meta_profile_and_zero_capabilities() {
        let target = BuildTarget::tondo_meta();
        assert!(target.supports_profile(HostProfile::Meta));
        assert!(!target.supports_profile(HostProfile::Hosted));
        assert!(target.supported_capabilities().is_empty());
        assert!(matches!(
            MetaVmProgram::load(
                &BuildTarget::vm_hosted(),
                HostProfile::Hosted,
                &BTreeSet::new(),
                unit_program(),
                BytecodeFunctionId::new(0),
                MetaVmLimits::default(),
            ),
            Err(MetaVmError::WrongTarget { .. })
        ));
    }

    #[test]
    fn every_run_has_fresh_deterministic_counters() {
        let program = load(unit_program(), MetaVmLimits::default()).unwrap();
        let first = program.run().unwrap();
        let second = program.run().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.outcome, VmOutcome::Returned(RuntimeValue::Unit));
        assert!(first.counters.steps > 0);
        assert_eq!(first.counters.peak_live_bytes, 0);
        assert_eq!(first.counters.output_bytes, 0);
    }

    #[test]
    fn artifacts_reload_with_orchestrator_owned_limits_and_semantic_metering() {
        let request_limits = MetaLimits::new(123, 4_096, 17).unwrap();
        let limits = MetaVmLimits::for_request(request_limits);
        assert_eq!(limits.max_steps, 123);
        assert_eq!(limits.max_live_bytes, 4_096);
        assert_eq!(limits.max_output_bytes, 17);
        let artifact = MetaVmArtifact::new(unit_program(), BytecodeFunctionId::new(0));
        let program = artifact.load(limits).unwrap();
        let execution = program.run_with_output_meter(|_| Ok(7)).unwrap();
        assert_eq!(execution.counters.output_bytes, 7);
        assert_eq!(
            MetaVmError::StructuredOutput("bad response".into()).to_string(),
            "invalid structured meta output: bad response"
        );
    }

    #[test]
    fn loader_rejects_forbidden_types_limits_and_entries() {
        let mut pointer = unit_program();
        pointer.types.push(BytecodeType {
            name: "Pointer[Unit]".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Pointer,
                arguments: vec![BytecodeTypeId::new(0)],
            },
        });
        assert!(matches!(
            load(pointer, MetaVmLimits::default()),
            Err(MetaVmError::ForbiddenType("Pointer"))
        ));

        let limits = MetaVmLimits {
            max_steps: 0,
            ..MetaVmLimits::default()
        };
        assert!(matches!(
            load(unit_program(), limits),
            Err(MetaVmError::InvalidLimit("max_steps"))
        ));
        assert!(matches!(
            MetaVmProgram::load(
                &BuildTarget::tondo_meta(),
                HostProfile::Meta,
                &BTreeSet::new(),
                unit_program(),
                BytecodeFunctionId::new(9),
                MetaVmLimits::default(),
            ),
            Err(MetaVmError::UnknownEntry(9))
        ));
    }

    #[test]
    fn output_meter_is_iterative_and_enforces_the_budget() {
        let mut value = RuntimeValue::String("abc".into());
        for _ in 0..10_000 {
            value = RuntimeValue::OptionSome(Box::new(value));
        }
        assert_eq!(
            outcome_payload_bytes(&VmOutcome::Returned(value)).unwrap(),
            10_003
        );
        assert!(matches!(
            outcome_payload_bytes(&VmOutcome::Returned(RuntimeValue::Host {
                kind: tondo_vm::runtime::RuntimeHostValueKind::Bytes,
                id: 1,
            })),
            Err(MetaVmError::HostValue)
        ));
    }
}
