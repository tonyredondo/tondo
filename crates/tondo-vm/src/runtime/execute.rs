use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

use crate::bytecode::{
    ArraySliceError, BytecodeAggregateKind, BytecodeArraySequenceKind, BytecodeAwaitable,
    BytecodeBinaryOperator, BytecodeBlockId, BytecodeBootstrapHostFunction, BytecodeCallArgument,
    BytecodeCallArgumentTarget, BytecodeCallable, BytecodeCoercion, BytecodeConstant,
    BytecodeConstantValue, BytecodeConstantValueKind, BytecodeConstantVariantValue,
    BytecodeContainmentKind, BytecodeCursorMode, BytecodeFunctionId, BytecodeIndexAccess,
    BytecodeInstruction, BytecodeInstructionKind, BytecodeIntrinsicType, BytecodeLoanId,
    BytecodeLoanKind, BytecodeNominalShape, BytecodeNumericConversion,
    BytecodeNumericConversionError, BytecodeOperand, BytecodeOperandKind, BytecodeOperation,
    BytecodeOperationKind, BytecodeParameterMode, BytecodePlace, BytecodePrefixOperator,
    BytecodeProgram, BytecodeProjection, BytecodeProjectionKind, BytecodeRangeKind, BytecodeRvalue,
    BytecodeRvalueKind, BytecodeScalarType, BytecodeScopeId, BytecodeSlotId, BytecodeSpan,
    BytecodeTag, BytecodeTerminator, BytecodeTerminatorKind, BytecodeTraceMetadata, BytecodeTypeId,
    BytecodeTypeKind, BytecodeVariantPayload, BytecodeVerificationLimits, normalize_array_index,
    normalize_array_slice_indices, verify_bytecode_with_trace_metadata,
};
use crate::literal;

use super::heap::{Heap, HeapHandle, HeapObject, IteratorAdapter};
use super::value::{
    AggregatePayload, RuntimeJoin, RuntimeLoan, TRANSFERRED_JOIN_SCOPE, Value, snapshot_value,
};
use super::{
    PanicCode, RuntimeHostValueKind, RuntimeValue, ValueCopyStrategy, VmError, VmLimits, VmPanic,
    VmStackFrame, VmStatistics,
};

type HeapMapEntry = (Option<Value>, Option<Value>);

/// Host boundary for callables that deliberately have no bytecode body.
///
/// Arguments and results are detached snapshots. A host may retain or mutate
/// its own values, but it never receives a VM heap handle and therefore cannot
/// keep a managed object alive accidentally.
pub trait VmHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError>;

    /// Starts work that may block independently of the cooperative executor.
    fn start_async(&mut self, name: &str, _arguments: &[RuntimeValue]) -> Result<u64, VmError> {
        Err(VmError::UnsupportedHostCall(name.to_owned()))
    }

    /// Returns a completed async result without blocking.
    fn poll_async(&mut self, _call: u64) -> Result<Option<RuntimeValue>, VmError> {
        Ok(None)
    }

    /// Waits until one of the supplied host calls completes.
    fn wait_async(&mut self, calls: &[u64]) -> Result<(u64, RuntimeValue), VmError> {
        let call = calls
            .first()
            .copied()
            .ok_or_else(|| VmError::invariant("host wait received no calls"))?;
        Err(VmError::UnsupportedHostCall(format!(
            "async host call #{call}"
        )))
    }

    /// Requests cancellation without reporting completion before cleanup ends.
    fn cancel_async(&mut self, _call: u64) -> Result<(), VmError> {
        Ok(())
    }

    /// Defensively releases a terminal host value during VM unwinding.
    fn cleanup(&mut self, _value: &RuntimeValue) -> Result<(), VmError> {
        Ok(())
    }

    /// Enters the single deterministic clock domain owned by a test attempt.
    fn begin_virtual_time(&mut self) -> Result<RuntimeValue, VmError> {
        Err(VmError::UnsupportedHostCall(
            "std.testing.withVirtualTime".to_owned(),
        ))
    }

    /// Restores the production clock after the virtual-time closure completes
    /// or unwinds.
    fn finish_virtual_time(&mut self, _controller: &RuntimeValue) -> Result<(), VmError> {
        Err(VmError::UnsupportedHostCall(
            "std.testing.withVirtualTime".to_owned(),
        ))
    }

    /// Identifies controller waits that may only complete after every runnable
    /// language task has reached durable quiescence.
    fn is_virtual_quiescence_call(&self, _call: u64) -> bool {
        false
    }

    /// Enters a compiler-owned test node boundary. Normal hosts never expose
    /// this operation; it is used only by verified test artifacts.
    fn begin_test_node(&mut self, _kind: VmTestNodeKind, id: &str) -> Result<(), VmError> {
        Err(VmError::UnsupportedHostCall(format!("test node `{id}`")))
    }

    /// Completes the active compiler-owned test node after all language
    /// cleanup has run.
    fn finish_test_node(
        &mut self,
        _kind: VmTestNodeKind,
        id: &str,
        _outcome: VmTestNodeOutcome,
    ) -> Result<(), VmError> {
        Err(VmError::UnsupportedHostCall(format!("test node `{id}`")))
    }

    /// Marks the transition from selected descendants to suite cleanup.
    fn begin_test_suite_cleanup(&mut self) -> Result<(), VmError> {
        Err(VmError::UnsupportedHostCall(
            "test suite cleanup".to_owned(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmTestNodeKind {
    Leaf,
    Suite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmTestNodeOutcome {
    Passed,
    Panicked(VmPanic),
}

#[derive(Debug, Default)]
pub struct RejectingHost;

impl VmHost for RejectingHost {
    fn invoke(&mut self, name: &str, _arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        Err(VmError::UnsupportedHostCall(name.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum VmOutcome {
    Returned(RuntimeValue),
    Panicked(VmPanic),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VmExecution {
    pub outcome: VmOutcome,
    pub statistics: VmStatistics,
}

pub fn execute(
    program: &BytecodeProgram,
    entry: BytecodeFunctionId,
    host: &mut dyn VmHost,
) -> Result<VmExecution, VmError> {
    execute_with_limits(program, entry, host, VmLimits::default())
}

pub fn execute_with_limits(
    program: &BytecodeProgram,
    entry: BytecodeFunctionId,
    host: &mut dyn VmHost,
    limits: VmLimits,
) -> Result<VmExecution, VmError> {
    execute_with_limits_and_copy_strategy(
        program,
        entry,
        host,
        limits,
        ValueCopyStrategy::default(),
    )
}

/// Executes verified bytecode with an explicit physical value-copy strategy.
///
/// This entry point exists so the eager reference implementation and the COW
/// implementation can run the same black-box conformance corpus.
pub fn execute_with_limits_and_copy_strategy(
    program: &BytecodeProgram,
    entry: BytecodeFunctionId,
    host: &mut dyn VmHost,
    limits: VmLimits,
    copy_strategy: ValueCopyStrategy,
) -> Result<VmExecution, VmError> {
    validate_limits(limits)?;
    let trace = verify_bytecode_with_trace_metadata(
        program,
        BytecodeVerificationLimits {
            max_dataflow_steps: limits.max_verification_steps,
        },
    )?;
    validate_entry_contract(program, entry)?;
    Engine::new(program, host, limits, copy_strategy, trace).run(entry)
}

fn validate_entry_contract(
    program: &BytecodeProgram,
    entry: BytecodeFunctionId,
) -> Result<(), VmError> {
    let function = program
        .function(entry)
        .ok_or_else(|| VmError::InvalidEntry(format!("unknown function {}", entry.index())))?;
    let callable = program.callable(function.callable).ok_or_else(|| {
        VmError::InvalidEntry("the selected function has no callable contract".into())
    })?;
    let signature = program.ty(callable.function_type).ok_or_else(|| {
        VmError::InvalidEntry("the selected callable has no function type".into())
    })?;
    let BytecodeTypeKind::Function(signature) = &signature.kind else {
        return Err(VmError::InvalidEntry(
            "the selected callable contract is not a function".into(),
        ));
    };
    if signature.is_unsafe {
        return Err(VmError::InvalidEntry(
            "the selected callable requires an unsafe execution context".into(),
        ));
    }
    Ok(())
}

fn validate_limits(limits: VmLimits) -> Result<(), VmError> {
    for (name, value) in [
        ("max_verification_steps", limits.max_verification_steps),
        ("max_steps", limits.max_steps),
        ("max_stack_depth", u64::from(limits.max_stack_depth)),
        ("max_heap_objects", u64::from(limits.max_heap_objects)),
        ("max_heap_bytes", limits.max_heap_bytes),
        (
            "initial_gc_threshold",
            u64::from(limits.initial_gc_threshold),
        ),
    ] {
        if value == 0 {
            return Err(VmError::InvalidLimits(name));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum SlotState {
    Dead,
    Uninitialized,
    Value(Value),
}

#[derive(Debug, Clone)]
struct CallContinuation {
    destination: Option<BytecodePlace>,
    target: Option<BytecodeBlockId>,
    unwind: BytecodeBlockId,
    call_span: BytecodeSpan,
    test_boundary: Option<TestBoundary>,
    virtual_time: Option<RuntimeValue>,
}

#[derive(Debug, Clone)]
struct TestBoundary {
    kind: VmTestNodeKind,
    id: String,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeReservation {
    mode: BytecodeParameterMode,
    path: ResolvedPlacePath,
}

#[derive(Debug)]
enum DeferredValue {
    Captured(Value),
    Guard,
}

impl DeferredValue {
    fn roots(&self, output: &mut Vec<Value>) {
        if let Self::Captured(value) = self {
            output.push(value.clone());
        }
    }
}

#[derive(Debug)]
struct DeferredCallArgument {
    target: BytecodeCallArgumentTarget,
    value: DeferredValue,
}

#[derive(Debug)]
enum DeferredOperation {
    Call {
        callee: DeferredValue,
        arguments: Vec<DeferredCallArgument>,
    },
    Assert {
        condition: DeferredValue,
        condition_repr: String,
        message_parts: Vec<(DeferredValue, bool)>,
    },
    BootstrapHostCall {
        function: BytecodeBootstrapHostFunction,
        arguments: Vec<DeferredValue>,
    },
}

impl DeferredOperation {
    fn roots(&self, output: &mut Vec<Value>) {
        match self {
            Self::Call { callee, arguments } => {
                callee.roots(output);
                for argument in arguments {
                    argument.value.roots(output);
                }
            }
            Self::Assert {
                condition,
                message_parts,
                ..
            } => {
                condition.roots(output);
                for (value, _) in message_parts {
                    value.roots(output);
                }
            }
            Self::BootstrapHostCall { arguments, .. } => {
                for value in arguments {
                    value.roots(output);
                }
            }
        }
    }
}

#[derive(Debug)]
struct RuntimeDefer {
    scope: BytecodeScopeId,
    span: BytecodeSpan,
    operation: DeferredOperation,
    guard: Option<BytecodePlace>,
    /// `true` only for a verified `defer await` call.  The bit is derived from
    /// the call signature rather than carried as syntax metadata, keeping the
    /// bytecode representation stable while making cleanup scheduling explicit
    /// at runtime.
    async_cleanup: bool,
}

impl RuntimeDefer {
    fn roots(&self, output: &mut Vec<Value>) {
        self.operation.roots(output);
    }
}

#[derive(Debug, Clone)]
struct RuntimeFallback {
    scope: BytecodeScopeId,
    owner: BytecodePlace,
}

#[derive(Debug, Clone)]
struct RuntimeType {
    ty: BytecodeTypeId,
    substitutions: Vec<RuntimeType>,
}

impl RuntimeType {
    fn child(&self, ty: BytecodeTypeId) -> Self {
        Self {
            ty,
            substitutions: self.substitutions.clone(),
        }
    }
}

#[derive(Debug)]
enum RuntimeCleanup {
    Explicit(RuntimeDefer),
    Fallback(RuntimeFallback),
}

impl RuntimeCleanup {
    fn scope(&self) -> BytecodeScopeId {
        match self {
            Self::Explicit(deferred) => deferred.scope,
            Self::Fallback(fallback) => fallback.scope,
        }
    }

    fn guard(&self) -> Option<&BytecodePlace> {
        match self {
            Self::Explicit(deferred) => deferred.guard.as_ref(),
            Self::Fallback(fallback) => Some(&fallback.owner),
        }
    }

    fn guard_mut(&mut self) -> Option<&mut BytecodePlace> {
        match self {
            Self::Explicit(deferred) => deferred.guard.as_mut(),
            Self::Fallback(fallback) => Some(&mut fallback.owner),
        }
    }

    fn roots(&self, output: &mut Vec<Value>) {
        if let Self::Explicit(deferred) = self {
            deferred.roots(output);
        }
    }
}

#[derive(Debug)]
struct Frame {
    function: BytecodeFunctionId,
    block: BytecodeBlockId,
    instruction: usize,
    slots: Vec<SlotState>,
    loans: Vec<Option<RuntimeReservation>>,
    cleanups: Vec<RuntimeCleanup>,
    task_scopes: Vec<usize>,
    continuation: Option<CallContinuation>,
}

impl Frame {
    fn roots(
        &self,
        trace: &crate::bytecode::BytecodeFrameTraceDescriptor,
        output: &mut Vec<Value>,
    ) {
        output.extend(
            trace
                .slots
                .iter()
                .zip(&self.slots)
                .filter_map(|(_, slot)| match slot {
                    SlotState::Value(value) => Some(value.clone()),
                    SlotState::Dead | SlotState::Uninitialized => None,
                }),
        );
        for cleanup in &self.cleanups {
            cleanup.roots(output);
        }
    }
}

#[derive(Debug)]
enum TaskCompletion {
    Returned(Value),
    Panicked(VmPanic),
    Cancelled,
}

#[derive(Debug)]
enum RuntimeUnwind {
    Panic(VmPanic),
    Cancelled,
}

#[derive(Debug)]
enum TaskWait {
    Join {
        child: usize,
        owner: BytecodePlace,
        destination: BytecodePlace,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    HostCall {
        call: u64,
        outcome: BytecodeTypeId,
        destination: BytecodePlace,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
        completion: Option<RuntimeValue>,
    },
    /// A host operation started by `defer await`.  Cleanup must be allowed to
    /// finish even when the surrounding task is already unwinding, so it has
    /// its own wait state instead of reusing the cancellable ordinary await.
    DeferredHostCall {
        call: u64,
        outcome: BytecodeTypeId,
        target: BytecodeBlockId,
        completion: Option<RuntimeValue>,
    },
    HostTask {
        call: u64,
        outcome: BytecodeTypeId,
    },
    OneShot {
        id: u64,
        outcome: BytecodeTypeId,
        destination: BytecodePlace,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    },
    OneShotTask {
        id: u64,
        outcome: BytecodeTypeId,
    },
    Scope,
}

#[derive(Debug, Clone)]
enum OneShotCompletion {
    Ok(Value),
    Err(Value),
    Cancelled,
}

#[derive(Debug, Default)]
struct OneShotState {
    completion: Option<OneShotCompletion>,
    waiter_tasks: Vec<usize>,
    waiter_consumed: bool,
}

#[derive(Debug)]
enum TaskStatus {
    Running,
    Runnable,
    Waiting(TaskWait),
    Complete(Option<TaskCompletion>),
    Consumed,
}

#[derive(Debug)]
struct TaskRecord {
    frames: Vec<Frame>,
    pending_unwind: Option<RuntimeUnwind>,
    status: TaskStatus,
    resume: Option<TaskWait>,
    queued: bool,
    cancel_requested: bool,
    waiters: Vec<usize>,
    parent_scope: Option<usize>,
    join_consumed: bool,
    panic_observed: bool,
}

#[derive(Debug)]
struct RuntimeTaskScope {
    source: BytecodeScopeId,
    owner: usize,
    children: Vec<usize>,
    closed: bool,
}

struct Engine<'program, 'host> {
    program: &'program BytecodeProgram,
    host: &'host mut dyn VmHost,
    limits: VmLimits,
    copy_strategy: ValueCopyStrategy,
    heap: Heap,
    frames: Vec<Frame>,
    frame_traces: Vec<crate::bytecode::BytecodeFrameTraceDescriptor>,
    temporary_roots: Vec<Value>,
    pending_unwind: Option<RuntimeUnwind>,
    tasks: Vec<TaskRecord>,
    runnable: VecDeque<usize>,
    current_task: usize,
    task_scopes: Vec<Option<RuntimeTaskScope>>,
    oneshots: BTreeMap<u64, OneShotState>,
    next_oneshot_id: u64,
    statistics: VmStatistics,
    callable_names: Vec<String>,
    nominal_names: Vec<String>,
}

impl<'program, 'host> Engine<'program, 'host> {
    fn new(
        program: &'program BytecodeProgram,
        host: &'host mut dyn VmHost,
        limits: VmLimits,
        copy_strategy: ValueCopyStrategy,
        trace: BytecodeTraceMetadata,
    ) -> Self {
        Self {
            program,
            host,
            limits,
            copy_strategy,
            heap: Heap::new(limits, trace.types),
            frames: Vec::new(),
            frame_traces: trace.frames,
            temporary_roots: Vec::new(),
            pending_unwind: None,
            tasks: Vec::new(),
            runnable: VecDeque::new(),
            current_task: 0,
            task_scopes: Vec::new(),
            oneshots: BTreeMap::new(),
            next_oneshot_id: 1,
            statistics: VmStatistics::default(),
            callable_names: program
                .callables
                .iter()
                .map(|callable| callable.name.clone())
                .collect(),
            nominal_names: program
                .nominals
                .iter()
                .map(|nominal| nominal.name.clone())
                .collect(),
        }
    }

    fn run(mut self, entry: BytecodeFunctionId) -> Result<VmExecution, VmError> {
        let entry_function = self
            .program
            .function(entry)
            .ok_or_else(|| VmError::InvalidEntry(format!("unknown function {}", entry.index())))?;
        if !entry_function.parameters.is_empty() {
            return Err(VmError::InvalidEntry(
                "the selected function requires parameters".into(),
            ));
        }
        self.tasks.push(TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status: TaskStatus::Running,
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: None,
            join_consumed: true,
            panic_observed: false,
        });
        self.push_frame(entry, Vec::new(), None)?;

        loop {
            if !self.resume_current_task()? {
                if let Some(execution) = self.schedule_next()? {
                    return Ok(execution);
                }
                continue;
            }
            self.step_budget()?;
            let frame_index = self
                .frames
                .len()
                .checked_sub(1)
                .ok_or_else(|| VmError::invariant("execution lost its root frame"))?;
            let (function_id, block_id, instruction_index) = {
                let frame = &self.frames[frame_index];
                (frame.function, frame.block, frame.instruction)
            };
            let function = self
                .program
                .function(function_id)
                .ok_or_else(|| VmError::invariant("frame has an invalid function"))?;
            let block = function
                .block(block_id)
                .ok_or_else(|| VmError::invariant("frame has an invalid block"))?;
            if let Some(instruction) = block.instructions.get(instruction_index).cloned() {
                self.frames[frame_index].instruction += 1;
                self.execute_instruction(frame_index, &instruction)?;
            } else {
                let terminator = block.terminator.clone();
                if let Some(completion) = self.execute_terminator(frame_index, &terminator)? {
                    self.complete_current_task(completion)?;
                }
            }
            if let Some(execution) = self.schedule_next()? {
                return Ok(execution);
            }
        }
    }

    fn resume_current_task(&mut self) -> Result<bool, VmError> {
        let wait = self
            .tasks
            .get_mut(self.current_task)
            .ok_or_else(|| VmError::invariant("the active task is missing"))?
            .resume
            .take();
        let Some(wait) = wait else {
            return Ok(true);
        };
        match wait {
            TaskWait::Scope => Ok(true),
            TaskWait::HostTask { .. } => Err(VmError::invariant(
                "a host-only child task entered the runnable queue",
            )),
            TaskWait::OneShot {
                id,
                outcome,
                destination,
                target,
                unwind,
            } => {
                let frame =
                    self.frames.len().checked_sub(1).ok_or_else(|| {
                        VmError::invariant("a resumed one-shot await has no frame")
                    })?;
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    self.begin_cancel(frame, unwind)?;
                    return Ok(true);
                }
                let completion = self
                    .oneshots
                    .get(&id)
                    .ok_or_else(|| VmError::invariant("one-shot wait references an unknown id"))?
                    .completion
                    .clone();
                match completion {
                    None => {
                        self.park_current(
                            TaskWait::OneShot {
                                id,
                                outcome,
                                destination,
                                target,
                                unwind,
                            },
                            &[],
                        )?;
                        Ok(false)
                    }
                    Some(OneShotCompletion::Ok(value)) => {
                        let result = self.oneshot_result(outcome, Ok(value))?;
                        self.write_place(frame, &destination, result)?;
                        self.jump(frame, target);
                        Ok(true)
                    }
                    Some(OneShotCompletion::Cancelled) => {
                        self.begin_cancel(frame, unwind)?;
                        Ok(true)
                    }
                    Some(OneShotCompletion::Err(error)) => {
                        let result = self.oneshot_result(outcome, Err(error))?;
                        self.write_place(frame, &destination, result)?;
                        self.jump(frame, target);
                        Ok(true)
                    }
                }
            }
            TaskWait::OneShotTask { id, outcome } => {
                if self.tasks[self.current_task].cancel_requested {
                    self.complete_current_task(TaskCompletion::Cancelled)?;
                    return Ok(false);
                }
                let completion = self
                    .oneshots
                    .get(&id)
                    .ok_or_else(|| VmError::invariant("one-shot task references an unknown id"))?
                    .completion
                    .clone();
                match completion {
                    None => {
                        self.park_current(TaskWait::OneShotTask { id, outcome }, &[])?;
                        Ok(false)
                    }
                    Some(OneShotCompletion::Ok(value)) => {
                        let result = self.oneshot_result(outcome, Ok(value))?;
                        self.complete_current_task(TaskCompletion::Returned(result))?;
                        Ok(false)
                    }
                    Some(OneShotCompletion::Cancelled) => {
                        self.complete_current_task(TaskCompletion::Cancelled)?;
                        Ok(false)
                    }
                    Some(OneShotCompletion::Err(error)) => {
                        let result = self.oneshot_result(outcome, Err(error))?;
                        self.complete_current_task(TaskCompletion::Returned(result))?;
                        Ok(false)
                    }
                }
            }
            TaskWait::HostCall {
                call,
                outcome,
                destination,
                target,
                unwind,
                completion,
            } => {
                let frame = self
                    .frames
                    .len()
                    .checked_sub(1)
                    .ok_or_else(|| VmError::invariant("a resumed host await has no frame"))?;
                let Some(completion) = completion else {
                    return Err(VmError::invariant(format!(
                        "host call #{call} resumed before completion"
                    )));
                };
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    self.host.cleanup(&completion)?;
                    self.begin_cancel(frame, unwind)?;
                    return Ok(true);
                }
                let value = self.materialize_host_value(outcome, completion)?;
                self.write_place(frame, &destination, value)?;
                self.jump(frame, target);
                Ok(true)
            }
            TaskWait::DeferredHostCall {
                call,
                outcome,
                target,
                completion,
            } => {
                let frame = self.frames.len().checked_sub(1).ok_or_else(|| {
                    VmError::invariant("a resumed deferred host cleanup has no frame")
                })?;
                let Some(completion) = completion else {
                    return Err(VmError::invariant(format!(
                        "deferred host call #{call} resumed before completion"
                    )));
                };
                let value = self.materialize_host_value(outcome, completion)?;
                if value != Value::Unit {
                    return Err(VmError::invariant(
                        "deferred async host call returned a non-Unit value",
                    ));
                }
                self.jump(frame, target);
                Ok(true)
            }
            TaskWait::Join {
                child,
                owner,
                destination,
                target,
                unwind,
            } => {
                let frame = self
                    .frames
                    .len()
                    .checked_sub(1)
                    .ok_or_else(|| VmError::invariant("a resumed await has no frame"))?;
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    self.begin_cancel(frame, unwind)?;
                    return Ok(true);
                }
                if !matches!(
                    self.tasks.get(child).map(|task| &task.status),
                    Some(TaskStatus::Complete(_))
                ) {
                    self.park_current(
                        TaskWait::Join {
                            child,
                            owner,
                            destination,
                            target,
                            unwind,
                        },
                        &[child],
                    )?;
                    return Ok(false);
                }
                let join = self.consume_join_owner(frame, &owner)?;
                if join.task != child {
                    return Err(VmError::invariant(
                        "resumed Join owner changed its child identity",
                    ));
                }
                let parent_scope = self.tasks[child].parent_scope;
                let completion = self
                    .take_task_completion(child)?
                    .ok_or_else(|| VmError::invariant("woken Join has no completion"))?;
                self.apply_join_completion(frame, completion, &destination, target, unwind)?;
                if let Some(scope) = parent_scope
                    && self.task_scopes.get(scope).is_some_and(Option::is_some)
                {
                    self.release_task_scope_if_consumed(scope)?;
                }
                Ok(true)
            }
        }
    }

    fn schedule_next(&mut self) -> Result<Option<VmExecution>, VmError> {
        if matches!(
            self.tasks.get(self.current_task).map(|task| &task.status),
            Some(TaskStatus::Running)
        ) {
            self.tasks[self.current_task].status = TaskStatus::Runnable;
            self.enqueue_task(self.current_task)?;
        }
        {
            let task = self
                .tasks
                .get_mut(self.current_task)
                .ok_or_else(|| VmError::invariant("the active task disappeared"))?;
            task.frames = std::mem::take(&mut self.frames);
            task.pending_unwind = self.pending_unwind.take();
        }

        self.poll_host_calls()?;
        loop {
            while let Some(next) = self.runnable.pop_front() {
                let task = self.tasks.get_mut(next).ok_or_else(|| {
                    VmError::invariant("the runnable queue contains an invalid task")
                })?;
                task.queued = false;
                if !matches!(task.status, TaskStatus::Runnable) {
                    continue;
                }
                task.status = TaskStatus::Running;
                self.current_task = next;
                self.frames = std::mem::take(&mut task.frames);
                self.pending_unwind = task.pending_unwind.take();
                return Ok(None);
            }

            if matches!(
                self.tasks.first().map(|task| &task.status),
                Some(TaskStatus::Complete(_))
            ) {
                return self.finish_root_task().map(Some);
            }
            let calls = self.pending_host_calls();
            if calls.is_empty() {
                return Err(VmError::invariant(
                    "the cooperative executor has no runnable task before root completion",
                ));
            }
            let (call, value) = self.host.wait_async(&calls)?;
            if !calls.contains(&call) {
                return Err(VmError::Host(format!(
                    "host completed unknown async call #{call}"
                )));
            }
            self.complete_host_call(call, value)?;
            self.poll_host_calls()?;
        }
    }

    fn pending_host_calls(&self) -> Vec<u64> {
        self.tasks
            .iter()
            .filter_map(|task| match &task.status {
                TaskStatus::Waiting(TaskWait::HostCall {
                    call,
                    completion: None,
                    ..
                })
                | TaskStatus::Waiting(TaskWait::DeferredHostCall {
                    call,
                    completion: None,
                    ..
                })
                | TaskStatus::Waiting(TaskWait::HostTask { call, .. }) => Some(*call),
                _ => None,
            })
            .collect()
    }

    fn poll_host_calls(&mut self) -> Result<(), VmError> {
        for call in self.pending_host_calls() {
            if self.host.is_virtual_quiescence_call(call) && !self.runnable.is_empty() {
                continue;
            }
            if let Some(value) = self.host.poll_async(call)? {
                self.complete_host_call(call, value)?;
            }
        }
        Ok(())
    }

    fn complete_host_call(&mut self, call: u64, value: RuntimeValue) -> Result<(), VmError> {
        let task = self
            .tasks
            .iter()
            .position(|task| {
                matches!(
                    &task.status,
                    TaskStatus::Waiting(TaskWait::HostCall {
                        call: pending,
                        completion: None,
                        ..
                    })
                        | TaskStatus::Waiting(TaskWait::DeferredHostCall {
                            call: pending,
                            completion: None,
                            ..
                        })
                        | TaskStatus::Waiting(TaskWait::HostTask { call: pending, .. })
                        if *pending == call
                )
            })
            .ok_or_else(|| VmError::Host(format!("host completed unknown async call #{call}")))?;

        let host_task = match &self.tasks[task].status {
            TaskStatus::Waiting(TaskWait::HostTask { outcome, .. }) => Some(*outcome),
            TaskStatus::Waiting(TaskWait::HostCall { .. })
            | TaskStatus::Waiting(TaskWait::DeferredHostCall { .. }) => None,
            _ => {
                return Err(VmError::invariant(
                    "async host completion target changed status",
                ));
            }
        };
        if let Some(outcome) = host_task {
            if self.tasks[task].cancel_requested {
                self.host.cleanup(&value)?;
                self.complete_task(task, TaskCompletion::Cancelled)
            } else {
                let value = self.materialize_host_value(outcome, value)?;
                self.complete_task(task, TaskCompletion::Returned(value))
            }
        } else {
            match &mut self.tasks[task].status {
                TaskStatus::Waiting(TaskWait::HostCall { completion, .. })
                | TaskStatus::Waiting(TaskWait::DeferredHostCall { completion, .. }) => {
                    *completion = Some(value);
                }
                _ => unreachable!("host call shape was checked"),
            }
            self.wake_task(task)
        }
    }

    fn enqueue_task(&mut self, task: usize) -> Result<(), VmError> {
        let record = self
            .tasks
            .get_mut(task)
            .ok_or_else(|| VmError::invariant("cannot enqueue an invalid task"))?;
        if record.queued {
            return Ok(());
        }
        if !matches!(record.status, TaskStatus::Runnable) {
            return Ok(());
        }
        record.queued = true;
        self.runnable.push_back(task);
        Ok(())
    }

    fn park_current(&mut self, wait: TaskWait, dependencies: &[usize]) -> Result<(), VmError> {
        for dependency in dependencies {
            let waiters = &mut self
                .tasks
                .get_mut(*dependency)
                .ok_or_else(|| VmError::invariant("task waits on an invalid dependency"))?
                .waiters;
            if !waiters.contains(&self.current_task) {
                waiters.push(self.current_task);
            }
        }
        self.tasks[self.current_task].status = TaskStatus::Waiting(wait);
        Ok(())
    }

    fn park_oneshot(&mut self, id: u64, wait: TaskWait) -> Result<(), VmError> {
        let state = self
            .oneshots
            .get_mut(&id)
            .ok_or_else(|| VmError::invariant("one-shot wait references an unknown id"))?;
        if state.completion.is_some() {
            return Err(VmError::invariant(
                "one-shot wait was parked after completion",
            ));
        }
        if !state.waiter_tasks.contains(&self.current_task) {
            state.waiter_tasks.push(self.current_task);
        }
        self.park_current(wait, &[])
    }

    fn wake_task(&mut self, task: usize) -> Result<(), VmError> {
        let record = self
            .tasks
            .get_mut(task)
            .ok_or_else(|| VmError::invariant("cannot wake an invalid task"))?;
        if !matches!(record.status, TaskStatus::Waiting(_)) {
            return Ok(());
        }
        let TaskStatus::Waiting(wait) = std::mem::replace(&mut record.status, TaskStatus::Runnable)
        else {
            unreachable!("status was checked as waiting");
        };
        record.resume = Some(wait);
        self.enqueue_task(task)
    }

    fn complete_current_task(&mut self, completion: TaskCompletion) -> Result<(), VmError> {
        self.complete_task(self.current_task, completion)
    }

    fn complete_task(&mut self, task: usize, completion: TaskCompletion) -> Result<(), VmError> {
        let panicked = matches!(completion, TaskCompletion::Panicked(_));
        let parent_scope = self.tasks[task].parent_scope;
        self.tasks[task].status = TaskStatus::Complete(Some(completion));
        if let Some(scope) = parent_scope {
            let (owner, siblings) = {
                let scope = self
                    .task_scopes
                    .get(scope)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| VmError::invariant("child task has no owning scope"))?;
                (scope.owner, scope.children.clone())
            };
            if panicked {
                for sibling in siblings {
                    if sibling != task {
                        self.request_cancel(sibling)?;
                    }
                }
                self.wake_task(owner)?;
            }
        }
        let waiters = std::mem::take(&mut self.tasks[task].waiters);
        for waiter in waiters {
            self.wake_task(waiter)?;
        }
        Ok(())
    }

    fn take_task_completion(&mut self, task: usize) -> Result<Option<TaskCompletion>, VmError> {
        let record = self
            .tasks
            .get_mut(task)
            .ok_or_else(|| VmError::invariant("Join references an invalid task"))?;
        let TaskStatus::Complete(completion) = &mut record.status else {
            return Ok(None);
        };
        let completion = completion
            .take()
            .ok_or_else(|| VmError::invariant("task completion was consumed twice"))?;
        record.status = TaskStatus::Consumed;
        Ok(Some(completion))
    }

    fn request_cancel(&mut self, task: usize) -> Result<(), VmError> {
        let record = self
            .tasks
            .get_mut(task)
            .ok_or_else(|| VmError::invariant("cannot cancel an invalid task"))?;
        if matches!(
            record.status,
            TaskStatus::Complete(_) | TaskStatus::Consumed
        ) {
            return Ok(());
        }
        record.cancel_requested = true;
        let host_call = match &record.status {
            TaskStatus::Waiting(TaskWait::HostCall { call, .. })
            | TaskStatus::Waiting(TaskWait::HostTask { call, .. }) => Some(*call),
            // A cleanup that is already in flight is not cooperatively
            // cancelled by the unwind which initiated it.
            TaskStatus::Waiting(TaskWait::DeferredHostCall { .. }) => return Ok(()),
            _ => None,
        };
        if let Some(call) = host_call {
            return self.host.cancel_async(call);
        }
        self.wake_task(task)
    }

    fn finish_root_task(&mut self) -> Result<VmExecution, VmError> {
        if self
            .tasks
            .iter()
            .skip(1)
            .any(|task| !matches!(task.status, TaskStatus::Consumed))
        {
            return Err(VmError::invariant(
                "the root task completed while structured children remained live",
            ));
        }
        let completion = self
            .take_task_completion(0)?
            .ok_or_else(|| VmError::invariant("the root task has no completion"))?;
        let outcome = match completion {
            TaskCompletion::Returned(value) => VmOutcome::Returned(snapshot_value(
                &value,
                &self.heap,
                &self.callable_names,
                &self.nominal_names,
            )?),
            TaskCompletion::Panicked(panic) => VmOutcome::Panicked(panic),
            TaskCompletion::Cancelled => {
                return Err(VmError::invariant(
                    "the root task was cancelled without a propagating child panic",
                ));
            }
        };
        self.heap.collect(&[], &mut self.statistics)?;
        Ok(VmExecution {
            outcome,
            statistics: self.statistics,
        })
    }

    fn spawn_task(
        &mut self,
        function: BytecodeFunctionId,
        arguments: Vec<Value>,
        scope: usize,
    ) -> Result<usize, VmError> {
        let parent_frames = std::mem::take(&mut self.frames);
        let pushed = self.push_frame(function, arguments, None);
        let child_frames = std::mem::take(&mut self.frames);
        self.frames = parent_frames;
        pushed?;

        let task = self.tasks.len();
        self.tasks.push(TaskRecord {
            frames: child_frames,
            pending_unwind: None,
            status: TaskStatus::Runnable,
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: Some(scope),
            join_consumed: false,
            panic_observed: false,
        });
        self.task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("spawn targets a missing task scope"))?
            .children
            .push(task);
        self.enqueue_task(task)?;
        Ok(task)
    }

    fn spawn_host_task(
        &mut self,
        call: u64,
        outcome: BytecodeTypeId,
        scope: usize,
    ) -> Result<usize, VmError> {
        let task = self.tasks.len();
        self.tasks.push(TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status: TaskStatus::Waiting(TaskWait::HostTask { call, outcome }),
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: Some(scope),
            join_consumed: false,
            panic_observed: false,
        });
        self.task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("host spawn targets a missing task scope"))?
            .children
            .push(task);
        Ok(task)
    }

    fn spawn_completed_task(&mut self, value: Value, scope: usize) -> Result<usize, VmError> {
        let task = self.tasks.len();
        self.tasks.push(TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status: TaskStatus::Complete(Some(TaskCompletion::Returned(value))),
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: Some(scope),
            join_consumed: false,
            panic_observed: false,
        });
        self.task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("completed task targets a missing task scope"))?
            .children
            .push(task);
        Ok(task)
    }

    fn spawn_oneshot_task(
        &mut self,
        id: u64,
        outcome: BytecodeTypeId,
        scope: usize,
    ) -> Result<usize, VmError> {
        let completion = self
            .oneshots
            .get(&id)
            .ok_or_else(|| VmError::invariant("one-shot task references an unknown id"))?;
        match completion.completion.as_ref() {
            Some(OneShotCompletion::Cancelled) => {
                return self.spawn_cancelled_task(scope);
            }
            Some(_) => {
                return Err(VmError::invariant(
                    "one-shot task was spawned after its wait completed",
                ));
            }
            None => {}
        }
        let task = self.tasks.len();
        self.oneshots
            .get_mut(&id)
            .expect("one-shot state was checked above")
            .waiter_tasks
            .push(task);
        self.tasks.push(TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status: TaskStatus::Waiting(TaskWait::OneShotTask { id, outcome }),
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: Some(scope),
            join_consumed: false,
            panic_observed: false,
        });
        self.task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("one-shot task targets a missing task scope"))?
            .children
            .push(task);
        Ok(task)
    }

    fn spawn_cancelled_task(&mut self, scope: usize) -> Result<usize, VmError> {
        let task = self.tasks.len();
        self.tasks.push(TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status: TaskStatus::Complete(Some(TaskCompletion::Cancelled)),
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: Some(scope),
            join_consumed: false,
            panic_observed: false,
        });
        self.task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("cancelled task targets a missing task scope"))?
            .children
            .push(task);
        Ok(task)
    }

    fn active_task_scope(&self, frame: usize, source: BytecodeScopeId) -> Result<usize, VmError> {
        let id = *self.frames[frame]
            .task_scopes
            .last()
            .ok_or_else(|| VmError::invariant("spawn has no active task scope"))?;
        let scope = self
            .task_scopes
            .get(id)
            .and_then(Option::as_ref)
            .ok_or_else(|| VmError::invariant("active task scope state is missing"))?;
        if scope.source != source || scope.owner != self.current_task || scope.closed {
            return Err(VmError::invariant(
                "spawn does not target the active innermost task scope",
            ));
        }
        Ok(id)
    }

    fn current_scope_has_unobserved_panic(&self, _frame: usize) -> Result<bool, VmError> {
        for frame in self.frames.iter().rev() {
            for id in frame.task_scopes.iter().rev() {
                let scope = self
                    .task_scopes
                    .get(*id)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| VmError::invariant("active task scope state is missing"))?;
                for child in &scope.children {
                    let task = self
                        .tasks
                        .get(*child)
                        .ok_or_else(|| VmError::invariant("task scope owns an invalid child"))?;
                    if !task.panic_observed
                        && matches!(
                            task.status,
                            TaskStatus::Complete(Some(TaskCompletion::Panicked(_)))
                        )
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn apply_join_completion(
        &mut self,
        frame: usize,
        completion: TaskCompletion,
        destination: &BytecodePlace,
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    ) -> Result<(), VmError> {
        match completion {
            TaskCompletion::Returned(value) => {
                self.write_place(frame, destination, value)?;
                self.jump(frame, target);
            }
            TaskCompletion::Panicked(panic) => {
                self.begin_propagated_panic(frame, panic, unwind)?;
            }
            TaskCompletion::Cancelled => {
                self.begin_cancel(frame, unwind)?;
            }
        }
        Ok(())
    }

    fn consume_join_owner(
        &mut self,
        frame: usize,
        owner: &BytecodePlace,
    ) -> Result<RuntimeJoin, VmError> {
        let value = self.take_place(frame, owner)?;
        self.disarm_cleanup(frame, owner)?;
        let Value::Join(join) = value else {
            return Err(VmError::invariant("await Join owner has no task handle"));
        };
        let transferred = join.scope == TRANSFERRED_JOIN_SCOPE;
        let task = self
            .tasks
            .get_mut(join.task)
            .ok_or_else(|| VmError::invariant("Join references an invalid task"))?;
        if (!transferred
            && (!self.frames[frame].task_scopes.contains(&join.scope)
                || task.parent_scope != Some(join.scope)))
            || (transferred && task.parent_scope.is_some())
            || task.join_consumed
        {
            return Err(VmError::invariant(
                "Join was consumed twice or by the wrong task scope",
            ));
        }
        task.join_consumed = true;
        Ok(join)
    }

    /// Moves a direct `Join` return value out of the lexical task scope that
    /// created it.  The child remains affine, but its ownership changes from
    /// the scope ledger to the caller's return slot.  A sentinel scope keeps
    /// the runtime representation compact without manufacturing a detached
    /// task API: the caller still has to consume the handle with `await`.
    fn transfer_return_join(&mut self, frame: usize, scope: usize) -> Result<(), VmError> {
        let function = self
            .program
            .function(self.frames[frame].function)
            .ok_or_else(|| VmError::invariant("task-scope frame has an invalid function"))?;
        let return_slot = function.return_slot;
        let join = match self.frames[frame].slots.get(return_slot.index() as usize) {
            Some(SlotState::Value(Value::Join(join))) if join.scope == scope => *join,
            _ => return Ok(()),
        };
        let task = self
            .tasks
            .get_mut(join.task)
            .ok_or_else(|| VmError::invariant("returned Join references an invalid task"))?;
        if task.parent_scope != Some(scope) || task.join_consumed {
            return Err(VmError::invariant(
                "returned Join was consumed twice or by the wrong task scope",
            ));
        }
        task.parent_scope = None;
        let scope_state = self
            .task_scopes
            .get_mut(scope)
            .and_then(Option::as_mut)
            .ok_or_else(|| VmError::invariant("returned Join scope state is missing"))?;
        let Some(index) = scope_state
            .children
            .iter()
            .position(|child| *child == join.task)
        else {
            return Err(VmError::invariant(
                "returned Join is absent from its owning task scope",
            ));
        };
        scope_state.children.remove(index);
        *self.slot_mut(frame, return_slot)? = SlotState::Value(Value::Join(RuntimeJoin {
            task: join.task,
            scope: TRANSFERRED_JOIN_SCOPE,
        }));
        Ok(())
    }

    fn drain_task_scopes(
        &mut self,
        frame: usize,
        sources: &[BytecodeScopeId],
    ) -> Result<bool, VmError> {
        if sources.is_empty() {
            return Ok(true);
        }
        if self.frames[frame].task_scopes.len() < sources.len() {
            return Ok(true);
        }
        let start = self.frames[frame].task_scopes.len() - sources.len();
        for (id, source) in self.frames[frame].task_scopes[start..].iter().zip(sources) {
            let actual = self
                .task_scopes
                .get(*id)
                .and_then(Option::as_ref)
                .ok_or_else(|| VmError::invariant("active task scope state is missing"))?
                .source;
            if actual != *source {
                return Err(VmError::invariant(
                    "task-scope drain does not match the active scope suffix",
                ));
            }
        }
        for source in sources.iter().rev() {
            let id = *self.frames[frame]
                .task_scopes
                .last()
                .ok_or_else(|| VmError::invariant("task-scope drain underflow"))?;
            let (actual, children) = {
                let scope = self
                    .task_scopes
                    .get(id)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| VmError::invariant("task-scope drain state is missing"))?;
                (scope.source, scope.children.clone())
            };
            if actual != *source {
                return Err(VmError::invariant(
                    "task scopes are not drained in inner-to-outer order",
                ));
            }

            if self.pending_unwind.is_none() {
                self.transfer_return_join(frame, id)?;
            }

            let mut pending = Vec::new();
            for child in &children {
                let status = &self
                    .tasks
                    .get(*child)
                    .ok_or_else(|| VmError::invariant("task scope owns an invalid child"))?
                    .status;
                if !matches!(status, TaskStatus::Complete(_) | TaskStatus::Consumed) {
                    self.request_cancel(*child)?;
                    pending.push(*child);
                }
            }
            if !pending.is_empty() {
                self.park_current(TaskWait::Scope, &pending)?;
                return Ok(false);
            }

            let mut panics = Vec::new();
            for child in &children {
                let task = self
                    .tasks
                    .get_mut(*child)
                    .ok_or_else(|| VmError::invariant("task scope owns an invalid child"))?;
                if task.panic_observed {
                    continue;
                }
                if let TaskStatus::Complete(Some(TaskCompletion::Panicked(panic))) = &task.status {
                    panics.push(panic.clone());
                    task.panic_observed = true;
                }
            }
            if let Some(mut primary) = panics.first().cloned() {
                primary.suppressed.extend(panics.into_iter().skip(1));
                if let Some(RuntimeUnwind::Panic(owner)) = &mut self.pending_unwind {
                    owner.suppressed.push(primary);
                } else {
                    self.pending_unwind = Some(RuntimeUnwind::Panic(primary));
                }
            }

            self.teardown_scope_join_fallbacks(frame, id)?;
            self.frames[frame].task_scopes.pop();
            self.task_scopes
                .get_mut(id)
                .and_then(Option::as_mut)
                .ok_or_else(|| VmError::invariant("task-scope drain state disappeared"))?
                .closed = true;
            self.release_task_scope_if_consumed(id)?;
        }
        Ok(true)
    }

    fn teardown_scope_join_fallbacks(&mut self, frame: usize, scope: usize) -> Result<(), VmError> {
        loop {
            let candidates = self.frames[frame]
                .cleanups
                .iter()
                .enumerate()
                .filter_map(|(index, cleanup)| match cleanup {
                    RuntimeCleanup::Fallback(fallback) => Some((index, fallback.clone())),
                    RuntimeCleanup::Explicit(_) => None,
                })
                .collect::<Vec<_>>();
            let mut selected = None;
            for (index, fallback) in candidates.into_iter().rev() {
                let value = self.read_place(frame, &fallback.owner)?;
                if self.value_contains_scope_join(&value, scope, &mut Default::default())? {
                    selected = Some((index, fallback));
                    break;
                }
            }
            let Some((index, fallback)) = selected else {
                return Ok(());
            };
            self.frames[frame].cleanups.remove(index);
            self.execute_terminal_fallback(frame, fallback)?;
        }
    }

    fn value_contains_scope_join(
        &self,
        value: &Value,
        scope: usize,
        visited: &mut std::collections::BTreeSet<HeapHandle>,
    ) -> Result<bool, VmError> {
        let Value::Heap(handle) = value else {
            return Ok(matches!(value, Value::Join(join) if join.scope == scope));
        };
        if !visited.insert(*handle) {
            return Ok(false);
        }
        let object = self.heap.get(*handle)?;
        let contains = match object {
            HeapObject::String(_) | HeapObject::OptionNone => false,
            HeapObject::Tuple(values)
            | HeapObject::Closure {
                captures: values, ..
            } => self.any_value_contains_scope_join(values.iter().flatten(), scope, visited)?,
            HeapObject::Array(values) | HeapObject::Set(values) => {
                self.any_value_contains_scope_join(values.iter().flatten(), scope, visited)?
            }
            HeapObject::Map(entries) => self.any_value_contains_scope_join(
                entries
                    .iter()
                    .flat_map(|(key, value)| key.iter().chain(value.iter())),
                scope,
                visited,
            )?,
            HeapObject::Newtype { value, .. }
            | HeapObject::OptionSome(value)
            | HeapObject::ResultOk(value)
            | HeapObject::ResultErr(value)
            | HeapObject::Union { value, .. }
            | HeapObject::Ref(value) => match value {
                Some(value) => self.value_contains_scope_join(value, scope, visited)?,
                None => false,
            },
            HeapObject::Iterator {
                source, adapter, ..
            } => {
                let source_contains = source
                    .as_ref()
                    .map(|value| self.value_contains_scope_join(value, scope, visited))
                    .transpose()?
                    .unwrap_or(false);
                let adapter_contains = match adapter {
                    Some(
                        IteratorAdapter::Map { callback, .. }
                        | IteratorAdapter::Filter { callback, .. },
                    ) => self.value_contains_scope_join(callback, scope, visited)?,
                    Some(IteratorAdapter::Take { .. }) | None => false,
                };
                source_contains || adapter_contains
            }
            HeapObject::Record { fields, .. } => self.any_value_contains_scope_join(
                fields.iter().filter_map(|(_, value)| value.as_ref()),
                scope,
                visited,
            )?,
            HeapObject::Variant { payload, .. } => match payload {
                AggregatePayload::Unit => false,
                AggregatePayload::Tuple(values) => {
                    self.any_value_contains_scope_join(values.iter().flatten(), scope, visited)?
                }
                AggregatePayload::Record(fields) => self.any_value_contains_scope_join(
                    fields.iter().filter_map(|(_, value)| value.as_ref()),
                    scope,
                    visited,
                )?,
            },
            HeapObject::Range { start, end, .. } => {
                self.any_value_contains_scope_join(start.iter().chain(end.iter()), scope, visited)?
            }
        };
        Ok(contains)
    }

    fn any_value_contains_scope_join<'value>(
        &self,
        values: impl IntoIterator<Item = &'value Value>,
        scope: usize,
        visited: &mut std::collections::BTreeSet<HeapHandle>,
    ) -> Result<bool, VmError> {
        for value in values {
            if self.value_contains_scope_join(value, scope, visited)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn release_task_scope_if_consumed(&mut self, scope: usize) -> Result<(), VmError> {
        let release = {
            let state = self
                .task_scopes
                .get(scope)
                .and_then(Option::as_ref)
                .ok_or_else(|| VmError::invariant("task scope state is missing"))?;
            state.closed
                && state.children.iter().all(|child| {
                    self.tasks
                        .get(*child)
                        .is_some_and(|task| matches!(task.status, TaskStatus::Consumed))
                })
        };
        if release {
            self.task_scopes[scope] = None;
        }
        Ok(())
    }

    fn step_budget(&mut self) -> Result<(), VmError> {
        if self.statistics.steps >= self.limits.max_steps {
            return Err(VmError::ResourceLimit {
                resource: "instruction steps",
                limit: self.limits.max_steps,
            });
        }
        self.statistics.steps += 1;
        Ok(())
    }

    fn push_frame(
        &mut self,
        function_id: BytecodeFunctionId,
        arguments: Vec<Value>,
        continuation: Option<CallContinuation>,
    ) -> Result<(), VmError> {
        if self.frames.len() >= self.limits.max_stack_depth as usize {
            return Err(VmError::ResourceLimit {
                resource: "stack depth",
                limit: u64::from(self.limits.max_stack_depth),
            });
        }
        let function = self
            .program
            .function(function_id)
            .ok_or_else(|| VmError::invariant("call targets an invalid function"))?;
        if arguments.len() != function.parameters.len() {
            return Err(VmError::invariant(
                "verified call supplied the wrong frame argument count",
            ));
        }
        let explicitly_managed = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match instruction.kind {
                BytecodeInstructionKind::StorageLive(slot)
                | BytecodeInstructionKind::StorageDead(slot) => Some(slot),
                BytecodeInstructionKind::ReserveLoan(_)
                | BytecodeInstructionKind::ReleaseLoan(_)
                | BytecodeInstructionKind::Store { .. }
                | BytecodeInstructionKind::EnterTaskScope { .. }
                | BytecodeInstructionKind::RegisterDefer { .. }
                | BytecodeInstructionKind::RegisterFallback { .. }
                | BytecodeInstructionKind::RetargetCleanup { .. }
                | BytecodeInstructionKind::DisarmCleanup(_) => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut slots = function
            .slots
            .iter()
            .enumerate()
            .map(|(index, _)| {
                if explicitly_managed.contains(&crate::bytecode::BytecodeSlotId::new(index as u32))
                {
                    SlotState::Dead
                } else {
                    SlotState::Uninitialized
                }
            })
            .collect::<Vec<_>>();
        for (slot, value) in function.parameters.iter().copied().zip(arguments) {
            slots[slot.index() as usize] = SlotState::Value(value);
        }
        self.frame_traces
            .get(function_id.index() as usize)
            .filter(|trace| {
                trace.function == function_id
                    && trace.slots.len() == function.slots.len()
                    && trace
                        .slots
                        .iter()
                        .zip(&function.slots)
                        .all(|(expected, slot)| *expected == slot.ty)
            })
            .ok_or_else(|| {
                VmError::invariant("function frame does not match its verified trace descriptor")
            })?;
        self.frames.push(Frame {
            function: function_id,
            block: function.entry,
            instruction: 0,
            slots,
            loans: vec![None; function.loans.len()],
            cleanups: Vec::new(),
            task_scopes: Vec::new(),
            continuation,
        });
        self.statistics.peak_stack_depth = self
            .statistics
            .peak_stack_depth
            .max(self.frames.len() as u32);
        Ok(())
    }

    fn execute_instruction(
        &mut self,
        frame: usize,
        instruction: &BytecodeInstruction,
    ) -> Result<(), VmError> {
        match &instruction.kind {
            BytecodeInstructionKind::StorageLive(slot) => {
                let state = self.slot_mut(frame, *slot)?;
                if !matches!(state, SlotState::Dead) {
                    return Err(VmError::invariant(
                        "StorageLive reached an already-live slot",
                    ));
                }
                *state = SlotState::Uninitialized;
            }
            BytecodeInstructionKind::StorageDead(slot) => {
                let state = self.slot_mut(frame, *slot)?;
                if matches!(state, SlotState::Dead) {
                    return Err(VmError::invariant(
                        "StorageDead reached an already-dead slot",
                    ));
                }
                *state = SlotState::Dead;
            }
            BytecodeInstructionKind::ReserveLoan(loan) => {
                self.reserve_loan(frame, *loan)?;
            }
            BytecodeInstructionKind::ReleaseLoan(loan) => {
                self.release_loan(frame, *loan)?;
            }
            BytecodeInstructionKind::Store { destination, value } => {
                let value = self.evaluate_rvalue(frame, value)?;
                self.write_place(frame, destination, value)?;
            }
            BytecodeInstructionKind::EnterTaskScope { scope } => {
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    let unwind = self
                        .program
                        .function(self.frames[frame].function)
                        .ok_or_else(|| VmError::invariant("task scope frame is invalid"))?
                        .unwind;
                    self.begin_cancel(frame, unwind)?;
                } else {
                    let id = self.task_scopes.len();
                    self.task_scopes.push(Some(RuntimeTaskScope {
                        source: *scope,
                        owner: self.current_task,
                        children: Vec::new(),
                        closed: false,
                    }));
                    self.frames[frame].task_scopes.push(id);
                }
            }
            BytecodeInstructionKind::RegisterDefer {
                scope,
                action,
                guard,
            } => {
                let span = self.resolve_span(frame, instruction.span)?;
                let marker = self.temporary_roots.len();
                let deferred = self.capture_defer(frame, *scope, span, action, guard.as_ref());
                let registration = match deferred {
                    Ok(deferred) => {
                        let replacement = match deferred.guard.as_ref() {
                            Some(guard) => self.replace_fallback_with_explicit(frame, guard),
                            None => Ok(()),
                        };
                        if replacement.is_ok() {
                            self.frames[frame]
                                .cleanups
                                .push(RuntimeCleanup::Explicit(deferred));
                        }
                        replacement
                    }
                    Err(error) => Err(error),
                };
                self.temporary_roots.truncate(marker);
                registration?;
            }
            BytecodeInstructionKind::RegisterFallback { scope, owner } => {
                self.register_fallback(frame, *scope, owner)?;
            }
            BytecodeInstructionKind::RetargetCleanup { from, to } => {
                self.retarget_cleanup(frame, from, to)?;
            }
            BytecodeInstructionKind::DisarmCleanup(place) => {
                self.disarm_cleanup(frame, place)?;
            }
        }
        Ok(())
    }

    fn capture_defer(
        &mut self,
        frame: usize,
        scope: BytecodeScopeId,
        span: BytecodeSpan,
        action: &BytecodeOperation,
        guard: Option<&BytecodePlace>,
    ) -> Result<RuntimeDefer, VmError> {
        let mut guard_uses = 0_usize;
        let operation = match &action.kind {
            BytecodeOperationKind::Call {
                callee, arguments, ..
            } => {
                let callee = self.capture_deferred_value(frame, callee, guard, &mut guard_uses)?;
                let mut captured = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    if argument.mode != BytecodeParameterMode::Value {
                        return Err(VmError::invariant(
                            "defer attempted to retain a borrowed call argument",
                        ));
                    }
                    captured.push(DeferredCallArgument {
                        target: argument.target,
                        value: self.capture_deferred_value(
                            frame,
                            &argument.value,
                            guard,
                            &mut guard_uses,
                        )?,
                    });
                }
                DeferredOperation::Call {
                    callee,
                    arguments: captured,
                }
            }
            BytecodeOperationKind::Assert {
                condition,
                condition_repr,
                message_parts,
            } => {
                let condition =
                    self.capture_deferred_value(frame, condition, guard, &mut guard_uses)?;
                let mut captured = Vec::with_capacity(message_parts.len());
                for part in message_parts {
                    captured.push((
                        self.capture_deferred_value(frame, &part.value, guard, &mut guard_uses)?,
                        part.spread,
                    ));
                }
                DeferredOperation::Assert {
                    condition,
                    condition_repr: condition_repr.clone(),
                    message_parts: captured,
                }
            }
            BytecodeOperationKind::BootstrapHostCall {
                function,
                arguments,
            } => {
                let mut captured = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    captured.push(self.capture_deferred_value(
                        frame,
                        argument,
                        guard,
                        &mut guard_uses,
                    )?);
                }
                DeferredOperation::BootstrapHostCall {
                    function: *function,
                    arguments: captured,
                }
            }
            _ => {
                return Err(VmError::invariant(
                    "defer action is not a verified Unit invocation",
                ));
            }
        };
        if guard_uses != usize::from(guard.is_some()) {
            return Err(VmError::invariant(
                "defer guard does not identify exactly one invocation operand",
            ));
        }
        if let Some(guard) = guard
            && self.frames[frame].cleanups.iter().any(|cleanup| {
                matches!(cleanup, RuntimeCleanup::Explicit(_)) && cleanup.guard() == Some(guard)
            })
        {
            return Err(VmError::invariant(
                "two active defer entries guard the same place",
            ));
        }
        Ok(RuntimeDefer {
            scope,
            span,
            operation,
            guard: guard.cloned(),
            async_cleanup: matches!(
                &action.kind,
                BytecodeOperationKind::Call { signature, .. }
                    if self
                        .program
                        .ty(*signature)
                        .is_some_and(|ty| matches!(&ty.kind, BytecodeTypeKind::Function(function) if function.is_async))
            ),
        })
    }

    fn capture_deferred_value(
        &mut self,
        frame: usize,
        operand: &BytecodeOperand,
        guard: Option<&BytecodePlace>,
        guard_uses: &mut usize,
    ) -> Result<DeferredValue, VmError> {
        if let (Some(guard), BytecodeOperandKind::Move(place)) = (guard, &operand.kind)
            && place == guard
        {
            *guard_uses += 1;
            return Ok(DeferredValue::Guard);
        }
        let value = self.evaluate_operand(frame, operand)?;
        self.temporary_roots.push(value.clone());
        Ok(DeferredValue::Captured(value))
    }

    fn retarget_cleanup(
        &mut self,
        frame: usize,
        from: &BytecodePlace,
        to: &BytecodePlace,
    ) -> Result<(), VmError> {
        let matches = self.frames[frame]
            .cleanups
            .iter()
            .enumerate()
            .filter_map(|(index, cleanup)| (cleanup.guard() == Some(from)).then_some(index))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(()),
            [index] => {
                *self.frames[frame].cleanups[*index]
                    .guard_mut()
                    .expect("matched cleanup entries have a guard") = to.clone();
                Ok(())
            }
            _ => Err(VmError::invariant(
                "more than one defer guard matched a retarget source",
            )),
        }
    }

    fn disarm_cleanup(&mut self, frame: usize, place: &BytecodePlace) -> Result<(), VmError> {
        let matches = self.frames[frame]
            .cleanups
            .iter()
            .enumerate()
            .filter_map(|(index, cleanup)| (cleanup.guard() == Some(place)).then_some(index))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(()),
            [index] => {
                self.frames[frame].cleanups.remove(*index);
                Ok(())
            }
            _ => Err(VmError::invariant(
                "more than one defer guard matched a disarm place",
            )),
        }
    }

    fn register_fallback(
        &mut self,
        frame: usize,
        scope: BytecodeScopeId,
        owner: &BytecodePlace,
    ) -> Result<(), VmError> {
        let mut contained = Vec::new();
        let mut already_guarded = false;
        for (index, cleanup) in self.frames[frame].cleanups.iter().enumerate() {
            match cleanup {
                RuntimeCleanup::Explicit(deferred)
                    if deferred.guard.as_ref().is_some_and(|guard| {
                        place_contains(guard, owner) || place_contains(owner, guard)
                    }) =>
                {
                    return Err(VmError::invariant(
                        "terminal fallback overlaps an explicit cleanup guard",
                    ));
                }
                RuntimeCleanup::Fallback(fallback) if place_contains(&fallback.owner, owner) => {
                    already_guarded = true;
                }
                RuntimeCleanup::Fallback(fallback) if place_contains(owner, &fallback.owner) => {
                    contained.push(index);
                }
                RuntimeCleanup::Explicit(_) | RuntimeCleanup::Fallback(_) => {}
            }
        }
        if already_guarded {
            return Ok(());
        }
        for index in contained.into_iter().rev() {
            self.frames[frame].cleanups.remove(index);
        }
        self.frames[frame]
            .cleanups
            .push(RuntimeCleanup::Fallback(RuntimeFallback {
                scope,
                owner: owner.clone(),
            }));
        Ok(())
    }

    fn replace_fallback_with_explicit(
        &mut self,
        frame: usize,
        guard: &BytecodePlace,
    ) -> Result<(), VmError> {
        let mut replaced = Vec::new();
        for (index, cleanup) in self.frames[frame].cleanups.iter().enumerate() {
            let RuntimeCleanup::Fallback(fallback) = cleanup else {
                continue;
            };
            if place_contains(guard, &fallback.owner) {
                replaced.push(index);
            } else if place_contains(&fallback.owner, guard) {
                return Err(VmError::invariant(
                    "explicit cleanup guard is only part of an active terminal fallback",
                ));
            }
        }
        if replaced.len() > 1 {
            return Err(VmError::invariant(
                "terminal explicit cleanup would replace more than one fallback",
            ));
        }
        for index in replaced.into_iter().rev() {
            self.frames[frame].cleanups.remove(index);
        }
        Ok(())
    }

    fn execute_terminal_fallback(
        &mut self,
        frame: usize,
        fallback: RuntimeFallback,
    ) -> Result<(), VmError> {
        let marker = self.temporary_roots.len();
        let result = self.execute_terminal_fallback_rooted(frame, fallback);
        self.temporary_roots.truncate(marker);
        result
    }

    fn execute_terminal_fallback_rooted(
        &mut self,
        frame: usize,
        fallback: RuntimeFallback,
    ) -> Result<(), VmError> {
        let owner = self.take_place(frame, &fallback.owner)?;
        self.retain_temporary(&owner);
        let mut pending = vec![(
            RuntimeType {
                ty: fallback.owner.ty,
                substitutions: Vec::new(),
            },
            owner,
        )];
        while let Some((ty, value)) = pending.pop() {
            self.step_budget()?;
            let ty = self.resolve_runtime_type(ty)?;
            let kind = self
                .program
                .ty(ty.ty)
                .ok_or_else(|| VmError::invariant("fallback references an unknown type"))?
                .kind
                .clone();
            match kind {
                BytecodeTypeKind::Scalar(_)
                | BytecodeTypeKind::Function(_)
                | BytecodeTypeKind::OpaqueResult { .. } => {}
                BytecodeTypeKind::GenericParameter(_) => {
                    return Err(VmError::invariant(
                        "terminal fallback retained an unresolved generic type",
                    ));
                }
                BytecodeTypeKind::Tuple(items) => {
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal tuple fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    let HeapObject::Tuple(values) = &mut object else {
                        return Err(VmError::invariant(
                            "terminal tuple fallback found a different heap object",
                        ));
                    };
                    if values.len() != items.len() {
                        return Err(VmError::invariant(
                            "terminal tuple fallback found the wrong arity",
                        ));
                    }
                    for (item, value) in items.into_iter().zip(values.iter_mut()) {
                        if let Some(value) = value.take() {
                            self.queue_fallback_value(&mut pending, ty.child(item), value);
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Option(item) => {
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal Option fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    match &mut object {
                        HeapObject::OptionNone => {}
                        HeapObject::OptionSome(value) => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(&mut pending, ty.child(item), value);
                            }
                        }
                        _ => {
                            return Err(VmError::invariant(
                                "terminal Option fallback found a different heap object",
                            ));
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Result { success, error } => {
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal Result fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    match &mut object {
                        HeapObject::ResultOk(value) => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(&mut pending, ty.child(success), value);
                            }
                        }
                        HeapObject::ResultErr(value) => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(&mut pending, ty.child(error), value);
                            }
                        }
                        _ => {
                            return Err(VmError::invariant(
                                "terminal Result fallback found a different heap object",
                            ));
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Union(_) => {
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal union fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    let HeapObject::Union { member, value } = &mut object else {
                        return Err(VmError::invariant(
                            "terminal union fallback found a different heap object",
                        ));
                    };
                    if let Some(value) = value.take() {
                        self.queue_fallback_value(&mut pending, ty.child(*member), value);
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Intrinsic {
                    constructor,
                    arguments,
                } => match constructor {
                    BytecodeIntrinsicType::Join => {
                        self.teardown_join(&mut pending, &ty, &arguments, value)?;
                    }
                    BytecodeIntrinsicType::Array | BytecodeIntrinsicType::Set => {
                        let [item] = arguments.as_slice() else {
                            return Err(VmError::invariant(
                                "terminal collection fallback has the wrong type arity",
                            ));
                        };
                        let Value::Heap(handle) = value else {
                            return Err(VmError::invariant(
                                "terminal collection fallback found a non-managed value",
                            ));
                        };
                        let mut object = self.heap.get(handle)?.clone();
                        let values = match &mut object {
                            HeapObject::Array(values)
                                if constructor == BytecodeIntrinsicType::Array =>
                            {
                                values
                            }
                            HeapObject::Set(values)
                                if constructor == BytecodeIntrinsicType::Set =>
                            {
                                values
                            }
                            _ => {
                                return Err(VmError::invariant(
                                    "terminal collection fallback found a different heap object",
                                ));
                            }
                        };
                        for value in values {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(&mut pending, ty.child(*item), value);
                            }
                        }
                        self.replace_fallback_object(handle, object)?;
                    }
                    BytecodeIntrinsicType::Map => {
                        let [key, item] = arguments.as_slice() else {
                            return Err(VmError::invariant(
                                "terminal Map fallback has the wrong type arity",
                            ));
                        };
                        let Value::Heap(handle) = value else {
                            return Err(VmError::invariant(
                                "terminal Map fallback found a non-managed value",
                            ));
                        };
                        let mut object = self.heap.get(handle)?.clone();
                        let HeapObject::Map(entries) = &mut object else {
                            return Err(VmError::invariant(
                                "terminal Map fallback found a different heap object",
                            ));
                        };
                        for (entry_key, entry_value) in entries {
                            if let Some(entry_key) = entry_key.take() {
                                self.queue_fallback_value(&mut pending, ty.child(*key), entry_key);
                            }
                            if let Some(entry_value) = entry_value.take() {
                                self.queue_fallback_value(
                                    &mut pending,
                                    ty.child(*item),
                                    entry_value,
                                );
                            }
                        }
                        self.replace_fallback_object(handle, object)?;
                    }
                    BytecodeIntrinsicType::Range => {
                        let [item] = arguments.as_slice() else {
                            return Err(VmError::invariant(
                                "terminal Range fallback has the wrong type arity",
                            ));
                        };
                        let Value::Heap(handle) = value else {
                            return Err(VmError::invariant(
                                "terminal Range fallback found a non-managed value",
                            ));
                        };
                        let mut object = self.heap.get(handle)?.clone();
                        let HeapObject::Range { start, end, .. } = &mut object else {
                            return Err(VmError::invariant(
                                "terminal Range fallback found a different heap object",
                            ));
                        };
                        if let Some(start) = start.take() {
                            self.queue_fallback_value(&mut pending, ty.child(*item), start);
                        }
                        if let Some(end) = end.take() {
                            self.queue_fallback_value(&mut pending, ty.child(*item), end);
                        }
                        self.replace_fallback_object(handle, object)?;
                    }
                    BytecodeIntrinsicType::Ref
                    | BytecodeIntrinsicType::Pointer
                    | BytecodeIntrinsicType::Waiter
                    | BytecodeIntrinsicType::Completer
                    | BytecodeIntrinsicType::AlreadyCompleted
                    | BytecodeIntrinsicType::Command
                    | BytecodeIntrinsicType::Pipeline
                    | BytecodeIntrinsicType::Bytes
                    | BytecodeIntrinsicType::BytesBuilder
                    | BytecodeIntrinsicType::BytesError
                    | BytecodeIntrinsicType::FormatBuilder
                    | BytecodeIntrinsicType::FormatError
                    | BytecodeIntrinsicType::TextError
                    | BytecodeIntrinsicType::CollectionError
                    | BytecodeIntrinsicType::Path
                    | BytecodeIntrinsicType::PathError
                    | BytecodeIntrinsicType::File
                    | BytecodeIntrinsicType::Directory
                    | BytecodeIntrinsicType::Metadata
                    | BytecodeIntrinsicType::OpenMode
                    | BytecodeIntrinsicType::FsError
                    | BytecodeIntrinsicType::MathError
                    | BytecodeIntrinsicType::FloatTolerance
                    | BytecodeIntrinsicType::FloatToleranceError
                    | BytecodeIntrinsicType::TextDiff
                    | BytecodeIntrinsicType::TempDirectory
                    | BytecodeIntrinsicType::TempError
                    | BytecodeIntrinsicType::Generator
                    | BytecodeIntrinsicType::GenerationId
                    | BytecodeIntrinsicType::GenerationError
                    | BytecodeIntrinsicType::Reader
                    | BytecodeIntrinsicType::Writer
                    | BytecodeIntrinsicType::IoLimits
                    | BytecodeIntrinsicType::IoError
                    | BytecodeIntrinsicType::ConsoleError
                    | BytecodeIntrinsicType::ExitStatus
                    | BytecodeIntrinsicType::ProcessOutput
                    | BytecodeIntrinsicType::ProcessError
                    | BytecodeIntrinsicType::ProcessExitError
                    | BytecodeIntrinsicType::Utf8Error
                    | BytecodeIntrinsicType::NumericConversionError
                    | BytecodeIntrinsicType::Duration
                    | BytecodeIntrinsicType::Instant
                    | BytecodeIntrinsicType::DurationError
                    | BytecodeIntrinsicType::ClockError
                    | BytecodeIntrinsicType::EnvSnapshot
                    | BytecodeIntrinsicType::EnvName
                    | BytecodeIntrinsicType::EnvValue
                    | BytecodeIntrinsicType::EnvError
                    | BytecodeIntrinsicType::VirtualTime
                    | BytecodeIntrinsicType::JsonLimits
                    | BytecodeIntrinsicType::JsonDecodeOptions
                    | BytecodeIntrinsicType::JsonEncodeOptions
                    | BytecodeIntrinsicType::JsonDuplicatePolicy
                    | BytecodeIntrinsicType::JsonUnknownFieldPolicy
                    | BytecodeIntrinsicType::JsonNumberPolicy
                    | BytecodeIntrinsicType::JsonValue
                    | BytecodeIntrinsicType::JsonValueView
                    | BytecodeIntrinsicType::JsonRaw
                    | BytecodeIntrinsicType::JsonNumber
                    | BytecodeIntrinsicType::JsonReader
                    | BytecodeIntrinsicType::JsonEvent
                    | BytecodeIntrinsicType::JsonWriter
                    | BytecodeIntrinsicType::JsonError => {}
                    BytecodeIntrinsicType::ProcessHandle => {
                        let Value::Host(value) = value else {
                            return Err(VmError::invariant(
                                "ProcessHandle fallback found a non-host value",
                            ));
                        };
                        self.host.cleanup(&value)?;
                    }
                    BytecodeIntrinsicType::Timer => {
                        let Value::Host(value) = value else {
                            return Err(VmError::invariant(
                                "Timer fallback found a non-host value",
                            ));
                        };
                        self.host.cleanup(&value)?;
                    }
                },
                BytecodeTypeKind::Nominal {
                    nominal, arguments, ..
                } => {
                    let nominal = nominal.ok_or_else(|| {
                        VmError::invariant(
                            "terminal fallback references an unresolved nominal type",
                        )
                    })?;
                    let metadata = self
                        .program
                        .nominals
                        .get(nominal.index() as usize)
                        .ok_or_else(|| {
                            VmError::invariant(
                                "terminal fallback references unknown nominal metadata",
                            )
                        })?
                        .clone();
                    let substitutions = arguments
                        .into_iter()
                        .map(|argument| ty.child(argument))
                        .collect::<Vec<_>>();
                    let child = |child| RuntimeType {
                        ty: child,
                        substitutions: substitutions.clone(),
                    };
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal nominal fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    match (&metadata.shape, &mut object) {
                        (
                            BytecodeNominalShape::Newtype { underlying },
                            HeapObject::Newtype {
                                nominal: actual,
                                value,
                            },
                        ) if *actual == nominal => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(&mut pending, child(*underlying), value);
                            }
                        }
                        (
                            BytecodeNominalShape::Record { fields: schema },
                            HeapObject::Record {
                                nominal: actual,
                                fields,
                            },
                        ) if *actual == nominal => {
                            for (member, value) in fields {
                                let field = schema
                                    .iter()
                                    .find(|field| field.member == *member)
                                    .ok_or_else(|| {
                                        VmError::invariant(
                                            "terminal record fallback found an unknown field",
                                        )
                                    })?;
                                if let Some(value) = value.take() {
                                    self.queue_fallback_value(&mut pending, child(field.ty), value);
                                }
                            }
                        }
                        (
                            BytecodeNominalShape::Enum { variants },
                            HeapObject::Variant { variant, payload },
                        ) => {
                            let variant = variants
                                .iter()
                                .find(|candidate| candidate.member == *variant)
                                .ok_or_else(|| {
                                    VmError::invariant(
                                        "terminal enum fallback found an unknown variant",
                                    )
                                })?;
                            match (&variant.payload, payload) {
                                (BytecodeVariantPayload::Unit, AggregatePayload::Unit) => {}
                                (
                                    BytecodeVariantPayload::Tuple(schema),
                                    AggregatePayload::Tuple(values),
                                ) if schema.len() == values.len() => {
                                    for (item, value) in schema.iter().zip(values) {
                                        if let Some(value) = value.take() {
                                            self.queue_fallback_value(
                                                &mut pending,
                                                child(*item),
                                                value,
                                            );
                                        }
                                    }
                                }
                                (
                                    BytecodeVariantPayload::Record(schema),
                                    AggregatePayload::Record(fields),
                                ) => {
                                    for (member, value) in fields {
                                        let field = schema
                                            .iter()
                                            .find(|field| field.member == *member)
                                            .ok_or_else(|| {
                                                VmError::invariant(
                                                    "terminal variant fallback found an unknown field",
                                                )
                                            })?;
                                        if let Some(value) = value.take() {
                                            self.queue_fallback_value(
                                                &mut pending,
                                                child(field.ty),
                                                value,
                                            );
                                        }
                                    }
                                }
                                _ => {
                                    return Err(VmError::invariant(
                                        "terminal enum fallback found the wrong payload shape",
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Err(VmError::invariant(
                                "terminal nominal fallback found a different heap object",
                            ));
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Generated { .. } => {
                    let captures = self
                        .program
                        .callables
                        .iter()
                        .find_map(|callable| {
                            callable
                                .closure
                                .as_ref()
                                .filter(|closure| closure.environment == ty.ty)
                                .map(|closure| closure.captures.clone())
                        })
                        .ok_or_else(|| {
                            VmError::invariant("terminal generated fallback has no closure schema")
                        })?;
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal closure fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    let HeapObject::Closure {
                        captures: values, ..
                    } = &mut object
                    else {
                        return Err(VmError::invariant(
                            "terminal closure fallback found a different heap object",
                        ));
                    };
                    if captures.len() != values.len() {
                        return Err(VmError::invariant(
                            "terminal closure fallback found the wrong capture arity",
                        ));
                    }
                    for (capture, value) in captures.into_iter().zip(values) {
                        if let Some(value) = value.take() {
                            self.queue_fallback_value(&mut pending, ty.child(capture), value);
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
                BytecodeTypeKind::Cursor { mode, collection } => {
                    if mode != BytecodeCursorMode::Own {
                        continue;
                    }
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "terminal cursor fallback found a non-managed value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    let HeapObject::Iterator {
                        mode: actual,
                        source,
                        ..
                    } = &mut object
                    else {
                        return Err(VmError::invariant(
                            "terminal cursor fallback found a different heap object",
                        ));
                    };
                    if *actual != BytecodeCursorMode::Own {
                        return Err(VmError::invariant(
                            "terminal cursor fallback found a ref iterator",
                        ));
                    }
                    if let Some(source) = source.take() {
                        self.queue_fallback_value(&mut pending, ty.child(collection), source);
                    }
                    self.replace_fallback_object(handle, object)?;
                }
            }
        }
        Ok(())
    }

    fn queue_fallback_value(
        &mut self,
        pending: &mut Vec<(RuntimeType, Value)>,
        ty: RuntimeType,
        value: Value,
    ) {
        self.retain_temporary(&value);
        pending.push((ty, value));
    }

    fn teardown_join(
        &mut self,
        pending: &mut Vec<(RuntimeType, Value)>,
        ty: &RuntimeType,
        arguments: &[BytecodeTypeId],
        value: Value,
    ) -> Result<(), VmError> {
        let [success, error] = arguments else {
            return Err(VmError::invariant(
                "terminal Join fallback has the wrong type arity",
            ));
        };
        let Value::Join(join) = value else {
            return Err(VmError::invariant(
                "terminal Join fallback found no task handle",
            ));
        };
        let task = self
            .tasks
            .get_mut(join.task)
            .ok_or_else(|| VmError::invariant("terminal Join references an invalid task"))?;
        if task.parent_scope != Some(join.scope) || task.join_consumed {
            return Err(VmError::invariant(
                "terminal Join was consumed twice or by the wrong scope",
            ));
        }
        task.join_consumed = true;
        let panic_observed = task.panic_observed;
        let completion = self
            .take_task_completion(join.task)?
            .ok_or_else(|| VmError::invariant("terminal Join child has not completed cleanup"))?;
        match completion {
            TaskCompletion::Returned(value) => {
                if matches!(
                    self.program.ty(*error).map(|ty| &ty.kind),
                    Some(BytecodeTypeKind::Scalar(BytecodeScalarType::Never))
                ) {
                    self.queue_fallback_value(pending, ty.child(*success), value);
                } else {
                    let Value::Heap(handle) = value else {
                        return Err(VmError::invariant(
                            "fallible Join completed with a non-Result value",
                        ));
                    };
                    let mut object = self.heap.get(handle)?.clone();
                    match &mut object {
                        HeapObject::ResultOk(value) => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(pending, ty.child(*success), value);
                            }
                        }
                        HeapObject::ResultErr(value) => {
                            if let Some(value) = value.take() {
                                self.queue_fallback_value(pending, ty.child(*error), value);
                            }
                        }
                        _ => {
                            return Err(VmError::invariant(
                                "fallible Join completed with a different heap object",
                            ));
                        }
                    }
                    self.replace_fallback_object(handle, object)?;
                }
            }
            TaskCompletion::Panicked(panic) if !panic_observed => {
                if let Some(RuntimeUnwind::Panic(primary)) = &mut self.pending_unwind {
                    primary.suppressed.push(panic);
                } else {
                    self.pending_unwind = Some(RuntimeUnwind::Panic(panic));
                }
            }
            TaskCompletion::Panicked(_) | TaskCompletion::Cancelled => {}
        }
        if self
            .task_scopes
            .get(join.scope)
            .is_some_and(Option::is_some)
        {
            self.release_task_scope_if_consumed(join.scope)?;
        }
        Ok(())
    }

    fn resolve_runtime_type(&self, mut ty: RuntimeType) -> Result<RuntimeType, VmError> {
        for _ in 0..=self.program.types.len() {
            let kind = &self
                .program
                .ty(ty.ty)
                .ok_or_else(|| VmError::invariant("fallback references an unknown type"))?
                .kind;
            let BytecodeTypeKind::GenericParameter(position) = kind else {
                return Ok(ty);
            };
            ty = ty
                .substitutions
                .get(*position as usize)
                .cloned()
                .ok_or_else(|| {
                    VmError::invariant(
                        "terminal fallback cannot resolve a generic nominal component",
                    )
                })?;
        }
        Err(VmError::invariant(
            "terminal fallback type substitution contains a cycle",
        ))
    }

    fn replace_fallback_object(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
    ) -> Result<(), VmError> {
        self.replace_object(handle, object, &[])
    }

    fn reserve_loan(&mut self, frame: usize, id: BytecodeLoanId) -> Result<(), VmError> {
        let loan = {
            let function = self
                .program
                .function(self.frames[frame].function)
                .ok_or_else(|| VmError::invariant("loan frame has an invalid function"))?;
            function
                .loans
                .get(id.index() as usize)
                .cloned()
                .ok_or_else(|| VmError::invariant("ReserveLoan references an invalid loan"))?
        };
        if loan.mode == BytecodeParameterMode::Value {
            return Err(VmError::invariant(
                "ReserveLoan uses the owning value parameter mode",
            ));
        }
        let source_mode =
            if let Some(mode) = self.validate_source_regions(frame, &loan.place, true)? {
                Some(mode)
            } else {
                self.root_loan(frame, loan.place.slot)?
                    .map(|source| source.mode)
            };
        if let Some(source) = source_mode {
            let compatible = match loan.mode {
                BytecodeParameterMode::Value => false,
                BytecodeParameterMode::Ref => true,
                BytecodeParameterMode::Mut => matches!(
                    source,
                    BytecodeParameterMode::Mut | BytecodeParameterMode::Var
                ),
                BytecodeParameterMode::Var => {
                    source == BytecodeParameterMode::Var
                        || source == BytecodeParameterMode::Mut
                            && loan.place.is_structurally_replaceable()
                }
            };
            if !compatible {
                return Err(VmError::invariant(
                    "a reborrow requests stronger permissions than its source loan",
                ));
            }
        }
        let path =
            self.validate_place(frame, &loan.place, false)
                .map_err(|failure| match failure {
                    PlaceFailure::Vm(error) => error,
                    PlaceFailure::Panic(_, _) => VmError::invariant(
                        "a fixed-region loan unexpectedly required a runtime place check",
                    ),
                })?;
        let reservations = self
            .frames
            .get(frame)
            .ok_or_else(|| VmError::invariant("ReserveLoan escaped the current frame"))?;
        if reservations
            .loans
            .get(id.index() as usize)
            .is_none_or(Option::is_some)
        {
            return Err(VmError::invariant(
                "ReserveLoan reuses an active or invalid reservation",
            ));
        }
        let function = self
            .program
            .function(reservations.function)
            .ok_or_else(|| VmError::invariant("loan frame has an invalid function"))?;
        let mut source_chain = Vec::new();
        let mut source = loan.place.source_loan;
        while let Some(parent) = source {
            if source_chain.contains(&parent) {
                return Err(VmError::invariant(
                    "loan source region chain contains a cycle",
                ));
            }
            source_chain.push(parent);
            source = function
                .loans
                .get(parent.index() as usize)
                .ok_or_else(|| VmError::invariant("loan source region is invalid"))?
                .place
                .source_loan;
        }
        if reservations
            .loans
            .iter()
            .enumerate()
            .any(|(index, active)| {
                let identity = BytecodeLoanId::new(index as u32);
                active.as_ref().is_some_and(|active| {
                    !source_chain.contains(&identity)
                        && paths_overlap(&active.path, &path)
                        && !(active.mode == BytecodeParameterMode::Ref
                            && loan.mode == BytecodeParameterMode::Ref)
                })
            })
        {
            return Err(VmError::invariant(
                "ReserveLoan overlaps an incompatible active reservation",
            ));
        }
        self.frames[frame].loans[id.index() as usize] = Some(RuntimeReservation {
            mode: loan.mode,
            path,
        });
        Ok(())
    }

    fn release_loan(&mut self, frame: usize, id: BytecodeLoanId) -> Result<(), VmError> {
        let dependent = {
            let frame = self
                .frames
                .get(frame)
                .ok_or_else(|| VmError::invariant("ReleaseLoan escaped the current frame"))?;
            let function = self
                .program
                .function(frame.function)
                .ok_or_else(|| VmError::invariant("loan frame has an invalid function"))?;
            let mut dependent = None;
            for (index, reservation) in frame.loans.iter().enumerate() {
                if reservation.is_none() || index == id.index() as usize {
                    continue;
                }
                let candidate = BytecodeLoanId::new(index as u32);
                let mut parent = function
                    .loans
                    .get(index)
                    .ok_or_else(|| VmError::invariant("active loan has no metadata"))?
                    .place
                    .source_loan;
                let mut remaining = function.loans.len();
                while let Some(source) = parent {
                    if source == id {
                        dependent = Some(candidate);
                        break;
                    }
                    if remaining == 0 {
                        return Err(VmError::invariant(
                            "loan source region chain contains a cycle",
                        ));
                    }
                    remaining -= 1;
                    parent = function
                        .loans
                        .get(source.index() as usize)
                        .ok_or_else(|| VmError::invariant("loan source region is invalid"))?
                        .place
                        .source_loan;
                }
                if dependent.is_some() {
                    break;
                }
            }
            dependent
        };
        if let Some(dependent) = dependent {
            return Err(VmError::invariant(format!(
                "ReleaseLoan closes source loan#{} while dependent loan#{} remains active",
                id.index(),
                dependent.index()
            )));
        }
        let reservation = self
            .frames
            .get_mut(frame)
            .and_then(|frame| frame.loans.get_mut(id.index() as usize))
            .ok_or_else(|| VmError::invariant("ReleaseLoan references an invalid loan"))?;
        if reservation.take().is_none() {
            return Err(VmError::invariant(
                "ReleaseLoan references an inactive reservation",
            ));
        }
        Ok(())
    }

    fn execute_terminator(
        &mut self,
        frame: usize,
        terminator: &BytecodeTerminator,
    ) -> Result<Option<TaskCompletion>, VmError> {
        match &terminator.kind {
            BytecodeTerminatorKind::Goto { target } => self.jump(frame, *target),
            BytecodeTerminatorKind::BranchBool {
                condition,
                if_true,
                if_false,
            } => {
                let condition = self.evaluate_operand(frame, condition)?;
                let Value::Bool(condition) = condition else {
                    return Err(VmError::invariant("verified boolean branch is not Bool"));
                };
                self.jump(frame, if condition { *if_true } else { *if_false });
            }
            BytecodeTerminatorKind::BranchTag {
                value,
                cases,
                otherwise,
            } => {
                let value = self.evaluate_operand(frame, value)?;
                let tag = self.value_tag(&value)?;
                let target = cases
                    .iter()
                    .find_map(|(candidate, target)| (*candidate == tag).then_some(*target))
                    .unwrap_or(*otherwise);
                self.jump(frame, target);
            }
            BytecodeTerminatorKind::Invoke {
                operation,
                destination,
                target,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                match self.evaluate_operation(frame, operation, span)? {
                    OperationResult::Value(value) => {
                        if let Some(destination) = destination {
                            self.write_place(frame, destination, value)?;
                        }
                        let target = target.ok_or_else(|| {
                            VmError::invariant("normal operation has no normal target")
                        })?;
                        self.jump(frame, target);
                    }
                    OperationResult::Call {
                        function,
                        arguments,
                    } => {
                        let continuation = CallContinuation {
                            destination: destination.clone(),
                            target: *target,
                            unwind: *unwind,
                            call_span: span,
                            test_boundary: None,
                            virtual_time: None,
                        };
                        self.push_frame(function, arguments, Some(continuation))?;
                    }
                    OperationResult::TestBoundaryCall {
                        function,
                        arguments,
                        boundary,
                    } => {
                        let continuation = CallContinuation {
                            destination: destination.clone(),
                            target: *target,
                            unwind: *unwind,
                            call_span: span,
                            test_boundary: Some(boundary),
                            virtual_time: None,
                        };
                        self.push_frame(function, arguments, Some(continuation))?;
                    }
                    OperationResult::HostAsync { .. } => {
                        return Err(VmError::invariant(
                            "an async host call appeared in a synchronous invocation",
                        ));
                    }
                    OperationResult::OneShotWait { .. } => {
                        return Err(VmError::invariant(
                            "a one-shot wait appeared in a synchronous invocation",
                        ));
                    }
                    OperationResult::VirtualTimeBoundaryCall { .. } => {
                        return Err(VmError::invariant(
                            "an async virtual-time boundary appeared in a synchronous invocation",
                        ));
                    }
                    OperationResult::Panic(code, message) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                }
            }
            BytecodeTerminatorKind::Await {
                awaitable,
                destination,
                target,
                unwind,
            } => {
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    self.begin_cancel(frame, *unwind)?;
                    return Ok(None);
                }
                let span = self.resolve_span(frame, terminator.span)?;
                match awaitable {
                    BytecodeAwaitable::Call(operation) => {
                        match self.evaluate_operation(frame, operation, span)? {
                            OperationResult::Value(value) => {
                                self.write_place(frame, destination, value)?;
                                self.jump(frame, *target);
                            }
                            OperationResult::Call {
                                function,
                                arguments,
                            } => {
                                self.push_frame(
                                    function,
                                    arguments,
                                    Some(CallContinuation {
                                        destination: Some(destination.clone()),
                                        target: Some(*target),
                                        unwind: *unwind,
                                        call_span: span,
                                        test_boundary: None,
                                        virtual_time: None,
                                    }),
                                )?;
                            }
                            OperationResult::HostAsync {
                                name,
                                arguments,
                                outcome,
                            } => {
                                let call = self.host.start_async(&name, &arguments)?;
                                self.park_current(
                                    TaskWait::HostCall {
                                        call,
                                        outcome,
                                        destination: destination.clone(),
                                        target: *target,
                                        unwind: *unwind,
                                        completion: None,
                                    },
                                    &[],
                                )?;
                            }
                            OperationResult::OneShotWait { id, outcome } => {
                                let cancelled = self
                                    .oneshots
                                    .get(&id)
                                    .ok_or_else(|| {
                                        VmError::invariant("one-shot wait references an unknown id")
                                    })?
                                    .completion
                                    .as_ref()
                                    .is_some_and(|completion| {
                                        matches!(completion, OneShotCompletion::Cancelled)
                                    });
                                if cancelled {
                                    self.begin_cancel(frame, *unwind)?;
                                } else {
                                    self.park_oneshot(
                                        id,
                                        TaskWait::OneShot {
                                            id,
                                            outcome,
                                            destination: destination.clone(),
                                            target: *target,
                                            unwind: *unwind,
                                        },
                                    )?;
                                }
                            }
                            OperationResult::Panic(code, message) => {
                                self.begin_panic(frame, code, message, span, *unwind)?;
                            }
                            OperationResult::TestBoundaryCall { .. } => {
                                return Err(VmError::invariant(
                                    "an internal test boundary was awaited",
                                ));
                            }
                            OperationResult::VirtualTimeBoundaryCall {
                                function,
                                arguments,
                                controller,
                            } => {
                                self.push_frame(
                                    function,
                                    arguments,
                                    Some(CallContinuation {
                                        destination: Some(destination.clone()),
                                        target: Some(*target),
                                        unwind: *unwind,
                                        call_span: span,
                                        test_boundary: None,
                                        virtual_time: Some(controller),
                                    }),
                                )?;
                            }
                        }
                    }
                    BytecodeAwaitable::Join(join) => {
                        let BytecodeOperandKind::Move(owner) = &join.kind else {
                            return Err(VmError::invariant(
                                "await did not consume its affine Join",
                            ));
                        };
                        let value = self.read_place(frame, owner)?;
                        let Value::Join(join) = value else {
                            return Err(VmError::invariant(
                                "await Join operand has no task handle",
                            ));
                        };
                        let task = self
                            .tasks
                            .get(join.task)
                            .ok_or_else(|| VmError::invariant("Join references an invalid task"))?;
                        let transferred = join.scope == TRANSFERRED_JOIN_SCOPE;
                        if (!transferred
                            && (task.parent_scope != Some(join.scope)
                                || !self.frames[frame].task_scopes.contains(&join.scope)))
                            || (transferred && task.parent_scope.is_some())
                            || task.join_consumed
                        {
                            return Err(VmError::invariant(
                                "Join was consumed twice or by the wrong task scope",
                            ));
                        }
                        let parent_scope = task.parent_scope;
                        if matches!(task.status, TaskStatus::Complete(_)) {
                            let consumed = self.consume_join_owner(frame, owner)?;
                            debug_assert_eq!(consumed, join);
                            let completion =
                                self.take_task_completion(join.task)?.ok_or_else(|| {
                                    VmError::invariant("completed Join has no result")
                                })?;
                            self.apply_join_completion(
                                frame,
                                completion,
                                destination,
                                *target,
                                *unwind,
                            )?;
                            if let Some(scope) = parent_scope
                                && self.task_scopes.get(scope).is_some_and(Option::is_some)
                            {
                                self.release_task_scope_if_consumed(scope)?;
                            }
                        } else {
                            self.park_current(
                                TaskWait::Join {
                                    child: join.task,
                                    owner: owner.clone(),
                                    destination: destination.clone(),
                                    target: *target,
                                    unwind: *unwind,
                                },
                                &[join.task],
                            )?;
                        }
                    }
                }
            }
            BytecodeTerminatorKind::Spawn {
                operation,
                scope,
                kind,
                destination,
                target,
                unwind,
            } => {
                // The cooperative executor uses the same affine task record
                // for both lanes.  A host/runtime may map `Thread` to a
                // worker without changing Join ownership or cleanup.
                let _thread_lane = kind;
                if self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?
                {
                    self.begin_cancel(frame, *unwind)?;
                    return Ok(None);
                }
                let span = self.resolve_span(frame, terminator.span)?;
                match self.evaluate_operation(frame, operation, span)? {
                    OperationResult::Call {
                        function,
                        arguments,
                    } => {
                        let scope = self.active_task_scope(frame, *scope)?;
                        let child = self.spawn_task(function, arguments, scope)?;
                        self.write_place(
                            frame,
                            destination,
                            Value::Join(RuntimeJoin { task: child, scope }),
                        )?;
                        self.jump(frame, *target);
                    }
                    OperationResult::HostAsync {
                        name,
                        arguments,
                        outcome,
                    } => {
                        let scope = self.active_task_scope(frame, *scope)?;
                        let call = self.host.start_async(&name, &arguments)?;
                        let child = self.spawn_host_task(call, outcome, scope)?;
                        self.write_place(
                            frame,
                            destination,
                            Value::Join(RuntimeJoin { task: child, scope }),
                        )?;
                        self.jump(frame, *target);
                    }
                    OperationResult::OneShotWait { id, outcome } => {
                        let scope = self.active_task_scope(frame, *scope)?;
                        let child = self.spawn_oneshot_task(id, outcome, scope)?;
                        self.write_place(
                            frame,
                            destination,
                            Value::Join(RuntimeJoin { task: child, scope }),
                        )?;
                        self.jump(frame, *target);
                    }
                    OperationResult::Panic(code, message) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                    OperationResult::Value(value) => {
                        let scope = self.active_task_scope(frame, *scope)?;
                        let child = self.spawn_completed_task(value, scope)?;
                        self.write_place(
                            frame,
                            destination,
                            Value::Join(RuntimeJoin { task: child, scope }),
                        )?;
                        self.jump(frame, *target);
                    }
                    OperationResult::TestBoundaryCall { .. } => {
                        return Err(VmError::invariant("an internal test boundary was spawned"));
                    }
                    OperationResult::VirtualTimeBoundaryCall { controller, .. } => {
                        self.host.finish_virtual_time(&controller)?;
                        return Err(VmError::invariant("virtual time cannot be spawned"));
                    }
                }
            }
            BytecodeTerminatorKind::IteratorNext {
                state,
                destination,
                borrowed_source,
                exhaustion_guard,
                has_value,
                exhausted,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                match self.iterator_next(
                    frame,
                    state,
                    borrowed_source.as_ref(),
                    destination.ty,
                    span,
                )? {
                    Ok(Some(IteratorStep::Value(value))) if borrowed_source.is_none() => {
                        self.write_place(frame, destination, value)?;
                        self.jump(frame, *has_value);
                    }
                    Ok(Some(IteratorStep::Position(position))) if borrowed_source.is_some() => {
                        let position = i128::try_from(position).map_err(|_| {
                            VmError::invariant("iterator position exceeds the Int domain")
                        })?;
                        self.write_place(frame, destination, Value::Integer(position))?;
                        self.jump(frame, *has_value);
                    }
                    Ok(Some(_)) => {
                        return Err(VmError::invariant(
                            "iterator mode and terminator destination disagree",
                        ));
                    }
                    Ok(None) => {
                        if let Some(guard) = exhaustion_guard {
                            self.disarm_cleanup(frame, guard)?;
                        }
                        self.jump(frame, *exhausted);
                    }
                    Err((code, message)) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                }
            }
            BytecodeTerminatorKind::ValidatePlaces {
                places,
                replacements,
                against,
                for_write,
                target,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                let result = self.validate_places(frame, places, replacements, against, *for_write);
                match result {
                    Ok(()) => self.jump(frame, *target),
                    Err(PlaceFailure::Panic(code, message)) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                    Err(PlaceFailure::Vm(error)) => return Err(error),
                }
            }
            BytecodeTerminatorKind::ValidateLoan {
                loan,
                against,
                target,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                match self.validate_loan(frame, *loan, against) {
                    Ok(()) => self.jump(frame, *target),
                    Err(PlaceFailure::Panic(code, message)) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                    Err(PlaceFailure::Vm(error)) => return Err(error),
                }
            }
            BytecodeTerminatorKind::DrainScopes {
                task_scopes,
                defer_scopes,
                target,
                unwind,
            } => {
                if (self.tasks[self.current_task].cancel_requested
                    || self.current_scope_has_unobserved_panic(frame)?)
                    && self.pending_unwind.is_none()
                {
                    self.pending_unwind = Some(RuntimeUnwind::Cancelled);
                }
                if self.drain_task_scopes(frame, task_scopes)? {
                    self.drain_explicit_scopes(frame, defer_scopes, *target, *unwind)?;
                }
            }
            BytecodeTerminatorKind::DrainDefers {
                scopes,
                target,
                unwind,
            } => self.drain_explicit_scopes(frame, scopes, *target, *unwind)?,
            BytecodeTerminatorKind::DrainUnwind { target } => {
                let scopes = self.frames[frame]
                    .task_scopes
                    .iter()
                    .map(|id| {
                        self.task_scopes
                            .get(*id)
                            .and_then(Option::as_ref)
                            .map(|scope| scope.source)
                            .ok_or_else(|| VmError::invariant("active task scope state is missing"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if !self.drain_task_scopes(frame, &scopes)? {
                    return Ok(None);
                }
                let continuation = self.frames[frame].block;
                let Some(cleanup) = self.frames[frame].cleanups.pop() else {
                    self.jump(frame, *target);
                    return Ok(None);
                };
                match cleanup {
                    RuntimeCleanup::Explicit(deferred) => {
                        let span = deferred.span;
                        let async_cleanup = deferred.async_cleanup;
                        match self.evaluate_deferred_operation(frame, deferred)? {
                            OperationResult::Value(Value::Unit) => {
                                self.jump(frame, continuation);
                            }
                            OperationResult::Value(_) => {
                                return Err(VmError::invariant(
                                    "deferred invocation returned a non-Unit value",
                                ));
                            }
                            OperationResult::Call {
                                function,
                                arguments,
                            } => {
                                self.push_frame(
                                    function,
                                    arguments,
                                    Some(CallContinuation {
                                        destination: None,
                                        target: Some(continuation),
                                        unwind: continuation,
                                        call_span: span,
                                        test_boundary: None,
                                        virtual_time: None,
                                    }),
                                )?;
                            }
                            OperationResult::HostAsync {
                                name,
                                arguments,
                                outcome,
                            } => {
                                if !async_cleanup {
                                    return Err(VmError::invariant(
                                        "a synchronous defer attempted an async host call",
                                    ));
                                }
                                let call = self.host.start_async(&name, &arguments)?;
                                self.park_current(
                                    TaskWait::DeferredHostCall {
                                        call,
                                        outcome,
                                        target: continuation,
                                        completion: None,
                                    },
                                    &[],
                                )?;
                            }
                            OperationResult::OneShotWait { .. } => {
                                return Err(VmError::invariant(
                                    "a one-shot wait cannot be used as deferred cleanup",
                                ));
                            }
                            OperationResult::Panic(code, message) => {
                                self.begin_panic(frame, code, message, span, continuation)?;
                            }
                            OperationResult::TestBoundaryCall { .. } => {
                                return Err(VmError::invariant(
                                    "an internal test boundary was deferred",
                                ));
                            }
                            OperationResult::VirtualTimeBoundaryCall { .. } => {
                                return Err(VmError::invariant("virtual time cannot be spawned"));
                            }
                        }
                    }
                    RuntimeCleanup::Fallback(fallback) => {
                        self.execute_terminal_fallback(frame, fallback)?;
                        self.jump(frame, continuation);
                    }
                }
            }
            BytecodeTerminatorKind::Return => {
                if !self.frames[frame].task_scopes.is_empty() {
                    return Err(VmError::invariant(
                        "a function returned with active structured task scopes",
                    ));
                }
                if self.frames[frame]
                    .cleanups
                    .iter()
                    .any(|cleanup| matches!(cleanup, RuntimeCleanup::Explicit(_)))
                {
                    return Err(VmError::invariant(
                        "a function returned with registered explicit cleanup entries",
                    ));
                }
                self.frames[frame].cleanups.clear();
                if self.frames[frame].loans.iter().any(Option::is_some) {
                    return Err(VmError::invariant(
                        "a function returned with an active loan reservation",
                    ));
                }
                let function = self
                    .program
                    .function(self.frames[frame].function)
                    .ok_or_else(|| VmError::invariant("returning frame has an invalid function"))?;
                let value = self.take_slot(frame, function.return_slot)?;
                let finished = self
                    .frames
                    .pop()
                    .ok_or_else(|| VmError::invariant("return could not pop the current frame"))?;
                if let Some(continuation) = finished.continuation {
                    let caller = self.frames.len().checked_sub(1).ok_or_else(|| {
                        VmError::invariant("callee returned without its caller frame")
                    })?;
                    if let Some(boundary) = &continuation.test_boundary {
                        if value != Value::Unit {
                            return Err(VmError::invariant(
                                "internal test boundary returned a non-Unit value",
                            ));
                        }
                        self.host.finish_test_node(
                            boundary.kind,
                            &boundary.id,
                            VmTestNodeOutcome::Passed,
                        )?;
                    }
                    if let Some(controller) = &continuation.virtual_time {
                        self.host.finish_virtual_time(controller)?;
                    }
                    if let Some(destination) = &continuation.destination {
                        self.write_place(caller, destination, value)?;
                    }
                    let target = continuation.target.ok_or_else(|| {
                        VmError::invariant("returning call has no normal successor")
                    })?;
                    self.jump(caller, target);
                } else {
                    return Ok(Some(TaskCompletion::Returned(value)));
                }
            }
            BytecodeTerminatorKind::ResumePanic => {
                if !self.frames[frame].cleanups.is_empty() {
                    return Err(VmError::invariant(
                        "panic unwinding abandoned registered cleanup entries",
                    ));
                }
                if self.pending_unwind.is_none() {
                    return Err(VmError::invariant(
                        "ResumePanic executed without active panic or cancellation",
                    ));
                }
                let finished = self
                    .frames
                    .pop()
                    .ok_or_else(|| VmError::invariant("panic resume could not pop its frame"))?;
                if let Some(continuation) = finished.continuation {
                    let caller = self.frames.len().checked_sub(1).ok_or_else(|| {
                        VmError::invariant("panicking callee has no caller frame")
                    })?;
                    self.frames[caller].loans.fill(None);
                    if let Some(controller) = &continuation.virtual_time {
                        self.host.finish_virtual_time(controller)?;
                    }
                    if let Some(boundary) = &continuation.test_boundary {
                        let unwind = self
                            .pending_unwind
                            .take()
                            .ok_or_else(|| VmError::invariant("test boundary panic disappeared"))?;
                        match unwind {
                            RuntimeUnwind::Panic(panic) => {
                                self.host.finish_test_node(
                                    boundary.kind,
                                    &boundary.id,
                                    VmTestNodeOutcome::Panicked(panic),
                                )?;
                                if let Some(destination) = &continuation.destination {
                                    self.write_place(caller, destination, Value::Unit)?;
                                }
                                let target = continuation.target.ok_or_else(|| {
                                    VmError::invariant("test boundary call has no normal successor")
                                })?;
                                self.jump(caller, target);
                                return Ok(None);
                            }
                            RuntimeUnwind::Cancelled => {
                                self.pending_unwind = Some(RuntimeUnwind::Cancelled);
                            }
                        }
                    }
                    self.jump(caller, continuation.unwind);
                } else {
                    let unwind = self.pending_unwind.take().ok_or_else(|| {
                        VmError::invariant("root unwind disappeared during cleanup")
                    })?;
                    return Ok(Some(match unwind {
                        RuntimeUnwind::Panic(panic) => TaskCompletion::Panicked(panic),
                        RuntimeUnwind::Cancelled => TaskCompletion::Cancelled,
                    }));
                }
            }
            BytecodeTerminatorKind::Unreachable => {
                return Err(VmError::invariant("executed unreachable bytecode"));
            }
        }
        Ok(None)
    }

    fn drain_explicit_scopes(
        &mut self,
        frame: usize,
        scopes: &[BytecodeScopeId],
        target: BytecodeBlockId,
        unwind: BytecodeBlockId,
    ) -> Result<(), VmError> {
        let continuation = self.frames[frame].block;
        let next = self.frames[frame].cleanups.iter().rposition(|cleanup| {
            matches!(cleanup, RuntimeCleanup::Explicit(_)) && scopes.contains(&cleanup.scope())
        });
        if let Some(index) = next {
            let RuntimeCleanup::Explicit(deferred) = self.frames[frame].cleanups.remove(index)
            else {
                unreachable!("normal defer drains select explicit entries");
            };
            let span = deferred.span;
            let async_cleanup = deferred.async_cleanup;
            match self.evaluate_deferred_operation(frame, deferred)? {
                OperationResult::Value(Value::Unit) => {
                    self.jump(frame, continuation);
                }
                OperationResult::Value(_) => {
                    return Err(VmError::invariant(
                        "deferred invocation returned a non-Unit value",
                    ));
                }
                OperationResult::Call {
                    function,
                    arguments,
                } => {
                    self.push_frame(
                        function,
                        arguments,
                        Some(CallContinuation {
                            destination: None,
                            target: Some(continuation),
                            unwind: continuation,
                            call_span: span,
                            test_boundary: None,
                            virtual_time: None,
                        }),
                    )?;
                }
                OperationResult::HostAsync {
                    name,
                    arguments,
                    outcome,
                } => {
                    if !async_cleanup {
                        return Err(VmError::invariant(
                            "a synchronous defer attempted an async host call",
                        ));
                    }
                    let call = self.host.start_async(&name, &arguments)?;
                    self.park_current(
                        TaskWait::DeferredHostCall {
                            call,
                            outcome,
                            target: continuation,
                            completion: None,
                        },
                        &[],
                    )?;
                }
                OperationResult::OneShotWait { .. } => {
                    return Err(VmError::invariant(
                        "a one-shot wait cannot be used as deferred cleanup",
                    ));
                }
                OperationResult::Panic(code, message) => {
                    self.begin_panic(frame, code, message, span, continuation)?;
                }
                OperationResult::TestBoundaryCall { .. } => {
                    return Err(VmError::invariant("an internal test boundary was deferred"));
                }
                OperationResult::VirtualTimeBoundaryCall {
                    function,
                    arguments,
                    controller,
                } => {
                    if !async_cleanup {
                        return Err(VmError::invariant(
                            "a synchronous defer attempted a virtual-time boundary",
                        ));
                    }
                    self.push_frame(
                        function,
                        arguments,
                        Some(CallContinuation {
                            destination: None,
                            target: Some(continuation),
                            unwind: continuation,
                            call_span: span,
                            test_boundary: None,
                            virtual_time: Some(controller),
                        }),
                    )?;
                }
            }
        } else if self.pending_unwind.is_some() {
            self.jump(frame, unwind);
        } else {
            self.jump(frame, target);
        }
        Ok(())
    }

    fn begin_panic(
        &mut self,
        frame: usize,
        code: PanicCode,
        message: String,
        span: BytecodeSpan,
        unwind: BytecodeBlockId,
    ) -> Result<(), VmError> {
        let stack = self
            .frames
            .iter()
            .rev()
            .enumerate()
            .map(|(depth, current)| {
                let function = self
                    .program
                    .function(current.function)
                    .ok_or_else(|| VmError::invariant("stack has an invalid function"))?;
                let callable = self
                    .program
                    .callable(function.callable)
                    .ok_or_else(|| VmError::invariant("stack has an invalid callable"))?;
                let location = if depth == 0 {
                    span
                } else {
                    self.frames[self.frames.len() - depth]
                        .continuation
                        .as_ref()
                        .map_or(function.source, |continuation| continuation.call_span)
                };
                Ok(VmStackFrame {
                    function: callable.name.clone(),
                    span: location,
                })
            })
            .collect::<Result<Vec<_>, VmError>>()?;
        let panic = VmPanic {
            code,
            message,
            span,
            stack,
            suppressed: Vec::new(),
        };
        if let Some(RuntimeUnwind::Panic(primary)) = &mut self.pending_unwind {
            primary.suppressed.push(panic);
        } else {
            self.pending_unwind = Some(RuntimeUnwind::Panic(panic));
        }
        self.frames[frame].loans.fill(None);
        self.jump(frame, unwind);
        Ok(())
    }

    fn begin_propagated_panic(
        &mut self,
        frame: usize,
        panic: VmPanic,
        unwind: BytecodeBlockId,
    ) -> Result<(), VmError> {
        if let Some(RuntimeUnwind::Panic(primary)) = &mut self.pending_unwind {
            primary.suppressed.push(panic);
        } else {
            self.pending_unwind = Some(RuntimeUnwind::Panic(panic));
        }
        self.frames[frame].loans.fill(None);
        self.jump(frame, unwind);
        Ok(())
    }

    fn begin_cancel(&mut self, frame: usize, unwind: BytecodeBlockId) -> Result<(), VmError> {
        if self.pending_unwind.is_none() {
            self.pending_unwind = Some(RuntimeUnwind::Cancelled);
        }
        self.frames[frame].loans.fill(None);
        self.jump(frame, unwind);
        Ok(())
    }

    fn jump(&mut self, frame: usize, target: BytecodeBlockId) {
        self.frames[frame].block = target;
        self.frames[frame].instruction = 0;
    }

    fn resolve_span(
        &self,
        frame: usize,
        span: crate::bytecode::BytecodeSpanId,
    ) -> Result<BytecodeSpan, VmError> {
        let function = self
            .program
            .function(self.frames[frame].function)
            .ok_or_else(|| VmError::invariant("frame has an invalid function"))?;
        function
            .span(span)
            .ok_or_else(|| VmError::invariant("instruction has an invalid source span"))
    }

    fn slot_mut(
        &mut self,
        frame: usize,
        slot: crate::bytecode::BytecodeSlotId,
    ) -> Result<&mut SlotState, VmError> {
        self.frames
            .get_mut(frame)
            .and_then(|frame| frame.slots.get_mut(slot.index() as usize))
            .ok_or_else(|| VmError::invariant("slot access escaped the current frame"))
    }

    fn read_slot(
        &self,
        frame: usize,
        slot: crate::bytecode::BytecodeSlotId,
    ) -> Result<&Value, VmError> {
        match self
            .frames
            .get(frame)
            .and_then(|frame| frame.slots.get(slot.index() as usize))
        {
            Some(SlotState::Value(value)) => Ok(value),
            Some(SlotState::Dead) => Err(VmError::invariant("read from a dead frame slot")),
            Some(SlotState::Uninitialized) => {
                Err(VmError::invariant("read from an uninitialized frame slot"))
            }
            None => Err(VmError::invariant("read from an invalid frame slot")),
        }
    }

    fn root_loan(
        &self,
        frame: usize,
        slot: crate::bytecode::BytecodeSlotId,
    ) -> Result<Option<RuntimeLoan>, VmError> {
        match self
            .frames
            .get(frame)
            .and_then(|frame| frame.slots.get(slot.index() as usize))
        {
            Some(SlotState::Value(Value::Loan(loan))) => Ok(Some(loan.clone())),
            Some(SlotState::Value(_)) | Some(SlotState::Uninitialized) | Some(SlotState::Dead) => {
                Ok(None)
            }
            None => Err(VmError::invariant("read from an invalid frame slot")),
        }
    }

    fn validate_source_regions(
        &self,
        frame: usize,
        place: &BytecodePlace,
        read_only: bool,
    ) -> Result<Option<BytecodeParameterMode>, VmError> {
        let Some(mut source) = place.source_loan else {
            return Ok(None);
        };
        let function_id = self
            .frames
            .get(frame)
            .ok_or_else(|| VmError::invariant("region access escaped the current frame"))?
            .function;
        let function = self
            .program
            .function(function_id)
            .ok_or_else(|| VmError::invariant("region frame has an invalid function"))?;
        let first_mode = function
            .loans
            .get(source.index() as usize)
            .map(|loan| loan.mode)
            .ok_or_else(|| VmError::invariant("place references an invalid source region"))?;
        let mut visited = Vec::new();
        loop {
            if visited.contains(&source) {
                return Err(VmError::invariant(
                    "place source region chain contains a cycle",
                ));
            }
            visited.push(source);
            let loan = function
                .loans
                .get(source.index() as usize)
                .ok_or_else(|| VmError::invariant("place references an invalid source region"))?;
            if loan.kind != BytecodeLoanKind::Region {
                return Err(VmError::invariant(
                    "place source is not a region reservation",
                ));
            }
            if !read_only && loan.mode == BytecodeParameterMode::Ref {
                return Err(VmError::invariant(
                    "a move or write attempted to use a shared region reference",
                ));
            }
            let reservation = self.frames[frame]
                .loans
                .get(source.index() as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| VmError::invariant("place uses an inactive source region"))?;
            if reservation.mode != loan.mode {
                return Err(VmError::invariant(
                    "source region metadata differs from its active reservation",
                ));
            }
            let Some(parent) = loan.place.source_loan else {
                return Ok(Some(first_mode));
            };
            source = parent;
        }
    }

    fn take_slot(
        &mut self,
        frame: usize,
        slot: crate::bytecode::BytecodeSlotId,
    ) -> Result<Value, VmError> {
        let state = self.slot_mut(frame, slot)?;
        match std::mem::replace(state, SlotState::Uninitialized) {
            SlotState::Value(value) => Ok(value),
            SlotState::Dead => {
                *state = SlotState::Dead;
                Err(VmError::invariant("move from a dead frame slot"))
            }
            SlotState::Uninitialized => {
                Err(VmError::invariant("move from an uninitialized frame slot"))
            }
        }
    }

    fn roots(&self, extra: &[Value]) -> Result<Vec<Value>, VmError> {
        let mut roots = extra.to_vec();
        roots.extend(self.temporary_roots.iter().cloned());
        self.append_frame_roots(&self.frames, &mut roots)?;
        for (task_id, task) in self.tasks.iter().enumerate() {
            if task_id != self.current_task {
                self.append_frame_roots(&task.frames, &mut roots)?;
            }
            if let TaskStatus::Complete(Some(TaskCompletion::Returned(value))) = &task.status {
                roots.push(value.clone());
            }
        }
        for state in self.oneshots.values() {
            match &state.completion {
                Some(OneShotCompletion::Ok(value)) | Some(OneShotCompletion::Err(value)) => {
                    roots.push(value.clone());
                }
                Some(OneShotCompletion::Cancelled) | None => {}
            }
        }
        Ok(roots)
    }

    fn append_frame_roots(&self, frames: &[Frame], roots: &mut Vec<Value>) -> Result<(), VmError> {
        for frame in frames {
            let trace = self
                .frame_traces
                .get(frame.function.index() as usize)
                .filter(|trace| trace.function == frame.function)
                .ok_or_else(|| VmError::invariant("live frame has no verified trace descriptor"))?;
            frame.roots(trace, roots);
        }
        Ok(())
    }

    /// Protects operation-local values until an allocation-capable step has
    /// either published them in a frame/object or failed.
    fn with_temporary_roots<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, VmError>,
    ) -> Result<T, VmError> {
        let marker = self.temporary_roots.len();
        let result = operation(self);
        self.temporary_roots.truncate(marker);
        result
    }

    fn retain_temporary(&mut self, value: &Value) {
        self.temporary_roots.push(value.clone());
    }

    fn allocate(
        &mut self,
        descriptor: BytecodeTypeId,
        object: HeapObject,
        extra: &[Value],
    ) -> Result<Value, VmError> {
        let roots = self.roots(extra)?;
        self.heap
            .allocate(descriptor, object, &roots, &mut self.statistics)
            .map(Value::Heap)
    }

    fn allocate_like(
        &mut self,
        source: HeapHandle,
        object: HeapObject,
        extra: &[Value],
    ) -> Result<Value, VmError> {
        let descriptor = self.heap.descriptor(source)?;
        self.allocate(descriptor, object, extra)
    }

    fn replace_object(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
        extra: &[Value],
    ) -> Result<(), VmError> {
        let roots = self.roots(extra)?;
        self.heap
            .replace(handle, object, &roots, &mut self.statistics)
    }

    // Value evaluation, places, operators, iterators, and calls continue below.
}

fn runtime_host_kind(constructor: BytecodeIntrinsicType) -> Option<RuntimeHostValueKind> {
    Some(match constructor {
        BytecodeIntrinsicType::Command => RuntimeHostValueKind::Command,
        BytecodeIntrinsicType::Pipeline => RuntimeHostValueKind::Pipeline,
        BytecodeIntrinsicType::Bytes => RuntimeHostValueKind::Bytes,
        BytecodeIntrinsicType::BytesBuilder => RuntimeHostValueKind::BytesBuilder,
        BytecodeIntrinsicType::BytesError => RuntimeHostValueKind::BytesError,
        BytecodeIntrinsicType::FormatBuilder => RuntimeHostValueKind::FormatBuilder,
        BytecodeIntrinsicType::FormatError => RuntimeHostValueKind::FormatError,
        BytecodeIntrinsicType::TextError => RuntimeHostValueKind::TextError,
        BytecodeIntrinsicType::CollectionError => RuntimeHostValueKind::CollectionError,
        BytecodeIntrinsicType::Path => RuntimeHostValueKind::Path,
        BytecodeIntrinsicType::PathError => RuntimeHostValueKind::PathError,
        BytecodeIntrinsicType::File => RuntimeHostValueKind::File,
        BytecodeIntrinsicType::Directory => RuntimeHostValueKind::Directory,
        BytecodeIntrinsicType::Metadata => RuntimeHostValueKind::Metadata,
        BytecodeIntrinsicType::OpenMode => RuntimeHostValueKind::OpenMode,
        BytecodeIntrinsicType::FsError => RuntimeHostValueKind::FsError,
        BytecodeIntrinsicType::MathError => RuntimeHostValueKind::MathError,
        BytecodeIntrinsicType::FloatTolerance => RuntimeHostValueKind::FloatTolerance,
        BytecodeIntrinsicType::FloatToleranceError => RuntimeHostValueKind::FloatToleranceError,
        BytecodeIntrinsicType::TextDiff => RuntimeHostValueKind::TextDiff,
        BytecodeIntrinsicType::TempDirectory => RuntimeHostValueKind::TempDirectory,
        BytecodeIntrinsicType::TempError => RuntimeHostValueKind::TempError,
        BytecodeIntrinsicType::Generator => RuntimeHostValueKind::Generator,
        BytecodeIntrinsicType::GenerationId => RuntimeHostValueKind::GenerationId,
        BytecodeIntrinsicType::GenerationError => RuntimeHostValueKind::GenerationError,
        BytecodeIntrinsicType::Reader => RuntimeHostValueKind::Reader,
        BytecodeIntrinsicType::Writer => RuntimeHostValueKind::Writer,
        BytecodeIntrinsicType::IoLimits => RuntimeHostValueKind::IoLimits,
        BytecodeIntrinsicType::IoError => RuntimeHostValueKind::IoError,
        BytecodeIntrinsicType::ConsoleError => RuntimeHostValueKind::ConsoleError,
        BytecodeIntrinsicType::ExitStatus => RuntimeHostValueKind::ExitStatus,
        BytecodeIntrinsicType::ProcessOutput => RuntimeHostValueKind::ProcessOutput,
        BytecodeIntrinsicType::ProcessHandle => RuntimeHostValueKind::ProcessHandle,
        BytecodeIntrinsicType::ProcessError => RuntimeHostValueKind::ProcessError,
        BytecodeIntrinsicType::ProcessExitError => RuntimeHostValueKind::ProcessExitError,
        BytecodeIntrinsicType::Utf8Error => RuntimeHostValueKind::Utf8Error,
        BytecodeIntrinsicType::Instant => RuntimeHostValueKind::Instant,
        BytecodeIntrinsicType::Timer => RuntimeHostValueKind::Timer,
        BytecodeIntrinsicType::DurationError => RuntimeHostValueKind::DurationError,
        BytecodeIntrinsicType::ClockError => RuntimeHostValueKind::ClockError,
        BytecodeIntrinsicType::EnvSnapshot => RuntimeHostValueKind::EnvSnapshot,
        BytecodeIntrinsicType::EnvName => RuntimeHostValueKind::EnvName,
        BytecodeIntrinsicType::EnvValue => RuntimeHostValueKind::EnvValue,
        BytecodeIntrinsicType::EnvError => RuntimeHostValueKind::EnvError,
        BytecodeIntrinsicType::VirtualTime => RuntimeHostValueKind::VirtualTime,
        BytecodeIntrinsicType::JsonLimits
        | BytecodeIntrinsicType::JsonDecodeOptions
        | BytecodeIntrinsicType::JsonEncodeOptions
        | BytecodeIntrinsicType::JsonDuplicatePolicy
        | BytecodeIntrinsicType::JsonUnknownFieldPolicy
        | BytecodeIntrinsicType::JsonNumberPolicy => RuntimeHostValueKind::JsonValue,
        BytecodeIntrinsicType::JsonValue => RuntimeHostValueKind::JsonValue,
        BytecodeIntrinsicType::JsonValueView => RuntimeHostValueKind::JsonValueView,
        BytecodeIntrinsicType::JsonRaw => RuntimeHostValueKind::JsonRaw,
        BytecodeIntrinsicType::JsonNumber => RuntimeHostValueKind::JsonNumber,
        BytecodeIntrinsicType::JsonReader => RuntimeHostValueKind::JsonReader,
        BytecodeIntrinsicType::JsonEvent => RuntimeHostValueKind::JsonEvent,
        BytecodeIntrinsicType::JsonWriter => RuntimeHostValueKind::JsonWriter,
        BytecodeIntrinsicType::JsonError => RuntimeHostValueKind::JsonError,
        BytecodeIntrinsicType::Array
        | BytecodeIntrinsicType::Map
        | BytecodeIntrinsicType::Set
        | BytecodeIntrinsicType::Range
        | BytecodeIntrinsicType::Ref
        | BytecodeIntrinsicType::Pointer
        | BytecodeIntrinsicType::Join
        | BytecodeIntrinsicType::Waiter
        | BytecodeIntrinsicType::Completer
        | BytecodeIntrinsicType::AlreadyCompleted
        | BytecodeIntrinsicType::Duration
        | BytecodeIntrinsicType::NumericConversionError => return None,
    })
}

fn oneshot_handle(value: &Value, expected: RuntimeHostValueKind) -> Result<u64, VmError> {
    let Value::Host(RuntimeValue::Host { kind, id }) = value else {
        return Err(VmError::invariant("one-shot handle is not opaque"));
    };
    if *kind != expected || *id == 0 {
        return Err(VmError::invariant("one-shot handle has the wrong kind"));
    }
    Ok(*id)
}

enum OperationResult {
    Value(Value),
    Call {
        function: BytecodeFunctionId,
        arguments: Vec<Value>,
    },
    TestBoundaryCall {
        function: BytecodeFunctionId,
        arguments: Vec<Value>,
        boundary: TestBoundary,
    },
    VirtualTimeBoundaryCall {
        function: BytecodeFunctionId,
        arguments: Vec<Value>,
        controller: RuntimeValue,
    },
    HostAsync {
        name: String,
        arguments: Vec<RuntimeValue>,
        outcome: BytecodeTypeId,
    },
    OneShotWait {
        id: u64,
        outcome: BytecodeTypeId,
    },
    Panic(PanicCode, String),
}

enum IteratorStep {
    Value(Value),
    Position(usize),
}

enum PlaceFailure {
    Panic(PanicCode, String),
    Vm(VmError),
}

impl From<VmError> for PlaceFailure {
    fn from(error: VmError) -> Self {
        Self::Vm(error)
    }
}

impl Engine<'_, '_> {
    fn evaluate_operand(
        &mut self,
        frame: usize,
        operand: &BytecodeOperand,
    ) -> Result<Value, VmError> {
        match &operand.kind {
            BytecodeOperandKind::Constant(constant) => self.inline_constant(operand.ty, constant),
            BytecodeOperandKind::Copy(place) => {
                let value = self.read_place(frame, place)?;
                self.copy_value(&value)
            }
            BytecodeOperandKind::Move(place) => self.take_place(frame, place),
            BytecodeOperandKind::Borrow(place) => self.read_place(frame, place),
            BytecodeOperandKind::Loan(_) => Err(VmError::invariant(
                "a loan operand escaped its consuming call",
            )),
            BytecodeOperandKind::Function {
                callable,
                arguments,
            } => Ok(Value::Function {
                callable: *callable,
                arguments: arguments.clone(),
            }),
        }
    }

    fn inline_constant(
        &mut self,
        ty: BytecodeTypeId,
        constant: &BytecodeConstant,
    ) -> Result<Value, VmError> {
        match constant {
            BytecodeConstant::Unit => Ok(Value::Unit),
            BytecodeConstant::Bool(value) => Ok(Value::Bool(*value)),
            BytecodeConstant::Integer(spelling) => {
                let value = literal::integer(spelling)
                    .ok_or_else(|| VmError::invariant("verified integer literal is malformed"))?;
                if self.scalar(ty)? == BytecodeScalarType::Byte {
                    u8::try_from(value)
                        .map(Value::Byte)
                        .map_err(|_| VmError::invariant("verified Byte literal is out of range"))
                } else {
                    Ok(Value::Integer(value))
                }
            }
            BytecodeConstant::Float(spelling) => {
                let single = self.scalar(ty)? == BytecodeScalarType::Float32;
                literal::float(spelling, single)
                    .map(Value::Float)
                    .ok_or_else(|| VmError::invariant("verified float literal is malformed"))
            }
            BytecodeConstant::Char(spelling) => literal::character(spelling)
                .map(Value::Char)
                .ok_or_else(|| VmError::invariant("verified character literal is malformed")),
            BytecodeConstant::String(spelling) => {
                let text = literal::string(spelling)
                    .ok_or_else(|| VmError::invariant("verified string literal is malformed"))?;
                self.allocate(ty, HeapObject::String(text), &[])
            }
            BytecodeConstant::Named(id) => {
                let value = self
                    .program
                    .constants
                    .get(id.index() as usize)
                    .ok_or_else(|| VmError::invariant("named constant index is invalid"))?
                    .value
                    .clone();
                self.materialize_constant(&value)
            }
        }
    }

    fn materialize_constant(&mut self, constant: &BytecodeConstantValue) -> Result<Value, VmError> {
        match &constant.kind {
            BytecodeConstantValueKind::Unit => Ok(Value::Unit),
            BytecodeConstantValueKind::Bool(value) => Ok(Value::Bool(*value)),
            BytecodeConstantValueKind::Integer(value) => {
                if self.scalar(constant.ty)? == BytecodeScalarType::Byte {
                    u8::try_from(*value).map(Value::Byte).map_err(|_| {
                        VmError::invariant("verified Byte constant is outside its range")
                    })
                } else {
                    Ok(Value::Integer(*value))
                }
            }
            BytecodeConstantValueKind::Float(bits) => Ok(Value::Float(f64::from_bits(*bits))),
            BytecodeConstantValueKind::Char(value) => Ok(Value::Char(*value)),
            BytecodeConstantValueKind::String(value) => {
                self.allocate(constant.ty, HeapObject::String(value.clone()), &[])
            }
            BytecodeConstantValueKind::Function {
                callable,
                arguments,
            } => Ok(Value::Function {
                callable: *callable,
                arguments: arguments.clone(),
            }),
            BytecodeConstantValueKind::Tuple(values) => {
                let values = self.materialize_constants(values)?;
                self.allocate(
                    constant.ty,
                    HeapObject::Tuple(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            BytecodeConstantValueKind::Array(values) => {
                let values = self.materialize_constants(values)?;
                self.allocate(
                    constant.ty,
                    HeapObject::Array(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            BytecodeConstantValueKind::Map(entries) => self.with_temporary_roots(|engine| {
                let mut output = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = engine.materialize_constant(key)?;
                    engine.retain_temporary(&key);
                    let value = engine.materialize_constant(value)?;
                    engine.retain_temporary(&value);
                    output.push((Some(key), Some(value)));
                }
                engine.allocate(constant.ty, HeapObject::Map(output.into()), &[])
            }),
            BytecodeConstantValueKind::Set(values) => {
                let values = self.materialize_constants(values)?;
                self.allocate(
                    constant.ty,
                    HeapObject::Set(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            BytecodeConstantValueKind::Newtype { nominal, value } => {
                let value = self.materialize_constant(value)?;
                self.allocate(
                    constant.ty,
                    HeapObject::Newtype {
                        nominal: *nominal,
                        value: Some(value.clone()),
                    },
                    &[value],
                )
            }
            BytecodeConstantValueKind::Record { nominal, fields } => {
                self.with_temporary_roots(|engine| {
                    let mut output = Vec::with_capacity(fields.len());
                    for (field, value) in fields {
                        let value = engine.materialize_constant(value)?;
                        engine.retain_temporary(&value);
                        output.push((*field, Some(value)));
                    }
                    engine.allocate(
                        constant.ty,
                        HeapObject::Record {
                            nominal: *nominal,
                            fields: output,
                        },
                        &[],
                    )
                })
            }
            BytecodeConstantValueKind::Variant { variant, payload } => {
                let payload = self.materialize_constant_payload(payload)?;
                let mut roots = Vec::new();
                payload.trace_values(&mut roots);
                self.allocate(
                    constant.ty,
                    HeapObject::Variant {
                        variant: *variant,
                        payload,
                    },
                    &roots,
                )
            }
            BytecodeConstantValueKind::OptionNone => {
                self.allocate(constant.ty, HeapObject::OptionNone, &[])
            }
            BytecodeConstantValueKind::OptionSome(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(
                    constant.ty,
                    HeapObject::OptionSome(Some(value.clone())),
                    &[value],
                )
            }
            BytecodeConstantValueKind::ResultOk(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(
                    constant.ty,
                    HeapObject::ResultOk(Some(value.clone())),
                    &[value],
                )
            }
            BytecodeConstantValueKind::ResultErr(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(
                    constant.ty,
                    HeapObject::ResultErr(Some(value.clone())),
                    &[value],
                )
            }
            BytecodeConstantValueKind::Range { kind, start, end } => {
                self.with_temporary_roots(|engine| {
                    let start = engine.materialize_constant(start)?;
                    engine.retain_temporary(&start);
                    let end = engine.materialize_constant(end)?;
                    engine.allocate(
                        constant.ty,
                        HeapObject::Range {
                            kind: *kind,
                            start: Some(start),
                            end: Some(end),
                        },
                        &[],
                    )
                })
            }
        }
    }

    fn materialize_constants(
        &mut self,
        constants: &[BytecodeConstantValue],
    ) -> Result<Vec<Value>, VmError> {
        self.with_temporary_roots(|engine| {
            let mut values = Vec::with_capacity(constants.len());
            for constant in constants {
                let value = engine.materialize_constant(constant)?;
                engine.retain_temporary(&value);
                values.push(value);
            }
            Ok(values)
        })
    }

    fn materialize_constant_payload(
        &mut self,
        payload: &BytecodeConstantVariantValue,
    ) -> Result<AggregatePayload, VmError> {
        match payload {
            BytecodeConstantVariantValue::Unit => Ok(AggregatePayload::Unit),
            BytecodeConstantVariantValue::Tuple(values) => Ok(AggregatePayload::Tuple(
                self.materialize_constants(values)?
                    .into_iter()
                    .map(Some)
                    .collect(),
            )),
            BytecodeConstantVariantValue::Record(fields) => self.with_temporary_roots(|engine| {
                let mut output = Vec::with_capacity(fields.len());
                for (field, value) in fields {
                    let value = engine.materialize_constant(value)?;
                    engine.retain_temporary(&value);
                    output.push((*field, Some(value)));
                }
                Ok(AggregatePayload::Record(output))
            }),
        }
    }

    fn copy_value(&mut self, value: &Value) -> Result<Value, VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(value);
        let result = self.copy_rooted_value(value);
        self.temporary_roots.truncate(marker);
        result
    }

    fn copy_rooted_value(&mut self, value: &Value) -> Result<Value, VmError> {
        if matches!(value, Value::Loan(_)) {
            return Err(VmError::invariant(
                "a call-local loan was copied as a first-class value",
            ));
        }
        let Value::Heap(handle) = value else {
            return Ok(value.clone());
        };
        let object = self.heap.get(*handle)?.clone();
        match object {
            HeapObject::String(_) | HeapObject::Ref(_) => Ok(value.clone()),
            HeapObject::Iterator {
                mode,
                source,
                next,
                adapter,
            } => {
                let source = match mode {
                    BytecodeCursorMode::Own => self.copy_optional_value(&source)?,
                    BytecodeCursorMode::Ref => source,
                    BytecodeCursorMode::Mut => {
                        return Err(VmError::invariant(
                            "an exclusive iterator was copied as a first-class value",
                        ));
                    }
                };
                let adapter = match adapter {
                    Some(IteratorAdapter::Map {
                        callback,
                        source_item,
                    }) => Some(IteratorAdapter::Map {
                        callback: self.copy_rooted_value(&callback)?,
                        source_item,
                    }),
                    Some(IteratorAdapter::Filter {
                        callback,
                        source_item,
                    }) => Some(IteratorAdapter::Filter {
                        callback: self.copy_rooted_value(&callback)?,
                        source_item,
                    }),
                    Some(IteratorAdapter::Take {
                        remaining,
                        source_item,
                    }) => Some(IteratorAdapter::Take {
                        remaining,
                        source_item,
                    }),
                    None => None,
                };
                let mut roots = source.iter().cloned().collect::<Vec<_>>();
                if let Some(
                    IteratorAdapter::Map { callback, .. }
                    | IteratorAdapter::Filter { callback, .. },
                ) = &adapter
                {
                    roots.push(callback.clone());
                }
                self.allocate_like(
                    *handle,
                    HeapObject::Iterator {
                        mode,
                        source,
                        next,
                        adapter,
                    },
                    &roots,
                )
            }
            HeapObject::Tuple(values) => {
                let values = self.copy_optional_values(&values)?;
                self.allocate_like(*handle, HeapObject::Tuple(values), &[])
            }
            HeapObject::Array(values) => {
                self.statistics.logical_collection_copies =
                    self.statistics.logical_collection_copies.saturating_add(1);
                if self.copy_strategy == ValueCopyStrategy::CopyOnWrite
                    && self.collection_buffer_is_shareable(*handle)?
                {
                    self.statistics.collection_buffer_shares =
                        self.statistics.collection_buffer_shares.saturating_add(1);
                    self.allocate_like(*handle, HeapObject::Array(values), &[])
                } else {
                    self.statistics.collection_elements_copied = self
                        .statistics
                        .collection_elements_copied
                        .saturating_add(values.len() as u64);
                    let values = self.copy_optional_values(&values)?;
                    self.allocate_like(*handle, HeapObject::Array(values.into()), &[])
                }
            }
            HeapObject::Map(entries) => {
                self.statistics.logical_collection_copies =
                    self.statistics.logical_collection_copies.saturating_add(1);
                if self.copy_strategy == ValueCopyStrategy::CopyOnWrite
                    && self.collection_buffer_is_shareable(*handle)?
                {
                    self.statistics.collection_buffer_shares =
                        self.statistics.collection_buffer_shares.saturating_add(1);
                    self.allocate_like(*handle, HeapObject::Map(entries), &[])
                } else {
                    self.statistics.collection_elements_copied = self
                        .statistics
                        .collection_elements_copied
                        .saturating_add(entries.len() as u64);
                    let output = self.copy_map_entries(&entries)?;
                    self.allocate_like(*handle, HeapObject::Map(output.into()), &[])
                }
            }
            HeapObject::Set(values) => {
                self.statistics.logical_collection_copies =
                    self.statistics.logical_collection_copies.saturating_add(1);
                if self.copy_strategy == ValueCopyStrategy::CopyOnWrite {
                    self.statistics.collection_buffer_shares =
                        self.statistics.collection_buffer_shares.saturating_add(1);
                    self.allocate_like(*handle, HeapObject::Set(values), &[])
                } else {
                    self.statistics.collection_elements_copied = self
                        .statistics
                        .collection_elements_copied
                        .saturating_add(values.len() as u64);
                    let values = self.copy_optional_values(&values)?;
                    self.allocate_like(*handle, HeapObject::Set(values.into()), &[])
                }
            }
            HeapObject::Closure { callable, captures } => {
                let captures = self.copy_optional_values(&captures)?;
                self.allocate_like(*handle, HeapObject::Closure { callable, captures }, &[])
            }
            HeapObject::Newtype { nominal, value } => {
                let value = self.copy_optional_value(&value)?;
                self.allocate_like(*handle, HeapObject::Newtype { nominal, value }, &[])
            }
            HeapObject::Record { nominal, fields } => {
                let output = self.copy_record_fields(&fields)?;
                self.allocate_like(
                    *handle,
                    HeapObject::Record {
                        nominal,
                        fields: output,
                    },
                    &[],
                )
            }
            HeapObject::Variant { variant, payload } => {
                let payload = self.copy_payload(&payload)?;
                self.allocate_like(*handle, HeapObject::Variant { variant, payload }, &[])
            }
            HeapObject::OptionNone => self.allocate_like(*handle, HeapObject::OptionNone, &[]),
            HeapObject::OptionSome(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate_like(*handle, HeapObject::OptionSome(value), &[])
            }
            HeapObject::ResultOk(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate_like(*handle, HeapObject::ResultOk(value), &[])
            }
            HeapObject::ResultErr(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate_like(*handle, HeapObject::ResultErr(value), &[])
            }
            HeapObject::Union { member, value } => {
                let value = self.copy_optional_value(&value)?;
                self.allocate_like(*handle, HeapObject::Union { member, value }, &[])
            }
            HeapObject::Range { kind, start, end } => {
                let start = self.copy_optional_value(&start)?;
                self.retain_optional_temporary(&start);
                let end = self.copy_optional_value(&end)?;
                self.allocate_like(*handle, HeapObject::Range { kind, start, end }, &[])
            }
        }
    }

    fn collection_buffer_is_shareable(&self, handle: HeapHandle) -> Result<bool, VmError> {
        let ty = self.heap.descriptor(handle)?;
        let kind = &self
            .program
            .ty(ty)
            .ok_or_else(|| VmError::invariant("collection has an unknown bytecode type"))?
            .kind;
        let BytecodeTypeKind::Intrinsic {
            constructor,
            arguments,
        } = kind
        else {
            return Ok(false);
        };
        let expected_arity = match constructor {
            BytecodeIntrinsicType::Array | BytecodeIntrinsicType::Set => 1,
            BytecodeIntrinsicType::Map => 2,
            _ => return Ok(false),
        };
        if arguments.len() != expected_arity {
            return Err(VmError::invariant("collection type has the wrong arity"));
        }
        for stored in arguments {
            if !self.shallow_value_is_shareable(*stored)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn shallow_value_is_shareable(&self, mut ty: BytecodeTypeId) -> Result<bool, VmError> {
        loop {
            let kind = &self
                .program
                .ty(ty)
                .ok_or_else(|| VmError::invariant("collection element has an unknown type"))?
                .kind;
            match kind {
                BytecodeTypeKind::Scalar(_) => return Ok(true),
                BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Ref,
                    ..
                } => return Ok(true),
                BytecodeTypeKind::OpaqueResult { witness, .. } => ty = *witness,
                _ => return Ok(false),
            }
        }
    }

    fn copy_optional_values(
        &mut self,
        values: &[Option<Value>],
    ) -> Result<Vec<Option<Value>>, VmError> {
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut output = Vec::with_capacity(values.len());
            for value in values {
                let value = self.copy_optional_value(value)?;
                self.retain_optional_temporary(&value);
                output.push(value);
            }
            Ok(output)
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn copy_optional_value(&mut self, value: &Option<Value>) -> Result<Option<Value>, VmError> {
        value
            .as_ref()
            .map(|value| self.copy_value(value))
            .transpose()
    }

    fn copy_map_entries(&mut self, entries: &[HeapMapEntry]) -> Result<Vec<HeapMapEntry>, VmError> {
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut output = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key = self.copy_optional_value(key)?;
                self.retain_optional_temporary(&key);
                let value = self.copy_optional_value(value)?;
                self.retain_optional_temporary(&value);
                output.push((key, value));
            }
            Ok(output)
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn copy_record_fields(
        &mut self,
        fields: &[(u32, Option<Value>)],
    ) -> Result<Vec<(u32, Option<Value>)>, VmError> {
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut output = Vec::with_capacity(fields.len());
            for (field, value) in fields {
                let value = self.copy_optional_value(value)?;
                self.retain_optional_temporary(&value);
                output.push((*field, value));
            }
            Ok(output)
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn retain_optional_temporary(&mut self, value: &Option<Value>) {
        if let Some(value) = value {
            self.temporary_roots.push(value.clone());
        }
    }

    fn copy_payload(&mut self, payload: &AggregatePayload) -> Result<AggregatePayload, VmError> {
        Ok(match payload {
            AggregatePayload::Unit => AggregatePayload::Unit,
            AggregatePayload::Tuple(values) => {
                AggregatePayload::Tuple(self.copy_optional_values(values)?)
            }
            AggregatePayload::Record(fields) => {
                AggregatePayload::Record(self.copy_record_fields(fields)?)
            }
        })
    }

    fn evaluate_rvalue(&mut self, frame: usize, rvalue: &BytecodeRvalue) -> Result<Value, VmError> {
        match &rvalue.kind {
            BytecodeRvalueKind::Use(operand) => self.evaluate_operand(frame, operand),
            BytecodeRvalueKind::Prefix { operator, operand } => {
                let value = self.evaluate_operand(frame, operand)?;
                self.pure_prefix(*operator, operand.ty, value)
            }
            BytecodeRvalueKind::Binary {
                operator,
                left,
                right,
            } => self.with_temporary_roots(|engine| {
                let left_value = engine.evaluate_operand(frame, left)?;
                engine.retain_temporary(&left_value);
                let right_value = engine.evaluate_operand(frame, right)?;
                engine.pure_binary(*operator, left.ty, right.ty, left_value, right_value)
            }),
            BytecodeRvalueKind::Construct { shape, values } => {
                let values = self.evaluate_operands(frame, values)?;
                self.construct_aggregate(rvalue.ty, shape, values)
            }
            BytecodeRvalueKind::RecordUpdate { base, fields } => {
                self.with_temporary_roots(|engine| {
                    let base = engine.evaluate_operand(frame, base)?;
                    engine.retain_temporary(&base);
                    let Value::Heap(handle) = base else {
                        return Err(VmError::invariant("record update base is not managed"));
                    };
                    let HeapObject::Record {
                        nominal,
                        fields: mut output,
                    } = engine.heap.get(handle)?.clone()
                    else {
                        return Err(VmError::invariant("record update base is not a record"));
                    };
                    for (field, value) in fields {
                        let value = engine.evaluate_operand(frame, value)?;
                        engine.retain_temporary(&value);
                        let destination = output
                            .iter_mut()
                            .find(|(candidate, _)| candidate == field)
                            .ok_or_else(|| VmError::invariant("record update field is missing"))?;
                        destination.1 = Some(value);
                    }
                    engine.allocate(
                        rvalue.ty,
                        HeapObject::Record {
                            nominal,
                            fields: output,
                        },
                        &[],
                    )
                })
            }
            BytecodeRvalueKind::Coerce { kind, value } => {
                let value_result = self.evaluate_operand(frame, value)?;
                match kind {
                    BytecodeCoercion::Exact
                    | BytecodeCoercion::Opaque
                    | BytecodeCoercion::CallableErasure
                    | BytecodeCoercion::CallableOnceErasure => Ok(value_result),
                    BytecodeCoercion::UnionInjection => self.allocate(
                        rvalue.ty,
                        HeapObject::Union {
                            member: value.ty,
                            value: Some(value_result.clone()),
                        },
                        &[value_result],
                    ),
                    BytecodeCoercion::UnionWidening => Ok(value_result),
                    BytecodeCoercion::OptionLift => self.allocate(
                        rvalue.ty,
                        HeapObject::OptionSome(Some(value_result.clone())),
                        &[value_result],
                    ),
                    BytecodeCoercion::Diverging => Err(VmError::invariant(
                        "a Never coercion produced a runtime value",
                    )),
                }
            }
            BytecodeRvalueKind::NumericConversion {
                target,
                conversion,
                value,
            } => {
                let value = self.evaluate_operand(frame, value)?;
                self.numeric_conversion(rvalue.ty, *target, *conversion, value)
            }
            BytecodeRvalueKind::Range { kind, start, end } => self.with_temporary_roots(|engine| {
                let start = engine.evaluate_operand(frame, start)?;
                engine.retain_temporary(&start);
                let end = engine.evaluate_operand(frame, end)?;
                engine.allocate(
                    rvalue.ty,
                    HeapObject::Range {
                        kind: *kind,
                        start: Some(start),
                        end: Some(end),
                    },
                    &[],
                )
            }),
            BytecodeRvalueKind::Contains {
                kind,
                item,
                container,
            } => self.with_temporary_roots(|engine| {
                let item = engine.evaluate_operand(frame, item)?;
                engine.retain_temporary(&item);
                let container = engine.evaluate_operand(frame, container)?;
                Ok(Value::Bool(engine.contains(*kind, &item, &container)?))
            }),
            BytecodeRvalueKind::MapRemove { map, key } => self.with_temporary_roots(|engine| {
                let map = engine.read_place(frame, map)?;
                engine.retain_temporary(&map);
                let key = engine.evaluate_operand(frame, key)?;
                engine.retain_temporary(&key);
                engine.map_remove(rvalue.ty, map, &key)
            }),
            BytecodeRvalueKind::Interpolate { segments, values } => {
                let values = self.evaluate_operands(frame, values)?;
                self.interpolate(rvalue.ty, segments, &values)
            }
            BytecodeRvalueKind::Length(value) => {
                let value = self.evaluate_operand(frame, value)?;
                let length = i128::try_from(self.length(&value)?)
                    .map_err(|_| VmError::invariant("materialized length does not fit Int"))?;
                Ok(Value::Integer(length))
            }
            BytecodeRvalueKind::IteratorState(value) => {
                let BytecodeTypeKind::Cursor { mode, .. } = self
                    .program
                    .ty(rvalue.ty)
                    .ok_or_else(|| VmError::invariant("iterator state type is missing"))?
                    .kind
                else {
                    return Err(VmError::invariant(
                        "iterator state result is not a concrete cursor",
                    ));
                };
                let value = self.evaluate_operand(frame, value)?;
                self.allocate(
                    rvalue.ty,
                    HeapObject::Iterator {
                        mode,
                        source: Some(value.clone()),
                        next: 0,
                        adapter: None,
                    },
                    &[value],
                )
            }
        }
    }

    fn evaluate_operands(
        &mut self,
        frame: usize,
        operands: &[BytecodeOperand],
    ) -> Result<Vec<Value>, VmError> {
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut values = Vec::with_capacity(operands.len());
            for operand in operands {
                let value = self.evaluate_operand(frame, operand)?;
                self.temporary_roots.push(value.clone());
                values.push(value);
            }
            Ok(values)
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn construct_aggregate(
        &mut self,
        ty: BytecodeTypeId,
        shape: &BytecodeAggregateKind,
        values: Vec<Value>,
    ) -> Result<Value, VmError> {
        let roots = values.clone();
        let object = match shape {
            BytecodeAggregateKind::Tuple => {
                HeapObject::Tuple(values.into_iter().map(Some).collect())
            }
            BytecodeAggregateKind::Array => {
                HeapObject::Array(values.into_iter().map(Some).collect())
            }
            BytecodeAggregateKind::Set => {
                let mut unique = Vec::new();
                for value in values {
                    let mut duplicate = false;
                    for item in unique.iter().flatten() {
                        if self.value_equal(item, &value)? {
                            duplicate = true;
                            break;
                        }
                    }
                    if !duplicate {
                        unique.push(Some(value));
                    }
                }
                HeapObject::Set(unique.into())
            }
            BytecodeAggregateKind::Closure { callable, captures } => {
                if captures.len() != values.len() {
                    return Err(VmError::invariant(
                        "closure construction has the wrong capture count",
                    ));
                }
                HeapObject::Closure {
                    callable: *callable,
                    captures: values.into_iter().map(Some).collect(),
                }
            }
            BytecodeAggregateKind::Newtype { nominal } => {
                let [value] = values.try_into().map_err(|_| {
                    VmError::invariant("newtype construction has the wrong value count")
                })?;
                HeapObject::Newtype {
                    nominal: *nominal,
                    value: Some(value),
                }
            }
            BytecodeAggregateKind::Ref => {
                let [value] = values.try_into().map_err(|_| {
                    VmError::invariant("Ref construction has the wrong value count")
                })?;
                HeapObject::Ref(Some(value))
            }
            BytecodeAggregateKind::Record { nominal, fields } => {
                if fields.len() != values.len() {
                    return Err(VmError::invariant(
                        "record construction has the wrong value count",
                    ));
                }
                HeapObject::Record {
                    nominal: *nominal,
                    fields: fields
                        .iter()
                        .copied()
                        .zip(values.into_iter().map(Some))
                        .collect(),
                }
            }
            BytecodeAggregateKind::Variant { variant, fields } => {
                if fields.len() != values.len() {
                    return Err(VmError::invariant(
                        "variant construction has the wrong value count",
                    ));
                }
                let payload = if fields.is_empty() {
                    AggregatePayload::Unit
                } else if fields.iter().all(Option::is_none) {
                    AggregatePayload::Tuple(values.into_iter().map(Some).collect())
                } else {
                    AggregatePayload::Record(
                        fields
                            .iter()
                            .zip(values)
                            .map(|(field, value)| {
                                Ok((
                                    field.ok_or_else(|| {
                                        VmError::invariant("mixed tuple/record variant payload")
                                    })?,
                                    Some(value),
                                ))
                            })
                            .collect::<Result<_, VmError>>()?,
                    )
                };
                HeapObject::Variant {
                    variant: *variant,
                    payload,
                }
            }
            BytecodeAggregateKind::OptionNone => {
                if !values.is_empty() {
                    return Err(VmError::invariant("none construction has a payload"));
                }
                HeapObject::OptionNone
            }
            BytecodeAggregateKind::OptionSome => {
                let [value] = values.try_into().map_err(|_| {
                    VmError::invariant("some construction has the wrong payload count")
                })?;
                HeapObject::OptionSome(Some(value))
            }
            BytecodeAggregateKind::ResultOk => {
                let [value] = values.try_into().map_err(|_| {
                    VmError::invariant("ok construction has the wrong payload count")
                })?;
                HeapObject::ResultOk(Some(value))
            }
            BytecodeAggregateKind::ResultErr => {
                let [value] = values.try_into().map_err(|_| {
                    VmError::invariant("err construction has the wrong payload count")
                })?;
                HeapObject::ResultErr(Some(value))
            }
        };
        self.allocate(ty, object, &roots)
    }

    fn scalar(&self, ty: BytecodeTypeId) -> Result<BytecodeScalarType, VmError> {
        match self.program.ty(ty).map(|ty| &ty.kind) {
            Some(BytecodeTypeKind::Scalar(scalar)) => Ok(*scalar),
            _ => Err(VmError::invariant("verified scalar type is not scalar")),
        }
    }

    fn pure_prefix(
        &mut self,
        operator: BytecodePrefixOperator,
        ty: BytecodeTypeId,
        value: Value,
    ) -> Result<Value, VmError> {
        match (operator, value) {
            (BytecodePrefixOperator::LogicalNot, Value::Bool(value)) => Ok(Value::Bool(!value)),
            (BytecodePrefixOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
            (BytecodePrefixOperator::BitwiseNot, Value::Integer(value)) => {
                let scalar = self.scalar(ty)?;
                let (minimum, maximum) = integer_bounds(scalar)
                    .ok_or_else(|| VmError::invariant("bitwise operand is not an integer"))?;
                let (_, bits) = integer_shape(scalar).expect("integer bounds have a shape");
                let mask = (1_i128 << bits) - 1;
                let raw = (!value) & mask;
                let normalized = if minimum < 0 && raw > maximum {
                    raw - (1_i128 << bits)
                } else {
                    raw
                };
                Ok(Value::Integer(normalized))
            }
            (BytecodePrefixOperator::BitwiseNot, Value::Byte(value)) => Ok(Value::Byte(!value)),
            _ => Err(VmError::invariant(
                "verified pure prefix operand is invalid",
            )),
        }
    }

    fn pure_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        _right_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Value, VmError> {
        use BytecodeBinaryOperator as Op;
        match operator {
            Op::Equal | Op::NotEqual => {
                let equal = self.value_equal(&left, &right)?;
                Ok(Value::Bool(if operator == Op::Equal {
                    equal
                } else {
                    !equal
                }))
            }
            Op::LogicalAnd | Op::LogicalOr => match (left, right) {
                (Value::Bool(left), Value::Bool(right)) => {
                    Ok(Value::Bool(if operator == Op::LogicalAnd {
                        left && right
                    } else {
                        left || right
                    }))
                }
                _ => Err(VmError::invariant("logical operands are not Bool")),
            },
            Op::Less | Op::LessEqual | Op::Greater | Op::GreaterEqual => {
                let order = self.value_order(&left, &right)?;
                let result = match operator {
                    Op::Less => order == Some(Ordering::Less),
                    Op::LessEqual => matches!(order, Some(Ordering::Less | Ordering::Equal)),
                    Op::Greater => order == Some(Ordering::Greater),
                    Op::GreaterEqual => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
                    _ => unreachable!(),
                };
                Ok(Value::Bool(result))
            }
            Op::Multiply | Op::Divide | Op::Remainder | Op::Add | Op::Subtract => {
                match (left, right) {
                    (Value::Float(left), Value::Float(right)) => {
                        let scalar = self.scalar(left_ty)?;
                        let value = match scalar {
                            BytecodeScalarType::Float32 => {
                                let left = left as f32;
                                let right = right as f32;
                                f64::from(match operator {
                                    Op::Multiply => left * right,
                                    Op::Divide => left / right,
                                    Op::Add => left + right,
                                    Op::Subtract => left - right,
                                    Op::Remainder => {
                                        return Err(VmError::invariant(
                                            "float remainder bypassed bytecode verification",
                                        ));
                                    }
                                    _ => unreachable!(),
                                })
                            }
                            BytecodeScalarType::Float => match operator {
                                Op::Multiply => left * right,
                                Op::Divide => left / right,
                                Op::Add => left + right,
                                Op::Subtract => left - right,
                                Op::Remainder => {
                                    return Err(VmError::invariant(
                                        "float remainder bypassed bytecode verification",
                                    ));
                                }
                                _ => unreachable!(),
                            },
                            _ => {
                                return Err(VmError::invariant(
                                    "float value has a non-float bytecode type",
                                ));
                            }
                        };
                        Ok(Value::Float(value))
                    }
                    _ => Err(VmError::invariant(
                        "non-float arithmetic bypassed checked execution",
                    )),
                }
            }
            Op::BitwiseAnd | Op::BitwiseXor | Op::BitwiseOr => match (left, right) {
                (Value::Integer(left), Value::Integer(right)) => {
                    Ok(Value::Integer(match operator {
                        Op::BitwiseAnd => left & right,
                        Op::BitwiseXor => left ^ right,
                        Op::BitwiseOr => left | right,
                        _ => unreachable!(),
                    }))
                }
                (Value::Byte(left), Value::Byte(right)) => Ok(Value::Byte(match operator {
                    Op::BitwiseAnd => left & right,
                    Op::BitwiseXor => left ^ right,
                    Op::BitwiseOr => left | right,
                    _ => unreachable!(),
                })),
                _ => Err(VmError::invariant("bitwise operands have invalid values")),
            },
            Op::ShiftLeft | Op::ShiftRight => {
                Err(VmError::invariant("shift bypassed checked execution"))
            }
        }
    }

    fn value_equal(&self, left: &Value, right: &Value) -> Result<bool, VmError> {
        let mut pending = vec![(left.clone(), right.clone())];
        let mut visited = std::collections::BTreeSet::new();
        while let Some((left, right)) = pending.pop() {
            match (left, right) {
                (Value::Unit, Value::Unit) => {}
                (Value::Bool(left), Value::Bool(right)) if left == right => {}
                (Value::Integer(left), Value::Integer(right)) if left == right => {}
                (Value::Float(left), Value::Float(right)) if left == right => {}
                (Value::Byte(left), Value::Byte(right)) if left == right => {}
                (Value::Char(left), Value::Char(right)) if left == right => {}
                (
                    Value::Function {
                        callable: left,
                        arguments: left_arguments,
                    },
                    Value::Function {
                        callable: right,
                        arguments: right_arguments,
                    },
                ) if left == right && left_arguments == right_arguments => {}
                (Value::Heap(left), Value::Heap(right)) => {
                    if left == right {
                        continue;
                    }
                    if !visited.insert((left, right)) {
                        continue;
                    }
                    let left_object = self.heap.get(left)?;
                    let right_object = self.heap.get(right)?;
                    match (left_object, right_object) {
                        (HeapObject::Set(left), HeapObject::Set(right)) => {
                            if left.len() != right.len() {
                                return Ok(false);
                            }
                            let mut matched = vec![false; right.len()];
                            for left in left {
                                let left = present(left, "set item")?;
                                let mut found = false;
                                for (index, right) in right.iter().enumerate() {
                                    if !matched[index]
                                        && self.value_equal(left, present(right, "set item")?)?
                                    {
                                        matched[index] = true;
                                        found = true;
                                        break;
                                    }
                                }
                                if !found {
                                    return Ok(false);
                                }
                            }
                            continue;
                        }
                        (HeapObject::Map(left), HeapObject::Map(right)) => {
                            if left.len() != right.len() {
                                return Ok(false);
                            }
                            let mut matched = vec![false; right.len()];
                            for (left_key, left_value) in left {
                                let left_key = present(left_key, "map key")?;
                                let mut found = None;
                                for (index, (right_key, right_value)) in right.iter().enumerate() {
                                    if !matched[index]
                                        && self
                                            .value_equal(left_key, present(right_key, "map key")?)?
                                    {
                                        found = Some((index, right_value));
                                        break;
                                    }
                                }
                                let Some((index, right_value)) = found else {
                                    return Ok(false);
                                };
                                matched[index] = true;
                                if !self.value_equal(
                                    present(left_value, "map value")?,
                                    present(right_value, "map value")?,
                                )? {
                                    return Ok(false);
                                }
                            }
                            continue;
                        }
                        _ => {}
                    }
                    if !queue_object_equality(left_object, right_object, &mut pending)? {
                        return Ok(false);
                    }
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    fn value_order(&self, left: &Value, right: &Value) -> Result<Option<Ordering>, VmError> {
        match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => Ok(Some(left.cmp(right))),
            (Value::Float(left), Value::Float(right)) => Ok(left.partial_cmp(right)),
            (Value::Byte(left), Value::Byte(right)) => Ok(Some(left.cmp(right))),
            (Value::Char(left), Value::Char(right)) => Ok(Some(left.cmp(right))),
            (Value::Heap(left), Value::Heap(right)) => {
                match (self.heap.get(*left)?, self.heap.get(*right)?) {
                    (HeapObject::String(left), HeapObject::String(right)) => {
                        Ok(Some(left.cmp(right)))
                    }
                    _ => Err(VmError::invariant("relational heap values are not strings")),
                }
            }
            _ => Err(VmError::invariant(
                "relational operands have invalid values",
            )),
        }
    }

    fn numeric_conversion(
        &mut self,
        result_ty: BytecodeTypeId,
        target: BytecodeScalarType,
        conversion: BytecodeNumericConversion,
        value: Value,
    ) -> Result<Value, VmError> {
        let converted = convert_numeric(target, &value);
        if conversion == BytecodeNumericConversion::Checked {
            let BytecodeTypeKind::Result {
                error: error_ty, ..
            } = &self
                .program
                .ty(result_ty)
                .ok_or_else(|| VmError::invariant("numeric conversion result type is missing"))?
                .kind
            else {
                return Err(VmError::invariant(
                    "checked numeric conversion does not produce Result",
                ));
            };
            match converted {
                Ok(value) => self.allocate(
                    result_ty,
                    HeapObject::ResultOk(Some(value.clone())),
                    &[value],
                ),
                Err(variant) => {
                    let error = self.allocate(
                        *error_ty,
                        HeapObject::Variant {
                            variant: variant.index(),
                            payload: AggregatePayload::Unit,
                        },
                        &[],
                    )?;
                    self.allocate(
                        result_ty,
                        HeapObject::ResultErr(Some(error.clone())),
                        &[error],
                    )
                }
            }
        } else {
            converted
                .map_err(|_| VmError::invariant("a total numeric conversion failed at runtime"))
        }
    }

    fn contains(
        &self,
        kind: BytecodeContainmentKind,
        item: &Value,
        container: &Value,
    ) -> Result<bool, VmError> {
        let Value::Heap(handle) = container else {
            return Err(VmError::invariant("containment container is not managed"));
        };
        match (kind, self.heap.get(*handle)?) {
            (BytecodeContainmentKind::Array, HeapObject::Array(values))
            | (BytecodeContainmentKind::Set, HeapObject::Set(values)) => {
                values.iter().flatten().try_fold(false, |found, value| {
                    Ok(found || self.value_equal(item, value)?)
                })
            }
            (BytecodeContainmentKind::MapKey, HeapObject::Map(entries)) => entries
                .iter()
                .filter_map(|(key, _)| key.as_ref())
                .try_fold(
                    false,
                    |found, key| Ok(found || self.value_equal(item, key)?),
                ),
            (BytecodeContainmentKind::Range, HeapObject::Range { kind, start, end }) => {
                let start = present(start, "range start")?;
                let end = present(end, "range end")?;
                let lower = self.value_order(item, start)? != Some(Ordering::Less);
                let upper = match kind {
                    BytecodeRangeKind::Exclusive => {
                        self.value_order(item, end)? == Some(Ordering::Less)
                    }
                    BytecodeRangeKind::Inclusive => {
                        self.value_order(item, end)? != Some(Ordering::Greater)
                    }
                };
                Ok(lower && upper)
            }
            (BytecodeContainmentKind::StringChar, HeapObject::String(text)) => {
                let Value::Char(item) = item else {
                    return Err(VmError::invariant("string membership item is not Char"));
                };
                Ok(text.contains(*item))
            }
            _ => Err(VmError::invariant("containment kind and value disagree")),
        }
    }

    fn length(&self, value: &Value) -> Result<usize, VmError> {
        let Value::Heap(handle) = value else {
            return Err(VmError::invariant("length operand is not managed"));
        };
        match self.heap.get(*handle)? {
            HeapObject::String(value) => Ok(value.chars().count()),
            HeapObject::Array(values) | HeapObject::Set(values) => Ok(values.len()),
            HeapObject::Map(entries) => Ok(entries.len()),
            _ => Err(VmError::invariant("length operand has no length")),
        }
    }
}

impl Engine<'_, '_> {
    fn with_task_context<T, E>(
        &mut self,
        task: usize,
        operation: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<VmError>,
    {
        if task == self.current_task {
            return operation(self);
        }
        if task >= self.tasks.len() {
            return Err(E::from(VmError::invariant(
                "a structured loan references an invalid task",
            )));
        }
        let active = self.current_task;
        self.tasks[active].frames = std::mem::take(&mut self.frames);
        self.tasks[active].pending_unwind = self.pending_unwind.take();
        self.frames = std::mem::take(&mut self.tasks[task].frames);
        self.pending_unwind = self.tasks[task].pending_unwind.take();
        self.current_task = task;

        let result = operation(self);

        self.tasks[task].frames = std::mem::take(&mut self.frames);
        self.tasks[task].pending_unwind = self.pending_unwind.take();
        self.current_task = active;
        self.frames = std::mem::take(&mut self.tasks[active].frames);
        self.pending_unwind = self.tasks[active].pending_unwind.take();
        result
    }

    fn read_task_place(
        &mut self,
        task: usize,
        frame: usize,
        place: &BytecodePlace,
    ) -> Result<Value, VmError> {
        self.with_task_context(task, |engine| engine.read_place(frame, place))
    }

    fn validate_task_place(
        &mut self,
        task: usize,
        frame: usize,
        place: &BytecodePlace,
        for_write: bool,
    ) -> Result<ResolvedPlacePath, PlaceFailure> {
        self.with_task_context(task, |engine| {
            engine.validate_place(frame, place, for_write)
        })
    }

    fn read_place(&mut self, frame: usize, place: &BytecodePlace) -> Result<Value, VmError> {
        self.validate_source_regions(frame, place, true)?;
        let mut value = self.read_slot(frame, place.slot)?.clone();
        if let Value::Loan(loan) = value {
            value = self.read_task_place(loan.task, loan.frame, &loan.place)?;
        }
        for projection in &place.projections {
            value = self.read_projection(frame, value, projection)?;
        }
        Ok(value)
    }

    fn take_place(&mut self, frame: usize, place: &BytecodePlace) -> Result<Value, VmError> {
        self.validate_source_regions(frame, place, false)?;
        if self.root_loan(frame, place.slot)?.is_some() {
            return Err(VmError::invariant(
                "a move attempted to transfer borrowed content",
            ));
        }
        let Some((last, prefix)) = place.projections.split_last() else {
            return self.take_slot(frame, place.slot);
        };
        let mut parent = self.read_slot(frame, place.slot)?.clone();
        for projection in prefix {
            parent = self.read_projection(frame, parent, projection)?;
        }
        self.take_projection(frame, parent, last)
    }

    fn write_place(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
        value: Value,
    ) -> Result<(), VmError> {
        let source_mode = self.validate_source_regions(frame, place, false)?;
        if source_mode == Some(BytecodeParameterMode::Mut) && self.array_element(place.ty).is_some()
        {
            self.ensure_mut_array_extent(frame, place, &value)?;
        }
        if self.write_map_iterator_value(frame, place, &value)? {
            return Ok(());
        }
        if let Some(loan) = self.root_loan(frame, place.slot)? {
            if loan.mode == BytecodeParameterMode::Ref {
                return Err(VmError::invariant(
                    "a write attempted to mutate through a shared loan",
                ));
            }
            if place.projections.is_empty() {
                if loan.mode == BytecodeParameterMode::Mut && self.array_element(place.ty).is_some()
                {
                    if loan.task != self.current_task {
                        return Err(VmError::invariant(
                            "an exclusive loan crossed a structured task boundary",
                        ));
                    }
                    self.ensure_mut_array_extent(loan.frame, &loan.place, &value)?;
                }
                if loan.task != self.current_task {
                    return Err(VmError::invariant(
                        "an exclusive loan crossed a structured task boundary",
                    ));
                }
                return self.write_place(loan.frame, &loan.place, value);
            }
            if loan.task != self.current_task {
                return Err(VmError::invariant(
                    "an exclusive loan crossed a structured task boundary",
                ));
            }
            let mut parent = self.read_place(loan.frame, &loan.place)?;
            let (last, prefix) = place
                .projections
                .split_last()
                .expect("the non-empty projection branch has a final projection");
            for projection in prefix {
                parent = self.read_projection(frame, parent, projection)?;
            }
            return self.write_projection(frame, parent, last, value);
        }
        let Some((last, prefix)) = place.projections.split_last() else {
            let state = self.slot_mut(frame, place.slot)?;
            if matches!(state, SlotState::Dead) {
                return Err(VmError::invariant(format!(
                    "write to dead frame slot {}",
                    place.slot.index()
                )));
            }
            *state = SlotState::Value(value);
            return Ok(());
        };
        let mut parent = self.read_slot(frame, place.slot)?.clone();
        for projection in prefix {
            parent = self.read_projection(frame, parent, projection)?;
        }
        self.write_projection(frame, parent, last, value)
    }

    fn write_map_iterator_value(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
        value: &Value,
    ) -> Result<bool, VmError> {
        let Some(position) = place.projections.iter().position(|projection| {
            matches!(
                projection.kind,
                BytecodeProjectionKind::IteratorElement { .. }
            )
        }) else {
            return Ok(false);
        };
        if position + 2 != place.projections.len()
            || !matches!(
                place.projections[position + 1].kind,
                BytecodeProjectionKind::TupleField(1)
            )
        {
            return Ok(false);
        }
        let BytecodeProjectionKind::IteratorElement { index } = place.projections[position].kind
        else {
            unreachable!();
        };
        let base_ty = if position == 0 {
            self.program
                .function(self.frames[frame].function)
                .and_then(|function| function.slots.get(place.slot.index() as usize))
                .map(|slot| slot.ty)
                .ok_or_else(|| VmError::invariant("iterator Map base has an invalid slot"))?
        } else {
            place.projections[position - 1].ty
        };
        let mut base = place.clone();
        base.ty = base_ty;
        base.projections.truncate(position);
        base.source_loan = None;
        let Value::Heap(handle) = self.read_place(frame, &base)? else {
            return Err(VmError::invariant(
                "exclusive Map iterator base is not managed",
            ));
        };
        let mut object = self.heap.get(handle)?.clone();
        let HeapObject::Map(entries) = &mut object else {
            return Ok(false);
        };
        let index = usize::try_from(self.integer_slot(frame, index)?).map_err(|_| {
            VmError::invariant("exclusive Map iterator position is negative or too large")
        })?;
        let entry = entries.get_mut(index).ok_or_else(|| {
            VmError::invariant("exclusive Map iterator position is out of bounds")
        })?;
        entry.1 = Some(value.clone());
        self.replace_object(handle, object, std::slice::from_ref(value))?;
        Ok(true)
    }

    fn ensure_mut_array_extent(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
        replacement: &Value,
    ) -> Result<(), VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(replacement);
        let result = (|| {
            let current = self.read_place(frame, place)?;
            if self.array_length(&current)? != self.array_length(replacement)? {
                return Err(VmError::invariant(
                    "mut Array write changed structural extent",
                ));
            }
            Ok(())
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn read_projection(
        &mut self,
        frame: usize,
        parent: Value,
        projection: &BytecodeProjection,
    ) -> Result<Value, VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(&parent);
        let result = self.read_rooted_projection(frame, parent, projection);
        self.temporary_roots.truncate(marker);
        result
    }

    fn read_rooted_projection(
        &mut self,
        frame: usize,
        parent: Value,
        projection: &BytecodeProjection,
    ) -> Result<Value, VmError> {
        let Value::Heap(handle) = parent else {
            return Err(VmError::invariant("projection base is not a heap object"));
        };
        let object = self.heap.get(handle)?.clone();
        match (&projection.kind, object) {
            (
                BytecodeProjectionKind::ClosureCapture { callable, index },
                HeapObject::Closure {
                    callable: actual,
                    captures,
                },
            ) if *callable == actual => clone_index(&captures, *index, "closure capture"),
            (BytecodeProjectionKind::Field(member), HeapObject::Record { fields, .. }) => {
                clone_field(&fields, *member, "record field")
            }
            (BytecodeProjectionKind::TupleField(index), HeapObject::Tuple(values)) => {
                clone_index(&values, *index, "tuple field")
            }
            (BytecodeProjectionKind::NewtypeValue, HeapObject::Newtype { value, .. }) => {
                present(&value, "newtype value").cloned()
            }
            (BytecodeProjectionKind::RefValue, HeapObject::Ref(value)) => {
                present(&value, "Ref value").cloned()
            }
            (
                BytecodeProjectionKind::VariantTuple { variant, index },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Tuple(values),
                },
            ) if *variant == actual => clone_index(&values, *index, "variant tuple item"),
            (
                BytecodeProjectionKind::VariantField { variant, field },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Record(fields),
                },
            ) if *variant == actual => clone_field(&fields, *field, "variant field"),
            (BytecodeProjectionKind::OptionValue, HeapObject::OptionSome(value)) => {
                present(&value, "option payload").cloned()
            }
            (BytecodeProjectionKind::ResultOkValue, HeapObject::ResultOk(value)) => {
                present(&value, "result success payload").cloned()
            }
            (BytecodeProjectionKind::ResultErrValue, HeapObject::ResultErr(value)) => {
                present(&value, "result error payload").cloned()
            }
            (BytecodeProjectionKind::UnionValue(expected), HeapObject::Union { member, value })
                if *expected == member =>
            {
                present(&value, "union payload").cloned()
            }
            (BytecodeProjectionKind::ArrayPatternIndex(index), HeapObject::Array(values)) => {
                clone_index(&values, *index, "array pattern item")
            }
            (
                BytecodeProjectionKind::ArrayPatternRest { start, suffix },
                HeapObject::Array(values),
            ) => {
                let start = *start as usize;
                let end = values.len().checked_sub(*suffix as usize).ok_or_else(|| {
                    VmError::invariant("array rest projection suffix exceeds length")
                })?;
                if start > end {
                    return Err(VmError::invariant(
                        "array rest projection prefix exceeds remaining length",
                    ));
                }
                let mut output = Vec::with_capacity(end - start);
                for value in &values[start..end] {
                    let value = Some(self.copy_value(present(value, "array rest item")?)?);
                    self.retain_optional_temporary(&value);
                    output.push(value);
                }
                self.allocate(projection.ty, HeapObject::Array(output.into()), &[])
            }
            (
                BytecodeProjectionKind::IteratorElement { index },
                HeapObject::Array(values) | HeapObject::Set(values),
            ) => {
                let index = usize::try_from(self.integer_slot(frame, *index)?).map_err(|_| {
                    VmError::invariant("borrowed iterator position is negative or too large")
                })?;
                present(
                    values.get(index).ok_or_else(|| {
                        VmError::invariant("borrowed iterator position is out of bounds")
                    })?,
                    "borrowed iterator item",
                )
                .cloned()
            }
            (BytecodeProjectionKind::IteratorElement { index }, HeapObject::Map(entries)) => {
                let index = usize::try_from(self.integer_slot(frame, *index)?).map_err(|_| {
                    VmError::invariant("borrowed iterator position is negative or too large")
                })?;
                let (key, value) = entries.get(index).ok_or_else(|| {
                    VmError::invariant("borrowed map iterator position is out of bounds")
                })?;
                let key = present(key, "borrowed map iterator key")?.clone();
                let value = present(value, "borrowed map iterator value")?.clone();
                self.allocate(
                    projection.ty,
                    HeapObject::Tuple(vec![Some(key.clone()), Some(value.clone())]),
                    &[key, value],
                )
            }
            (BytecodeProjectionKind::IteratorSource, HeapObject::Iterator { source, .. }) => {
                present(&source, "iterator source").cloned()
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = self.integer_slot(frame, *index)?;
                let index = normalize_array_index(index, values.len()).ok_or_else(|| {
                    VmError::invariant("unvalidated array index reached a projection")
                })?;
                present(&values[index], "array element").cloned()
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Map(entries)) => {
                let key = self.read_slot(frame, *index)?.clone();
                let found = self.find_map_entry(&entries, &key)?;
                match access {
                    BytecodeIndexAccess::MapLookup => {
                        if let Some(index) = found {
                            let value =
                                self.copy_value(present(&entries[index].1, "map value")?)?;
                            self.allocate(
                                projection.ty,
                                HeapObject::OptionSome(Some(value.clone())),
                                &[value],
                            )
                        } else {
                            self.allocate(projection.ty, HeapObject::OptionNone, &[])
                        }
                    }
                    BytecodeIndexAccess::MapEntry => {
                        let index = found
                            .ok_or_else(|| VmError::invariant("unvalidated map entry is absent"))?;
                        present(&entries[index].1, "map value").cloned()
                    }
                    BytecodeIndexAccess::Array | BytecodeIndexAccess::String => Err(
                        VmError::invariant("non-map index access was applied to a map"),
                    ),
                }
            }
            (BytecodeProjectionKind::Slice { start, end, step }, HeapObject::Array(values)) => {
                let indices = self
                    .slice_indices_from_slots(frame, *start, *end, *step, values.len())
                    .map_err(|_| VmError::invariant("unvalidated slice reached a projection"))?;
                self.copy_array_snapshot(projection.ty, &values, &indices)
            }
            _ => Err(VmError::invariant(
                "verified projection does not match its runtime object",
            )),
        }
    }

    fn take_projection(
        &mut self,
        frame: usize,
        parent: Value,
        projection: &BytecodeProjection,
    ) -> Result<Value, VmError> {
        let Value::Heap(handle) = parent else {
            return Err(VmError::invariant("move projection base is not managed"));
        };
        let mut object = self.heap.get(handle)?.clone();
        let value = match (&projection.kind, &mut object) {
            (
                BytecodeProjectionKind::ClosureCapture { callable, index },
                HeapObject::Closure {
                    callable: actual,
                    captures,
                },
            ) if callable == actual => take_index(captures, *index, "closure capture")?,
            (BytecodeProjectionKind::Field(member), HeapObject::Record { fields, .. }) => {
                take_field(fields, *member, "record field")?
            }
            (BytecodeProjectionKind::TupleField(index), HeapObject::Tuple(values)) => {
                take_index(values, *index, "tuple field")?
            }
            (BytecodeProjectionKind::NewtypeValue, HeapObject::Newtype { value, .. }) => {
                take_option(value, "newtype value")?
            }
            (
                BytecodeProjectionKind::VariantTuple { variant, index },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Tuple(values),
                },
            ) if variant == actual => take_index(values, *index, "variant tuple item")?,
            (
                BytecodeProjectionKind::VariantField { variant, field },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Record(fields),
                },
            ) if variant == actual => take_field(fields, *field, "variant field")?,
            (BytecodeProjectionKind::OptionValue, HeapObject::OptionSome(value)) => {
                take_option(value, "option payload")?
            }
            (BytecodeProjectionKind::ResultOkValue, HeapObject::ResultOk(value)) => {
                take_option(value, "result success payload")?
            }
            (BytecodeProjectionKind::ResultErrValue, HeapObject::ResultErr(value)) => {
                take_option(value, "result error payload")?
            }
            (BytecodeProjectionKind::UnionValue(expected), HeapObject::Union { member, value })
                if expected == member =>
            {
                take_option(value, "union payload")?
            }
            (BytecodeProjectionKind::ArrayPatternIndex(index), HeapObject::Array(values)) => {
                take_index(values, *index, "array pattern item")?
            }
            (
                BytecodeProjectionKind::ArrayPatternRest { start, suffix },
                HeapObject::Array(values),
            ) => {
                let start = *start as usize;
                let end = values.len().checked_sub(*suffix as usize).ok_or_else(|| {
                    VmError::invariant("array rest projection suffix exceeds length")
                })?;
                if start > end {
                    return Err(VmError::invariant(
                        "array rest projection prefix exceeds remaining length",
                    ));
                }
                let mut output = Vec::with_capacity(end - start);
                for value in &mut values[start..end] {
                    output.push(Some(take_option(value, "array rest item")?));
                }
                let mut roots = output.iter().flatten().cloned().collect::<Vec<_>>();
                roots.push(Value::Heap(handle));
                self.allocate(projection.ty, HeapObject::Array(output.into()), &roots)?
            }
            (BytecodeProjectionKind::IteratorSource, HeapObject::Iterator { source, .. }) => {
                take_option(source, "iterator source")?
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = normalize_array_index(self.integer_slot(frame, *index)?, values.len())
                    .ok_or_else(|| VmError::invariant("unvalidated array move index"))?;
                values[index]
                    .take()
                    .ok_or_else(|| VmError::invariant("array element was already moved"))?
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Map(entries))
                if *access == BytecodeIndexAccess::MapEntry =>
            {
                let key = self.read_slot(frame, *index)?.clone();
                let index = self
                    .find_map_entry(entries, &key)?
                    .ok_or_else(|| VmError::invariant("unvalidated map move key"))?;
                entries[index]
                    .1
                    .take()
                    .ok_or_else(|| VmError::invariant("map value was already moved"))?
            }
            _ => {
                return Err(VmError::invariant(
                    "projection cannot be consumed as one stored value",
                ));
            }
        };
        self.replace_object(handle, object, std::slice::from_ref(&value))?;
        Ok(value)
    }

    fn write_projection(
        &mut self,
        frame: usize,
        parent: Value,
        projection: &BytecodeProjection,
        value: Value,
    ) -> Result<(), VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(&parent);
        self.retain_temporary(&value);
        let result = self.write_rooted_projection(frame, parent, projection, value);
        self.temporary_roots.truncate(marker);
        result
    }

    fn write_rooted_projection(
        &mut self,
        frame: usize,
        parent: Value,
        projection: &BytecodeProjection,
        value: Value,
    ) -> Result<(), VmError> {
        let Value::Heap(handle) = parent else {
            return Err(VmError::invariant("write projection base is not managed"));
        };
        let mut object = self.heap.get(handle)?.clone();
        match (&projection.kind, &mut object) {
            (
                BytecodeProjectionKind::ClosureCapture { callable, index },
                HeapObject::Closure {
                    callable: actual,
                    captures,
                },
            ) if callable == actual => {
                set_index(captures, *index, value.clone(), "closure capture")?;
            }
            (BytecodeProjectionKind::Field(member), HeapObject::Record { fields, .. }) => {
                set_field(fields, *member, value.clone())?;
            }
            (BytecodeProjectionKind::TupleField(index), HeapObject::Tuple(values)) => {
                set_index(values, *index, value.clone(), "tuple field")?;
            }
            (BytecodeProjectionKind::NewtypeValue, HeapObject::Newtype { value: slot, .. }) => {
                *slot = Some(value.clone());
            }
            (
                BytecodeProjectionKind::VariantTuple { variant, index },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Tuple(values),
                },
            ) if variant == actual => {
                set_index(values, *index, value.clone(), "variant tuple item")?;
            }
            (
                BytecodeProjectionKind::VariantField { variant, field },
                HeapObject::Variant {
                    variant: actual,
                    payload: AggregatePayload::Record(fields),
                },
            ) if variant == actual => set_field(fields, *field, value.clone())?,
            (BytecodeProjectionKind::OptionValue, HeapObject::OptionSome(slot))
            | (BytecodeProjectionKind::ResultOkValue, HeapObject::ResultOk(slot))
            | (BytecodeProjectionKind::ResultErrValue, HeapObject::ResultErr(slot)) => {
                *slot = Some(value.clone());
            }
            (
                BytecodeProjectionKind::UnionValue(expected),
                HeapObject::Union {
                    member,
                    value: slot,
                },
            ) if expected == member => *slot = Some(value.clone()),
            (BytecodeProjectionKind::ArrayPatternIndex(index), HeapObject::Array(values)) => {
                set_index(values, *index, value.clone(), "array pattern item")?;
            }
            (BytecodeProjectionKind::IteratorElement { index }, HeapObject::Array(values)) => {
                let index = usize::try_from(self.integer_slot(frame, *index)?).map_err(|_| {
                    VmError::invariant("exclusive iterator position is negative or too large")
                })?;
                let slot = values.get_mut(index).ok_or_else(|| {
                    VmError::invariant("exclusive iterator position is out of bounds")
                })?;
                *slot = Some(value.clone());
            }
            (BytecodeProjectionKind::IteratorSource, HeapObject::Iterator { source, .. }) => {
                *source = Some(value.clone());
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = normalize_array_index(self.integer_slot(frame, *index)?, values.len())
                    .ok_or_else(|| VmError::invariant("unvalidated array write index"))?;
                values[index] = Some(value.clone());
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Map(entries))
                if *access == BytecodeIndexAccess::MapEntry =>
            {
                let key = self.read_slot(frame, *index)?.clone();
                if let Some(index) = self.find_map_entry(entries, &key)? {
                    entries[index].1 = Some(value.clone());
                } else {
                    entries.push((Some(self.copy_value(&key)?), Some(value.clone())));
                }
            }
            (BytecodeProjectionKind::Slice { start, end, step }, HeapObject::Array(values)) => {
                let indices = self
                    .slice_indices_from_slots(frame, *start, *end, *step, values.len())
                    .map_err(|_| VmError::invariant("unvalidated slice write"))?;
                let Value::Heap(source) = value.clone() else {
                    return Err(VmError::invariant("slice assignment source is not Array"));
                };
                let HeapObject::Array(replacements) = self.heap.get(source)?.clone() else {
                    return Err(VmError::invariant("slice assignment source is not Array"));
                };
                if indices.len() != replacements.len() {
                    return Err(VmError::invariant(
                        "slice shape mismatch escaped checked assignment validation",
                    ));
                }
                for (index, replacement) in indices.into_iter().zip(replacements) {
                    values[index] = replacement;
                }
            }
            _ => {
                return Err(VmError::invariant(
                    "verified write projection does not match its object",
                ));
            }
        }
        self.replace_object(handle, object, std::slice::from_ref(&value))
    }

    fn validate_places(
        &mut self,
        frame: usize,
        places: &[BytecodePlace],
        replacements: &[Option<BytecodeOperand>],
        against: &[Vec<BytecodeLoanId>],
        for_write: bool,
    ) -> Result<(), PlaceFailure> {
        if places.len() != replacements.len() || places.len() != against.len() {
            return Err(PlaceFailure::Vm(VmError::invariant(
                "place validation inputs are not aligned",
            )));
        }
        let mut paths = Vec::with_capacity(places.len());
        for ((place, replacement), against) in places.iter().zip(replacements).zip(against) {
            let path = self.validate_place(frame, place, for_write)?;
            for loan in against {
                let reservation = self.frames[frame]
                    .loans
                    .get(loan.index() as usize)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        PlaceFailure::Vm(VmError::invariant(
                            "place validation references an inactive reservation",
                        ))
                    })?;
                if paths_overlap(&path, &reservation.path) {
                    return Err(PlaceFailure::Panic(
                        PanicCode::OverlappingBorrow,
                        format!(
                            "place access overlaps active loan#{} at runtime",
                            loan.index()
                        ),
                    ));
                }
            }
            let replacement = match (for_write, replacement) {
                (true, Some(replacement)) => Some(replacement),
                (false, None) => None,
                (true, None) => {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "write validation has no replacement operand",
                    )));
                }
                (false, Some(_)) => {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "read validation has a replacement operand",
                    )));
                }
            };
            if for_write && matches!(path.components.last(), Some(PlaceComponent::Slice(_))) {
                let replacement = replacement.as_ref().ok_or_else(|| {
                    PlaceFailure::Vm(VmError::invariant(
                        "slice write validation has no replacement operand",
                    ))
                })?;
                let value = self
                    .evaluate_operand(frame, replacement)
                    .map_err(PlaceFailure::Vm)?;
                let Value::Heap(handle) = value else {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "slice assignment replacement is not an Array",
                    )));
                };
                let HeapObject::Array(values) = self.heap.get(handle).map_err(PlaceFailure::Vm)?
                else {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "slice assignment replacement is not an Array",
                    )));
                };
                let Some(PlaceComponent::Slice(indices)) = path.components.last() else {
                    unreachable!("the branch established a slice component")
                };
                if indices.len() != values.len() {
                    return Err(PlaceFailure::Panic(
                        PanicCode::ArrayShapeMismatch,
                        format!(
                            "slice assignment has destination length {} and replacement length {}",
                            indices.len(),
                            values.len()
                        ),
                    ));
                }
            }
            paths.push(path);
        }
        for left in 0..paths.len() {
            for right in left + 1..paths.len() {
                if paths_overlap(&paths[left], &paths[right]) {
                    return Err(PlaceFailure::Panic(
                        PanicCode::OverlappingBorrow,
                        "assignment destinations overlap at runtime".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_loan(
        &mut self,
        frame: usize,
        loan: BytecodeLoanId,
        against: &[BytecodeLoanId],
    ) -> Result<(), PlaceFailure> {
        let metadata = self
            .program
            .function(self.frames[frame].function)
            .and_then(|function| function.loans.get(loan.index() as usize))
            .cloned()
            .ok_or_else(|| {
                PlaceFailure::Vm(VmError::invariant(
                    "loan validation references invalid metadata",
                ))
            })?;
        if self.frames[frame]
            .loans
            .get(loan.index() as usize)
            .is_none_or(Option::is_some)
        {
            return Err(PlaceFailure::Vm(VmError::invariant(
                "loan validation reuses an active or invalid reservation",
            )));
        }
        let path = self.validate_place(frame, &metadata.place, false)?;
        for existing in against {
            let reservation = self.frames[frame]
                .loans
                .get(existing.index() as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    PlaceFailure::Vm(VmError::invariant(
                        "loan validation references an inactive reservation",
                    ))
                })?;
            if paths_overlap(&path, &reservation.path) {
                return Err(PlaceFailure::Panic(
                    PanicCode::OverlappingBorrow,
                    format!(
                        "loan#{} overlaps active loan#{} at runtime",
                        loan.index(),
                        existing.index()
                    ),
                ));
            }
        }
        Ok(())
    }

    fn validate_runtime_access(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
        against: &[BytecodeLoanId],
    ) -> Result<(), PlaceFailure> {
        let path = self.validate_place(frame, place, false)?;
        for loan in against {
            let reservation = self.frames[frame]
                .loans
                .get(loan.index() as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    PlaceFailure::Vm(VmError::invariant(
                        "runtime access validation references an inactive reservation",
                    ))
                })?;
            if paths_overlap(&path, &reservation.path) {
                return Err(PlaceFailure::Panic(
                    PanicCode::OverlappingBorrow,
                    format!(
                        "place access overlaps active loan#{} at runtime",
                        loan.index()
                    ),
                ));
            }
        }
        Ok(())
    }

    fn validate_place(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
        for_write: bool,
    ) -> Result<ResolvedPlacePath, PlaceFailure> {
        self.validate_source_regions(frame, place, !for_write)?;
        let root_loan = self.root_loan(frame, place.slot)?;
        let mut path = if let Some(loan) = &root_loan {
            if for_write && loan.mode == BytecodeParameterMode::Ref {
                return Err(PlaceFailure::Vm(VmError::invariant(
                    "a write place is rooted in a shared loan",
                )));
            }
            self.validate_task_place(loan.task, loan.frame, &loan.place, for_write)?
        } else {
            ResolvedPlacePath {
                root: (self.current_task, frame, place.slot.index()),
                components: Vec::with_capacity(place.projections.len()),
            }
        };
        if place.projections.is_empty() {
            if !for_write && root_loan.is_none() {
                self.read_slot(frame, place.slot)?;
            }
            return Ok(path);
        }
        let mut value = if let Some(loan) = root_loan {
            self.read_task_place(loan.task, loan.frame, &loan.place)?
        } else {
            self.read_slot(frame, place.slot)?.clone()
        };
        for (index, projection) in place.projections.iter().enumerate() {
            let allow_missing_map_entry = for_write && index + 1 == place.projections.len();
            let component =
                self.resolve_place_component(frame, &value, projection, allow_missing_map_entry)?;
            path.components.push(component);
            if index + 1 < place.projections.len() {
                value = self
                    .read_projection(frame, value, projection)
                    .map_err(PlaceFailure::Vm)?;
            }
        }
        Ok(path)
    }

    fn resolve_place_component(
        &self,
        frame: usize,
        parent: &Value,
        projection: &BytecodeProjection,
        allow_missing_map_entry: bool,
    ) -> Result<PlaceComponent, PlaceFailure> {
        let Value::Heap(handle) = parent else {
            return Err(PlaceFailure::Vm(VmError::invariant(
                "place projection base is not managed",
            )));
        };
        let object = self.heap.get(*handle)?;
        Ok(match (&projection.kind, object) {
            (
                BytecodeProjectionKind::ClosureCapture { callable, index },
                HeapObject::Closure {
                    callable: actual, ..
                },
            ) if callable == actual => PlaceComponent::Field(*index),
            (BytecodeProjectionKind::Field(field), HeapObject::Record { .. })
            | (BytecodeProjectionKind::VariantField { field, .. }, HeapObject::Variant { .. }) => {
                PlaceComponent::Field(*field)
            }
            (BytecodeProjectionKind::TupleField(index), HeapObject::Tuple(_))
            | (BytecodeProjectionKind::VariantTuple { index, .. }, HeapObject::Variant { .. })
            | (BytecodeProjectionKind::ArrayPatternIndex(index), HeapObject::Array(_)) => {
                PlaceComponent::Index(*index as i128)
            }
            (BytecodeProjectionKind::NewtypeValue, HeapObject::Newtype { .. }) => {
                PlaceComponent::Field(0)
            }
            (BytecodeProjectionKind::RefValue, HeapObject::Ref(_)) => PlaceComponent::Field(0),
            (BytecodeProjectionKind::OptionValue, HeapObject::OptionSome(_)) => {
                PlaceComponent::Variant(1)
            }
            (BytecodeProjectionKind::ResultOkValue, HeapObject::ResultOk(_)) => {
                PlaceComponent::Variant(0)
            }
            (BytecodeProjectionKind::ResultErrValue, HeapObject::ResultErr(_)) => {
                PlaceComponent::Variant(1)
            }
            (
                BytecodeProjectionKind::UnionValue(member),
                HeapObject::Union { member: actual, .. },
            ) if member == actual => PlaceComponent::Variant(member.index()),
            (
                BytecodeProjectionKind::ArrayPatternRest { start, suffix },
                HeapObject::Array(values),
            ) => {
                let end = values.len().checked_sub(*suffix as usize).ok_or_else(|| {
                    PlaceFailure::Vm(VmError::invariant("invalid array rest projection"))
                })?;
                if *start as usize > end {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "invalid array rest projection",
                    )));
                }
                PlaceComponent::Slice((*start as usize..end).collect())
            }
            (
                BytecodeProjectionKind::IteratorElement { index },
                HeapObject::Array(values) | HeapObject::Set(values),
            ) => {
                let index = usize::try_from(self.integer_slot(frame, *index)?).map_err(|_| {
                    PlaceFailure::Vm(VmError::invariant(
                        "borrowed iterator position is negative or too large",
                    ))
                })?;
                if index >= values.len() {
                    return Err(PlaceFailure::Vm(VmError::invariant(
                        "borrowed iterator position is out of bounds",
                    )));
                }
                PlaceComponent::Index(index as i128)
            }
            (BytecodeProjectionKind::IteratorSource, HeapObject::Iterator { .. }) => {
                PlaceComponent::Field(0)
            }
            (BytecodeProjectionKind::IteratorElement { index }, HeapObject::Map(entries)) => {
                let index = usize::try_from(self.integer_slot(frame, *index)?).map_err(|_| {
                    PlaceFailure::Vm(VmError::invariant(
                        "borrowed iterator position is negative or too large",
                    ))
                })?;
                let key = present(
                    &entries
                        .get(index)
                        .ok_or_else(|| {
                            PlaceFailure::Vm(VmError::invariant(
                                "borrowed map iterator position is out of bounds",
                            ))
                        })?
                        .0,
                    "borrowed map iterator key",
                )?;
                PlaceComponent::MapKey(snapshot_value(
                    key,
                    &self.heap,
                    &self.callable_names,
                    &self.nominal_names,
                )?)
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let raw = self.integer_slot(frame, *index)?;
                let index = normalize_array_index(raw, values.len()).ok_or_else(|| {
                    PlaceFailure::Panic(PanicCode::Bounds, "array index is out of bounds".into())
                })?;
                PlaceComponent::Index(index as i128)
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Map(entries))
                if matches!(
                    access,
                    BytecodeIndexAccess::MapLookup | BytecodeIndexAccess::MapEntry
                ) =>
            {
                let key = self.read_slot(frame, *index)?.clone();
                let found = self.find_map_entry(entries, &key)?;
                if *access == BytecodeIndexAccess::MapEntry
                    && found.is_none()
                    && !allow_missing_map_entry
                {
                    return Err(PlaceFailure::Panic(
                        PanicCode::Bounds,
                        "map entry is absent".into(),
                    ));
                }
                PlaceComponent::MapKey(snapshot_value(
                    &key,
                    &self.heap,
                    &self.callable_names,
                    &self.nominal_names,
                )?)
            }
            (BytecodeProjectionKind::Slice { start, end, step }, HeapObject::Array(values)) => {
                PlaceComponent::Slice(
                    self.slice_indices_from_slots(frame, *start, *end, *step, values.len())
                        .map_err(|failure| PlaceFailure::Panic(failure.0, failure.1))?,
                )
            }
            _ => {
                return Err(PlaceFailure::Vm(VmError::invariant(
                    "place validation object and projection disagree",
                )));
            }
        })
    }

    fn integer_slot(
        &self,
        frame: usize,
        slot: crate::bytecode::BytecodeSlotId,
    ) -> Result<i128, VmError> {
        match self.read_slot(frame, slot)? {
            Value::Integer(value) => Ok(*value),
            _ => Err(VmError::invariant("index slot is not Int")),
        }
    }

    fn find_map_entry(
        &self,
        entries: &[(Option<Value>, Option<Value>)],
        key: &Value,
    ) -> Result<Option<usize>, VmError> {
        for (index, (candidate, _)) in entries.iter().enumerate() {
            if self.value_equal(present(candidate, "map key")?, key)? {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn slice_indices_from_slots(
        &self,
        frame: usize,
        start: Option<crate::bytecode::BytecodeSlotId>,
        end: Option<crate::bytecode::BytecodeSlotId>,
        step: Option<crate::bytecode::BytecodeSlotId>,
        length: usize,
    ) -> Result<Vec<usize>, (PanicCode, String)> {
        let bound = |slot: Option<crate::bytecode::BytecodeSlotId>| {
            slot.map(|slot| self.integer_slot(frame, slot))
                .transpose()
                .map_err(|error| (PanicCode::Bounds, error.to_string()))
        };
        slice_indices(bound(start)?, bound(end)?, bound(step)?, length)
    }
}

impl Engine<'_, '_> {
    fn evaluate_deferred_operation(
        &mut self,
        frame: usize,
        deferred: RuntimeDefer,
    ) -> Result<OperationResult, VmError> {
        let RuntimeDefer {
            operation,
            mut guard,
            ..
        } = deferred;
        match operation {
            DeferredOperation::Call { callee, arguments } => {
                let callee = self.take_deferred_value(frame, callee, &mut guard)?;
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push((
                        argument.target,
                        self.take_deferred_value(frame, argument.value, &mut guard)?,
                    ));
                }
                self.prepare_evaluated_call(callee, values)
            }
            DeferredOperation::Assert {
                condition,
                condition_repr,
                message_parts,
            } => {
                let condition = self.take_deferred_value(frame, condition, &mut guard)?;
                let Value::Bool(condition) = condition else {
                    return Err(VmError::invariant("deferred assert condition is not Bool"));
                };
                let mut values = Vec::with_capacity(message_parts.len());
                for (value, spread) in message_parts {
                    values.push((self.take_deferred_value(frame, value, &mut guard)?, spread));
                }
                if condition {
                    Ok(OperationResult::Value(Value::Unit))
                } else {
                    let message = self.assert_message(&condition_repr, &values)?;
                    Ok(OperationResult::Panic(PanicCode::AssertionFailed, message))
                }
            }
            DeferredOperation::BootstrapHostCall {
                function,
                arguments,
            } => {
                let mut values = Vec::with_capacity(arguments.len());
                for argument in arguments {
                    values.push(self.take_deferred_value(frame, argument, &mut guard)?);
                }
                let snapshots = values
                    .iter()
                    .map(|value| {
                        snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let returned = self.host.invoke(function.name(), &snapshots)?;
                if matches!(
                    function,
                    BytecodeBootstrapHostFunction::TestingFailNow
                        | BytecodeBootstrapHostFunction::TestingSkip
                ) {
                    let RuntimeValue::Unit = returned else {
                        return Err(VmError::Host(format!(
                            "{} returned a non-Unit terminal acknowledgement",
                            function.name()
                        )));
                    };
                    return Ok(OperationResult::Panic(
                        PanicCode::ExplicitPanic,
                        format!("{} terminated the test", function.name()),
                    ));
                }
                match (function, returned) {
                    (
                        BytecodeBootstrapHostFunction::ConsolePrint
                        | BytecodeBootstrapHostFunction::ConsolePrintln
                        | BytecodeBootstrapHostFunction::TestingLog
                        | BytecodeBootstrapHostFunction::TestingTags
                        | BytecodeBootstrapHostFunction::TestingFailNow
                        | BytecodeBootstrapHostFunction::TestingSkip
                        | BytecodeBootstrapHostFunction::TestingAttach
                        | BytecodeBootstrapHostFunction::TestingSnapshot,
                        RuntimeValue::Unit,
                    ) => Ok(OperationResult::Value(Value::Unit)),
                    (BytecodeBootstrapHostFunction::ConsolePrint, _) => Err(VmError::Host(
                        "std.console.print returned a non-Unit value".into(),
                    )),
                    (BytecodeBootstrapHostFunction::ConsolePrintln, _) => Err(VmError::Host(
                        "std.console.println returned a non-Unit value".into(),
                    )),
                    (function, _) => Err(VmError::invariant(format!(
                        "non-Unit bootstrap host operation `{}` was registered as defer",
                        function.name()
                    ))),
                }
            }
        }
    }

    fn take_deferred_value(
        &mut self,
        frame: usize,
        value: DeferredValue,
        guard: &mut Option<BytecodePlace>,
    ) -> Result<Value, VmError> {
        match value {
            DeferredValue::Captured(value) => Ok(value),
            DeferredValue::Guard => {
                let place = guard.take().ok_or_else(|| {
                    VmError::invariant("deferred operation consumes its guard more than once")
                })?;
                self.take_place(frame, &place)
            }
        }
    }

    fn assert_message(
        &mut self,
        condition_repr: &str,
        values: &[(Value, bool)],
    ) -> Result<String, VmError> {
        if values.is_empty() {
            return Ok(format!("assertion failed: {condition_repr}"));
        }
        let mut message = String::new();
        for (value, spread) in values {
            if *spread {
                let Value::Heap(handle) = value else {
                    return Err(VmError::invariant("spread assert message is not managed"));
                };
                let HeapObject::Array(parts) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant("spread assert message is not an Array"));
                };
                for part in parts {
                    let part = present(&part, "assert message part")?;
                    message.push_str(self.string_value(part)?);
                }
            } else {
                message.push_str(self.string_value(value)?);
            }
        }
        Ok(message)
    }

    fn evaluate_operation(
        &mut self,
        frame: usize,
        operation: &BytecodeOperation,
        _span: BytecodeSpan,
    ) -> Result<OperationResult, VmError> {
        match &operation.kind {
            BytecodeOperationKind::CheckedPrefix { operator, operand } => {
                let value = self.evaluate_operand(frame, operand)?;
                Ok(match self.checked_prefix(*operator, operand.ty, value)? {
                    Ok(value) => OperationResult::Value(value),
                    Err((code, message)) => OperationResult::Panic(code, message),
                })
            }
            BytecodeOperationKind::CheckedBinary {
                operator,
                left,
                right,
            } => self.with_temporary_roots(|engine| {
                let left_value = engine.evaluate_operand(frame, left)?;
                engine.retain_temporary(&left_value);
                let right_value = engine.evaluate_operand(frame, right)?;
                Ok(
                    match engine.checked_binary(
                        *operator,
                        left.ty,
                        right.ty,
                        operation.ty,
                        left_value,
                        right_value,
                    )? {
                        Ok(value) => OperationResult::Value(value),
                        Err((code, message)) => OperationResult::Panic(code, message),
                    },
                )
            }),
            BytecodeOperationKind::ArraySequence {
                kind,
                array,
                argument,
            } => self.with_temporary_roots(|engine| {
                let array = engine.evaluate_operand(frame, array)?;
                engine.retain_temporary(&array);
                let argument = engine.evaluate_operand(frame, argument)?;
                engine.retain_temporary(&argument);
                Ok(
                    match engine.array_sequence(operation.ty, *kind, array, argument)? {
                        Ok(value) => OperationResult::Value(value),
                        Err((code, message)) => OperationResult::Panic(code, message),
                    },
                )
            }),
            BytecodeOperationKind::BuildMap {
                entries,
                reject_dynamic_duplicates,
            } => self.with_temporary_roots(|engine| {
                let mut evaluated = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    let key = engine.evaluate_operand(frame, key)?;
                    engine.retain_temporary(&key);
                    let value = engine.evaluate_operand(frame, value)?;
                    engine.retain_temporary(&value);
                    evaluated.push((key, value));
                }
                let mut output: Vec<(Option<Value>, Option<Value>)> =
                    Vec::with_capacity(entries.len());
                for (key, value) in evaluated {
                    if let Some(index) = engine.find_map_entry(&output, &key)? {
                        if *reject_dynamic_duplicates {
                            return Ok(OperationResult::Panic(
                                PanicCode::DuplicateDynamicMapKey,
                                "map literal produced a duplicate dynamic key".into(),
                            ));
                        }
                        output[index].1 = Some(value);
                    } else {
                        output.push((Some(key), Some(value)));
                    }
                }
                Ok(OperationResult::Value(engine.allocate(
                    operation.ty,
                    HeapObject::Map(output.into()),
                    &[],
                )?))
            }),
            BytecodeOperationKind::Index {
                base,
                index,
                access,
                against,
            } => {
                if !against.is_empty() {
                    let place = operation_access_place(operation)?
                        .ok_or_else(|| VmError::invariant("index operation has no access place"))?;
                    if let Err(failure) = self.validate_runtime_access(frame, &place, against) {
                        return match failure {
                            PlaceFailure::Panic(code, message) => {
                                Ok(OperationResult::Panic(code, message))
                            }
                            PlaceFailure::Vm(error) => Err(error),
                        };
                    }
                }
                self.with_temporary_roots(|engine| {
                    let base = engine.evaluate_operand(frame, base)?;
                    engine.retain_temporary(&base);
                    let index = engine.evaluate_operand(frame, index)?;
                    engine.retain_temporary(&index);
                    Ok(
                        match engine.index_value(operation.ty, base, index, *access)? {
                            Ok(value) => OperationResult::Value(value),
                            Err((code, message)) => OperationResult::Panic(code, message),
                        },
                    )
                })
            }
            BytecodeOperationKind::Slice {
                base,
                bounds,
                against,
            } => {
                if !against.is_empty() {
                    let place = operation_access_place(operation)?
                        .ok_or_else(|| VmError::invariant("slice operation has no access place"))?;
                    if let Err(failure) = self.validate_runtime_access(frame, &place, against) {
                        return match failure {
                            PlaceFailure::Panic(code, message) => {
                                Ok(OperationResult::Panic(code, message))
                            }
                            PlaceFailure::Vm(error) => Err(error),
                        };
                    }
                }
                self.with_temporary_roots(|engine| {
                    let base = engine.evaluate_operand(frame, base)?;
                    engine.retain_temporary(&base);
                    let start = bounds
                        .start
                        .as_ref()
                        .map(|value| engine.evaluate_operand(frame, value))
                        .transpose()?;
                    engine.retain_optional_temporary(&start);
                    let end = bounds
                        .end
                        .as_ref()
                        .map(|value| engine.evaluate_operand(frame, value))
                        .transpose()?;
                    engine.retain_optional_temporary(&end);
                    let step = bounds
                        .step
                        .as_ref()
                        .map(|value| engine.evaluate_operand(frame, value))
                        .transpose()?;
                    engine.retain_optional_temporary(&step);
                    Ok(
                        match engine.slice_value(operation.ty, base, start, end, step)? {
                            Ok(value) => OperationResult::Value(value),
                            Err((code, message)) => OperationResult::Panic(code, message),
                        },
                    )
                })
            }
            BytecodeOperationKind::Call {
                callee, arguments, ..
            } => {
                let callee = self.evaluate_operand(frame, callee)?;
                self.prepare_call(frame, callee, arguments)
            }
            BytecodeOperationKind::Display { argument } => Ok(OperationResult::Value(
                self.intrinsic_display(frame, operation.ty, argument)?,
            )),
            BytecodeOperationKind::Format { value, display } => {
                self.with_temporary_roots(|engine| {
                    let value_ty = value.ty;
                    let value = engine.evaluate_operand(frame, value)?;
                    engine.retain_temporary(&value);
                    let callback = display
                        .as_ref()
                        .map(|operand| engine.evaluate_operand(frame, operand))
                        .transpose()?;
                    if let Some(callback) = &callback {
                        engine.retain_temporary(callback);
                    }
                    let text = engine.format_display_value_with_type(value, value_ty, callback)?;
                    Ok(OperationResult::Value(
                        engine.format_text_result(operation.ty, [text].into_iter())?,
                    ))
                })
            }
            BytecodeOperationKind::JoinFormat {
                values: values_operand,
                separator,
                display,
            } => self.with_temporary_roots(|engine| {
                let values_ty = values_operand.ty;
                let values = engine.evaluate_operand(frame, values_operand)?;
                engine.retain_temporary(&values);
                let separator = engine.evaluate_operand(frame, separator)?;
                engine.retain_temporary(&separator);
                let callback = display
                    .as_ref()
                    .map(|operand| engine.evaluate_operand(frame, operand))
                    .transpose()?;
                if let Some(callback) = &callback {
                    engine.retain_temporary(callback);
                }
                let element_ty = engine
                    .array_element(values_ty)
                    .ok_or_else(|| VmError::invariant("format.join values are not an Array"))?;
                let separator = engine.string_value(&separator)?.to_owned();
                let elements = engine.array_values(&values)?;
                let mut texts = Vec::with_capacity(elements.len().saturating_mul(2));
                for (index, item) in elements.into_iter().enumerate() {
                    if index != 0 {
                        texts.push(separator.clone());
                    }
                    let item = item.ok_or_else(|| {
                        VmError::invariant("format.join encountered a moved Array element")
                    })?;
                    engine.retain_temporary(&item);
                    texts.push(engine.format_display_value_with_type(
                        item,
                        element_ty,
                        callback.clone(),
                    )?);
                }
                Ok(OperationResult::Value(
                    engine.format_text_result(operation.ty, texts.into_iter())?,
                ))
            }),
            BytecodeOperationKind::ExplicitPanic { message } => {
                let message = self.evaluate_operand(frame, message)?;
                Ok(OperationResult::Panic(
                    PanicCode::ExplicitPanic,
                    self.string_value(&message)?.to_owned(),
                ))
            }
            BytecodeOperationKind::Assert {
                condition,
                condition_repr,
                message_parts,
            } => self.with_temporary_roots(|engine| {
                let condition = engine.evaluate_operand(frame, condition)?;
                let Value::Bool(condition) = condition else {
                    return Err(VmError::invariant("assert condition is not Bool"));
                };
                let mut values = Vec::with_capacity(message_parts.len());
                for part in message_parts {
                    let value = engine.evaluate_operand(frame, &part.value)?;
                    engine.retain_temporary(&value);
                    values.push((value, part.spread));
                }
                if condition {
                    Ok(OperationResult::Value(Value::Unit))
                } else {
                    Ok(OperationResult::Panic(
                        PanicCode::AssertionFailed,
                        engine.assert_message(condition_repr, &values)?,
                    ))
                }
            }),
            BytecodeOperationKind::BootstrapHostCall {
                function,
                arguments,
            } => {
                let values = self.evaluate_operands(frame, arguments)?;
                if matches!(
                    function,
                    BytecodeBootstrapHostFunction::TestingRunLeaf
                        | BytecodeBootstrapHostFunction::TestingRunSuite
                ) {
                    let [id, body] = values.as_slice() else {
                        return Err(VmError::invariant(
                            "test boundary does not have id and body operands",
                        ));
                    };
                    let id = self.string_value(id)?.to_owned();
                    let kind = if matches!(function, BytecodeBootstrapHostFunction::TestingRunLeaf)
                    {
                        VmTestNodeKind::Leaf
                    } else {
                        VmTestNodeKind::Suite
                    };
                    let call = self.prepare_evaluated_call(body.clone(), Vec::new())?;
                    let OperationResult::Call {
                        function,
                        arguments,
                    } = call
                    else {
                        return Err(VmError::invariant(
                            "test boundary body is not a source closure",
                        ));
                    };
                    self.host.begin_test_node(kind, &id)?;
                    return Ok(OperationResult::TestBoundaryCall {
                        function,
                        arguments,
                        boundary: TestBoundary { kind, id },
                    });
                }
                if matches!(
                    function,
                    BytecodeBootstrapHostFunction::TestingBeginSuiteCleanup
                ) {
                    self.host.begin_test_suite_cleanup()?;
                    return Ok(OperationResult::Value(Value::Unit));
                }
                let snapshots = values
                    .iter()
                    .map(|value| {
                        snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let returned = self.host.invoke(function.name(), &snapshots)?;
                if matches!(
                    function,
                    BytecodeBootstrapHostFunction::TestingFailNow
                        | BytecodeBootstrapHostFunction::TestingSkip
                ) {
                    let RuntimeValue::Unit = returned else {
                        return Err(VmError::Host(format!(
                            "{} returned a non-Unit terminal acknowledgement",
                            function.name()
                        )));
                    };
                    return Ok(OperationResult::Panic(
                        PanicCode::ExplicitPanic,
                        format!("{} terminated the test", function.name()),
                    ));
                }
                match (*function, returned) {
                    (
                        BytecodeBootstrapHostFunction::ConsolePrint
                        | BytecodeBootstrapHostFunction::ConsolePrintln,
                        RuntimeValue::Unit,
                    ) => Ok(OperationResult::Value(Value::Unit)),
                    (BytecodeBootstrapHostFunction::ConsolePrint, _) => Err(VmError::Host(
                        "std.console.print returned a non-Unit value".into(),
                    )),
                    (BytecodeBootstrapHostFunction::ConsolePrintln, _) => Err(VmError::Host(
                        "std.console.println returned a non-Unit value".into(),
                    )),
                    (_, returned) => Ok(OperationResult::Value(
                        self.materialize_host_value(operation.ty, returned)?,
                    )),
                }
            }
        }
    }

    fn interpolate(
        &mut self,
        result_ty: BytecodeTypeId,
        segments: &[String],
        values: &[Value],
    ) -> Result<Value, VmError> {
        if segments.len() != values.len() + 1 {
            return Err(VmError::invariant(
                "interpolation segment and value counts disagree",
            ));
        }
        let mut byte_length = segments
            .iter()
            .try_fold(0usize, |total, segment| total.checked_add(segment.len()));
        for value in values {
            let value_length = self.string_value(value)?.len();
            byte_length = byte_length.and_then(|total| total.checked_add(value_length));
        }
        let byte_length = byte_length.ok_or(VmError::ResourceLimit {
            resource: "String interpolation bytes",
            limit: self.limits.max_heap_bytes,
        })?;
        let requested = u64::try_from(byte_length)
            .unwrap_or(u64::MAX)
            .saturating_add(std::mem::size_of::<HeapObject>() as u64);
        if requested > self.limits.max_heap_bytes {
            return Err(VmError::ResourceLimit {
                resource: "String interpolation bytes",
                limit: self.limits.max_heap_bytes,
            });
        }
        let mut output = String::new();
        output
            .try_reserve_exact(byte_length)
            .map_err(|_| VmError::ResourceLimit {
                resource: "String interpolation bytes",
                limit: self.limits.max_heap_bytes,
            })?;
        for (index, value) in values.iter().enumerate() {
            output.push_str(&segments[index]);
            output.push_str(self.string_value(value)?);
        }
        output.push_str(
            segments
                .last()
                .expect("verified interpolation has a trailing segment"),
        );
        if !collection_length_fits_int(output.chars().count()) {
            return Err(VmError::ResourceLimit {
                resource: "String interpolation scalar length",
                limit: i64::MAX as u64,
            });
        }
        self.allocate(result_ty, HeapObject::String(output), values)
    }

    fn format_display_value_with_type(
        &mut self,
        value: Value,
        value_ty: BytecodeTypeId,
        callback: Option<Value>,
    ) -> Result<String, VmError> {
        if let Some(callback) = callback {
            return match self.invoke_sync_value(
                callback,
                vec![(BytecodeCallArgumentTarget::Fixed(0), value)],
            )? {
                Ok(value) => self.string_value(&value).map(str::to_owned),
                Err((code, message)) => Err(VmError::Host(format!(
                    "Display callback panicked ({}): {message}",
                    code.code()
                ))),
            };
        }
        self.display_text(value_ty, value)
    }

    fn format_text_result(
        &mut self,
        result_ty: BytecodeTypeId,
        texts: impl IntoIterator<Item = String>,
    ) -> Result<Value, VmError> {
        let builder = self.host.invoke("std.format.Builder.new", &[])?;
        if !matches!(builder, RuntimeValue::Host { .. }) {
            return Err(VmError::Host(
                "std.format.Builder.new returned a non-builder value".into(),
            ));
        }
        for text in texts {
            let appended = self.host.invoke(
                "std.format.Builder.append",
                &[builder.clone(), RuntimeValue::String(text)],
            )?;
            match appended {
                RuntimeValue::ResultOk(value) if matches!(*value, RuntimeValue::Unit) => {}
                RuntimeValue::ResultErr(error) => {
                    return self.materialize_host_value(result_ty, RuntimeValue::ResultErr(error));
                }
                _ => {
                    return Err(VmError::Host(
                        "std.format.Builder.append returned an invalid result".into(),
                    ));
                }
            }
        }
        let finished = self.host.invoke("std.format.Builder.finish", &[builder])?;
        self.materialize_host_value(result_ty, finished)
    }

    fn intrinsic_display(
        &mut self,
        frame: usize,
        result_ty: BytecodeTypeId,
        argument: &BytecodeCallArgument,
    ) -> Result<Value, VmError> {
        let BytecodeOperandKind::Loan(id) = argument.value.kind else {
            return Err(VmError::invariant(
                "intrinsic Display argument is not a loan",
            ));
        };
        let loan = self
            .program
            .function(self.frames[frame].function)
            .and_then(|function| function.loans.get(id.index() as usize))
            .cloned()
            .ok_or_else(|| VmError::invariant("intrinsic Display loan is invalid"))?;
        if loan.kind != BytecodeLoanKind::CallLocal
            || loan.mode != BytecodeParameterMode::Ref
            || argument.mode != BytecodeParameterMode::Ref
            || argument.target != BytecodeCallArgumentTarget::Receiver
            || loan.place.ty != argument.value.ty
        {
            return Err(VmError::invariant(
                "intrinsic Display loan contract is inconsistent",
            ));
        }
        self.validate_source_regions(frame, &loan.place, true)?;
        let reservation = self.frames[frame]
            .loans
            .get(id.index() as usize)
            .and_then(Option::as_ref)
            .ok_or_else(|| VmError::invariant("intrinsic Display loan is inactive"))?;
        if reservation.mode != BytecodeParameterMode::Ref {
            return Err(VmError::invariant(
                "intrinsic Display reservation is not shared",
            ));
        }
        let value = self.read_place(frame, &loan.place)?;
        self.frames[frame].loans[id.index() as usize] = None;
        let text = self.display_text(argument.value.ty, value)?;
        self.allocate(result_ty, HeapObject::String(text), &[])
    }

    fn display_text(&self, input_ty: BytecodeTypeId, value: Value) -> Result<String, VmError> {
        let mut represented = input_ty;
        let mut remaining = self.program.types.len();
        while let Some(BytecodeTypeKind::OpaqueResult { witness, .. }) =
            self.program.ty(represented).map(|ty| &ty.kind)
        {
            if remaining == 0 {
                return Err(VmError::invariant(
                    "verified Display type contains an opaque cycle",
                ));
            }
            remaining -= 1;
            represented = *witness;
        }

        match self.program.ty(represented).map(|ty| &ty.kind) {
            Some(BytecodeTypeKind::Scalar(BytecodeScalarType::String)) => {
                Ok(self.string_value(&value)?.to_owned())
            }
            Some(BytecodeTypeKind::Scalar(scalar)) => match (*scalar, value) {
                (BytecodeScalarType::Unit, Value::Unit) => Ok("()".to_owned()),
                (BytecodeScalarType::Bool, Value::Bool(value)) => Ok(value.to_string()),
                (
                    BytecodeScalarType::Int
                    | BytecodeScalarType::Int8
                    | BytecodeScalarType::Int16
                    | BytecodeScalarType::Int32
                    | BytecodeScalarType::UInt8
                    | BytecodeScalarType::UInt16
                    | BytecodeScalarType::UInt32
                    | BytecodeScalarType::UInt64,
                    Value::Integer(value),
                ) => Ok(value.to_string()),
                (BytecodeScalarType::Float, Value::Float(value)) => Ok(value.to_string()),
                (BytecodeScalarType::Float32, Value::Float(value)) => {
                    Ok((value as f32).to_string())
                }
                (BytecodeScalarType::Byte, Value::Byte(value)) => Ok(value.to_string()),
                (BytecodeScalarType::Char, Value::Char(value)) => Ok(value.to_string()),
                _ => Err(VmError::invariant(
                    "intrinsic Display value does not match its scalar type",
                )),
            },
            Some(BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Array,
                arguments,
            }) if arguments.len() == 1 => {
                let values = self.array_values(&value)?;
                let mut text = String::from("[");
                for (index, value) in values.into_iter().enumerate() {
                    if index != 0 {
                        text.push_str(", ");
                    }
                    let value = value.ok_or_else(|| {
                        VmError::invariant("intrinsic Display Array contains a moved element")
                    })?;
                    text.push_str(&self.display_text(arguments[0], value)?);
                }
                text.push(']');
                Ok(text)
            }
            _ => Err(VmError::invariant(
                "verified intrinsic Display type is unsupported",
            )),
        }
    }

    fn checked_prefix(
        &mut self,
        operator: BytecodePrefixOperator,
        ty: BytecodeTypeId,
        value: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        match (operator, value) {
            (BytecodePrefixOperator::Negate, Value::Integer(value)) => {
                let scalar = self.scalar(ty)?;
                let (minimum, maximum) = integer_bounds(scalar)
                    .ok_or_else(|| VmError::invariant("checked negate type is not integer"))?;
                Ok(value
                    .checked_neg()
                    .filter(|result| (minimum..=maximum).contains(result))
                    .map(Value::Integer)
                    .ok_or_else(|| {
                        (
                            PanicCode::CheckedOverflow,
                            format!("negation overflows {}", self.type_name(ty)),
                        )
                    }))
            }
            _ => Err(VmError::invariant(
                "verified checked prefix operation is invalid",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn checked_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        result_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let left_element = self.array_element(left_ty);
        let right_element = self.array_element(right_ty);
        if left_element.is_some() || right_element.is_some() {
            return self.checked_array_binary(operator, left_ty, right_ty, result_ty, left, right);
        }
        self.checked_scalar_binary(operator, left_ty, right_ty, left, right)
    }

    fn array_sequence(
        &mut self,
        result_ty: BytecodeTypeId,
        kind: BytecodeArraySequenceKind,
        array: Value,
        argument: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let array_length = self.array_length(&array)?;
        let (copies, argument_values, length) = match kind {
            BytecodeArraySequenceKind::Concat => {
                let argument_length = self.array_length(&argument)?;
                let Some(length) = array_length.checked_add(argument_length) else {
                    return Ok(Err((
                        PanicCode::CheckedOverflow,
                        "Array.concat result length exceeds Int".into(),
                    )));
                };
                if !collection_length_fits_int(length) {
                    return Ok(Err((
                        PanicCode::CheckedOverflow,
                        "Array.concat result length exceeds Int".into(),
                    )));
                }
                (1, Some(self.array_values(&argument)?), length)
            }
            BytecodeArraySequenceKind::Repeat => {
                let Value::Integer(count) = argument else {
                    return Err(VmError::invariant("Array.repeat count is not Int"));
                };
                if count < 0 {
                    return Ok(Err((
                        PanicCode::InvalidRepeatCount,
                        "Array.repeat count cannot be negative".into(),
                    )));
                }
                let Some((_, maximum_length)) = integer_bounds(BytecodeScalarType::Int) else {
                    return Err(VmError::invariant("Int has no integer bounds"));
                };
                let length = i128::try_from(array_length)
                    .ok()
                    .and_then(|length| length.checked_mul(count));
                let Some(length) = length.filter(|length| *length <= maximum_length) else {
                    return Ok(Err((
                        PanicCode::CheckedOverflow,
                        "Array.repeat result length exceeds Int".into(),
                    )));
                };
                let length = usize::try_from(length).map_err(|_| VmError::ResourceLimit {
                    resource: "array allocation bytes",
                    limit: self.limits.max_heap_bytes,
                })?;
                let copies = if array_length == 0 {
                    0
                } else {
                    usize::try_from(count).map_err(|_| VmError::ResourceLimit {
                        resource: "array allocation bytes",
                        limit: self.limits.max_heap_bytes,
                    })?
                };
                (copies, None, length)
            }
        };
        let element_bytes = std::mem::size_of::<Option<Value>>() as u64;
        let requested_bytes = u64::try_from(length)
            .unwrap_or(u64::MAX)
            .saturating_mul(element_bytes)
            .saturating_add(std::mem::size_of::<HeapObject>() as u64);
        if requested_bytes > self.limits.max_heap_bytes {
            return Err(VmError::ResourceLimit {
                resource: "array allocation bytes",
                limit: self.limits.max_heap_bytes,
            });
        }

        let source = self.array_values(&array)?;
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut output = Vec::new();
            output
                .try_reserve_exact(length)
                .map_err(|_| VmError::ResourceLimit {
                    resource: "array allocation bytes",
                    limit: self.limits.max_heap_bytes,
                })?;
            for _ in 0..copies {
                self.copy_array_elements(&source, &mut output)?;
            }
            if let Some(argument) = &argument_values {
                self.copy_array_elements(argument, &mut output)?;
            }
            self.allocate(result_ty, HeapObject::Array(output.into()), &[])
        })();
        self.temporary_roots.truncate(marker);
        result.map(Ok)
    }

    fn copy_array_elements(
        &mut self,
        source: &[Option<Value>],
        output: &mut Vec<Option<Value>>,
    ) -> Result<(), VmError> {
        for value in source {
            let value = self.copy_value(present(value, "Array sequence item")?)?;
            self.retain_temporary(&value);
            output.push(Some(value));
        }
        Ok(())
    }

    fn checked_scalar_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let left_scalar = self.scalar(left_ty)?;
        match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => Ok(self
                .checked_integer_binary(operator, left_scalar, left, right)
                .map(Value::Integer)),
            (Value::Byte(left), Value::Integer(right)) => Ok(self
                .checked_integer_binary(operator, BytecodeScalarType::Byte, i128::from(left), right)
                .and_then(|value| {
                    u8::try_from(value).map(Value::Byte).map_err(|_| {
                        (
                            PanicCode::CheckedOverflow,
                            "Byte arithmetic overflow".into(),
                        )
                    })
                })),
            (Value::Byte(left), Value::Byte(right)) => Ok(self
                .checked_integer_binary(
                    operator,
                    BytecodeScalarType::Byte,
                    i128::from(left),
                    i128::from(right),
                )
                .and_then(|value| {
                    u8::try_from(value).map(Value::Byte).map_err(|_| {
                        (
                            PanicCode::CheckedOverflow,
                            "Byte arithmetic overflow".into(),
                        )
                    })
                })),
            (Value::Float(left), Value::Float(right)) => self
                .pure_binary(
                    operator,
                    left_ty,
                    right_ty,
                    Value::Float(left),
                    Value::Float(right),
                )
                .map(Ok),
            _ => Err(VmError::invariant(
                "verified checked binary values are not numeric",
            )),
        }
    }

    fn checked_integer_binary(
        &self,
        operator: BytecodeBinaryOperator,
        scalar: BytecodeScalarType,
        left: i128,
        right: i128,
    ) -> Result<i128, (PanicCode, String)> {
        use BytecodeBinaryOperator as Op;
        let (minimum, maximum) = if scalar == BytecodeScalarType::Byte {
            (0, 255)
        } else {
            integer_bounds(scalar).ok_or_else(|| {
                (
                    PanicCode::CheckedOverflow,
                    "checked arithmetic type is not an integer".into(),
                )
            })?
        };
        let result = match operator {
            Op::Multiply => left.checked_mul(right),
            Op::Add => left.checked_add(right),
            Op::Subtract => left.checked_sub(right),
            Op::Divide => {
                if right == 0 {
                    return Err((
                        PanicCode::IntegerDivisionByZero,
                        "integer division by zero".into(),
                    ));
                }
                if left == minimum && right == -1 {
                    return Err((
                        PanicCode::CheckedOverflow,
                        "integer division overflows its result type".into(),
                    ));
                }
                left.checked_div(right)
            }
            Op::Remainder => {
                if right == 0 {
                    return Err((
                        PanicCode::IntegerDivisionByZero,
                        "integer remainder by zero".into(),
                    ));
                }
                if left == minimum && right == -1 {
                    Some(0)
                } else {
                    left.checked_rem(right)
                }
            }
            Op::ShiftLeft | Op::ShiftRight => {
                let (signed, bits) = if scalar == BytecodeScalarType::Byte {
                    (false, 8)
                } else {
                    integer_shape(scalar).ok_or_else(|| {
                        (
                            PanicCode::InvalidShiftCount,
                            "shift left operand is not an integer".into(),
                        )
                    })?
                };
                let count = u32::try_from(right)
                    .ok()
                    .filter(|count| *count < bits)
                    .ok_or_else(|| {
                        (
                            PanicCode::InvalidShiftCount,
                            format!("shift count must be between 0 and {}", bits - 1),
                        )
                    })?;
                let mask = (1_u128 << bits) - 1;
                let source = (left as u128) & mask;
                let shifted = if operator == Op::ShiftLeft {
                    (source << count) & mask
                } else if signed {
                    ((left >> count) as u128) & mask
                } else {
                    source >> count
                };
                let sign = 1_u128 << (bits - 1);
                Some(if signed && shifted & sign != 0 {
                    shifted as i128 - (1_i128 << bits)
                } else {
                    shifted as i128
                })
            }
            Op::BitwiseAnd
            | Op::BitwiseXor
            | Op::BitwiseOr
            | Op::Less
            | Op::LessEqual
            | Op::Greater
            | Op::GreaterEqual
            | Op::Equal
            | Op::NotEqual
            | Op::LogicalAnd
            | Op::LogicalOr => None,
        };
        result
            .filter(|result| (minimum..=maximum).contains(result))
            .ok_or_else(|| {
                (
                    PanicCode::CheckedOverflow,
                    "integer arithmetic exceeds its result type".into(),
                )
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn checked_array_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        result_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(&left);
        self.retain_temporary(&right);
        let result = match self
            .validate_rooted_array_binary_shape(left_ty, right_ty, &left, &right)?
        {
            Ok(()) => self
                .checked_rooted_array_binary(operator, left_ty, right_ty, result_ty, left, right),
            Err(panic) => Ok(Err(panic)),
        };
        self.temporary_roots.truncate(marker);
        result
    }

    fn validate_rooted_array_binary_shape(
        &self,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        left: &Value,
        right: &Value,
    ) -> Result<Result<(), (PanicCode, String)>, VmError> {
        let left_element = self.array_element(left_ty);
        let right_element = self.array_element(right_ty);
        if left_element.is_none() && right_element.is_none() {
            return Ok(Ok(()));
        }
        let left_values = left_element.map(|_| self.array_values(left)).transpose()?;
        let right_values = right_element
            .map(|_| self.array_values(right))
            .transpose()?;
        let length = match (&left_values, &right_values) {
            (Some(left), Some(right)) if left.len() != right.len() => {
                return Ok(Err((
                    PanicCode::ArrayShapeMismatch,
                    format!(
                        "array arithmetic requires equal lengths, found {} and {}",
                        left.len(),
                        right.len()
                    ),
                )));
            }
            (Some(left), _) => left.len(),
            (_, Some(right)) => right.len(),
            (None, None) => unreachable!("an Array type was established above"),
        };
        for index in 0..length {
            let left_value = left_values.as_ref().map_or_else(
                || Ok(left.clone()),
                |values| clone_present(&values[index], "array element"),
            )?;
            let right_value = right_values.as_ref().map_or_else(
                || Ok(right.clone()),
                |values| clone_present(&values[index], "array element"),
            )?;
            if let Err(panic) = self.validate_rooted_array_binary_shape(
                left_element.unwrap_or(left_ty),
                right_element.unwrap_or(right_ty),
                &left_value,
                &right_value,
            )? {
                return Ok(Err(panic));
            }
        }
        Ok(Ok(()))
    }

    #[allow(clippy::too_many_arguments)]
    fn checked_rooted_array_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        result_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let left_element = self.array_element(left_ty);
        let right_element = self.array_element(right_ty);
        let result_element = self
            .array_element(result_ty)
            .ok_or_else(|| VmError::invariant("elevated arithmetic result is not an Array"))?;
        let left_values = left_element.map(|_| self.array_values(&left)).transpose()?;
        let right_values = right_element
            .map(|_| self.array_values(&right))
            .transpose()?;
        let length = match (&left_values, &right_values) {
            (Some(left), Some(right)) if left.len() != right.len() => {
                return Err(VmError::invariant(
                    "array shape changed after arithmetic preflight",
                ));
            }
            (Some(left), _) => left.len(),
            (_, Some(right)) => right.len(),
            (None, None) => {
                return Err(VmError::invariant(
                    "elevated arithmetic has no Array operand",
                ));
            }
        };
        let mut output = Vec::with_capacity(length);
        for index in 0..length {
            let left_value = left_values.as_ref().map_or_else(
                || Ok(left.clone()),
                |values| clone_present(&values[index], "array element"),
            );
            let right_value = right_values.as_ref().map_or_else(
                || Ok(right.clone()),
                |values| clone_present(&values[index], "array element"),
            );
            let element = self.checked_rooted_binary(
                operator,
                left_element.unwrap_or(left_ty),
                right_element.unwrap_or(right_ty),
                result_element,
                left_value?,
                right_value?,
            )?;
            match element {
                Ok(value) => {
                    self.retain_temporary(&value);
                    output.push(Some(value));
                }
                Err(panic) => return Ok(Err(panic)),
            }
        }
        Ok(Ok(self.allocate(
            result_ty,
            HeapObject::Array(output.into()),
            &[],
        )?))
    }

    #[allow(clippy::too_many_arguments)]
    fn checked_rooted_binary(
        &mut self,
        operator: BytecodeBinaryOperator,
        left_ty: BytecodeTypeId,
        right_ty: BytecodeTypeId,
        result_ty: BytecodeTypeId,
        left: Value,
        right: Value,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        if self.array_element(left_ty).is_some() || self.array_element(right_ty).is_some() {
            self.checked_rooted_array_binary(operator, left_ty, right_ty, result_ty, left, right)
        } else {
            self.checked_scalar_binary(operator, left_ty, right_ty, left, right)
        }
    }

    fn array_element(&self, ty: BytecodeTypeId) -> Option<BytecodeTypeId> {
        match self.program.ty(ty).map(|ty| &ty.kind) {
            Some(BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Array,
                arguments,
            }) => arguments.first().copied(),
            _ => None,
        }
    }

    fn array_values(&self, value: &Value) -> Result<Vec<Option<Value>>, VmError> {
        let Value::Heap(handle) = value else {
            return Err(VmError::invariant("Array value is not managed"));
        };
        match self.heap.get(*handle)? {
            HeapObject::Array(values) => Ok(values.to_vec()),
            _ => Err(VmError::invariant("Array value has the wrong heap shape")),
        }
    }

    fn array_length(&self, value: &Value) -> Result<usize, VmError> {
        let Value::Heap(handle) = value else {
            return Err(VmError::invariant("Array value is not managed"));
        };
        match self.heap.get(*handle)? {
            HeapObject::Array(values) => Ok(values.len()),
            _ => Err(VmError::invariant("Array value has the wrong heap shape")),
        }
    }

    fn index_value(
        &mut self,
        result_ty: BytecodeTypeId,
        base: Value,
        index: Value,
        access: BytecodeIndexAccess,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let Value::Heap(handle) = base else {
            return Err(VmError::invariant("index base is not managed"));
        };
        match (access, self.heap.get(handle)?.clone()) {
            (BytecodeIndexAccess::Array, HeapObject::Array(values)) => {
                let Value::Integer(index) = index else {
                    return Err(VmError::invariant("array index is not Int"));
                };
                let Some(index) = normalize_array_index(index, values.len()) else {
                    return Ok(Err((
                        PanicCode::Bounds,
                        format!(
                            "array index {index} is out of bounds for length {}",
                            values.len()
                        ),
                    )));
                };
                Ok(Ok(
                    self.copy_value(present(&values[index], "array element")?)?
                ))
            }
            (BytecodeIndexAccess::String, HeapObject::String(text)) => {
                let Value::Integer(index) = index else {
                    return Err(VmError::invariant("String index is not Int"));
                };
                let length = text.chars().count();
                let Some(index) = normalize_array_index(index, length) else {
                    return Ok(Err((
                        PanicCode::Bounds,
                        format!("String index {index} is out of bounds for length {length}"),
                    )));
                };
                let character = text.chars().nth(index).ok_or_else(|| {
                    VmError::invariant("normalized String index has no Unicode scalar")
                })?;
                Ok(Ok(Value::Char(character)))
            }
            (BytecodeIndexAccess::MapLookup, HeapObject::Map(entries)) => {
                if let Some(position) = self.find_map_entry(&entries, &index)? {
                    let value = self.copy_value(present(&entries[position].1, "map value")?)?;
                    Ok(Ok(self.allocate(
                        result_ty,
                        HeapObject::OptionSome(Some(value.clone())),
                        &[value],
                    )?))
                } else {
                    Ok(Ok(self.allocate(result_ty, HeapObject::OptionNone, &[])?))
                }
            }
            (BytecodeIndexAccess::MapEntry, HeapObject::Map(entries)) => {
                let Some(position) = self.find_map_entry(&entries, &index)? else {
                    return Ok(Err((PanicCode::Bounds, "map entry is absent".into())));
                };
                Ok(Ok(
                    self.copy_value(present(&entries[position].1, "map value")?)?
                ))
            }
            _ => Err(VmError::invariant("index access and heap value disagree")),
        }
    }

    fn map_remove(
        &mut self,
        result_ty: BytecodeTypeId,
        map: Value,
        key: &Value,
    ) -> Result<Value, VmError> {
        let Value::Heap(handle) = map else {
            return Err(VmError::invariant("Map.remove receiver is not managed"));
        };
        let HeapObject::Map(mut entries) = self.heap.get(handle)?.clone() else {
            return Err(VmError::invariant(
                "Map.remove receiver has the wrong heap shape",
            ));
        };
        let Some(position) = self.find_map_entry(&entries, key)? else {
            return self.allocate(result_ty, HeapObject::OptionNone, &[]);
        };
        let (_, value) = entries.remove(position);
        let value =
            value.ok_or_else(|| VmError::invariant("removed map value was already absent"))?;
        self.retain_temporary(&value);
        self.replace_object(
            handle,
            HeapObject::Map(entries),
            std::slice::from_ref(&value),
        )?;
        self.allocate(
            result_ty,
            HeapObject::OptionSome(Some(value.clone())),
            &[value],
        )
    }

    fn slice_value(
        &mut self,
        result_ty: BytecodeTypeId,
        base: Value,
        start: Option<Value>,
        end: Option<Value>,
        step: Option<Value>,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let marker = self.temporary_roots.len();
        self.retain_temporary(&base);
        self.retain_optional_temporary(&start);
        self.retain_optional_temporary(&end);
        self.retain_optional_temporary(&step);
        let result = self.slice_rooted_value(result_ty, base, start, end, step);
        self.temporary_roots.truncate(marker);
        result
    }

    fn slice_rooted_value(
        &mut self,
        result_ty: BytecodeTypeId,
        base: Value,
        start: Option<Value>,
        end: Option<Value>,
        step: Option<Value>,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let Value::Heap(handle) = base else {
            return Err(VmError::invariant("slice base is not managed"));
        };
        let object = self.heap.get(handle)?.clone();
        let integer = |value: Option<Value>, label: &str| -> Result<Option<i128>, VmError> {
            value
                .map(|value| match value {
                    Value::Integer(value) => Ok(value),
                    _ => Err(VmError::invariant(format!("slice {label} is not Int"))),
                })
                .transpose()
        };
        let start = integer(start, "start")?;
        let end = integer(end, "end")?;
        let step = integer(step, "step")?;
        match object {
            HeapObject::Array(values) => {
                let indices = match slice_indices(start, end, step, values.len()) {
                    Ok(indices) => indices,
                    Err(panic) => return Ok(Err(panic)),
                };
                Ok(Ok(self.copy_array_snapshot(result_ty, &values, &indices)?))
            }
            HeapObject::String(text) => {
                let characters = text.chars().collect::<Vec<_>>();
                let indices = match slice_indices(start, end, step, characters.len()) {
                    Ok(indices) => indices,
                    Err(panic) => return Ok(Err(panic)),
                };
                let output = indices.into_iter().map(|index| characters[index]).collect();
                Ok(Ok(self.allocate(
                    result_ty,
                    HeapObject::String(output),
                    &[],
                )?))
            }
            _ => Err(VmError::invariant("slice base is not Array or String")),
        }
    }

    fn copy_array_snapshot(
        &mut self,
        result_ty: BytecodeTypeId,
        values: &[Option<Value>],
        indices: &[usize],
    ) -> Result<Value, VmError> {
        let marker = self.temporary_roots.len();
        let result = (|| {
            let mut output = Vec::with_capacity(indices.len());
            for &index in indices {
                let source = values
                    .get(index)
                    .ok_or_else(|| VmError::invariant("normalized slice index is out of bounds"))?;
                let value = self.copy_value(present(source, "slice item")?)?;
                self.retain_temporary(&value);
                output.push(Some(value));
            }
            self.allocate(result_ty, HeapObject::Array(output.into()), &[])
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn prepare_call(
        &mut self,
        frame: usize,
        callee: Value,
        arguments: &[BytecodeCallArgument],
    ) -> Result<OperationResult, VmError> {
        let marker = self.temporary_roots.len();
        self.temporary_roots.push(callee.clone());
        let result = (|| {
            let mut consumed_loans = Vec::new();
            let mut evaluated = Vec::with_capacity(arguments.len());
            for argument in arguments {
                let value = if argument.mode == BytecodeParameterMode::Value {
                    self.evaluate_operand(frame, &argument.value)?
                } else {
                    let BytecodeOperandKind::Loan(id) = &argument.value.kind else {
                        return Err(VmError::invariant(
                            "a borrowed call argument is not a loan operand",
                        ));
                    };
                    let id = *id;
                    if consumed_loans.contains(&id) {
                        return Err(VmError::invariant(
                            "a call consumes the same loan reservation more than once",
                        ));
                    }
                    let loan = {
                        let function = self
                            .program
                            .function(self.frames[frame].function)
                            .ok_or_else(|| {
                                VmError::invariant("call frame has an invalid function")
                            })?;
                        function
                            .loans
                            .get(id.index() as usize)
                            .cloned()
                            .ok_or_else(|| {
                                VmError::invariant("call references an invalid loan operand")
                            })?
                    };
                    if loan.kind != BytecodeLoanKind::CallLocal {
                        return Err(VmError::invariant(
                            "a call attempted to consume a region reservation",
                        ));
                    }
                    self.validate_source_regions(frame, &loan.place, true)?;
                    let reservation = self.frames[frame]
                        .loans
                        .get(id.index() as usize)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| {
                            VmError::invariant("call consumes an inactive loan reservation")
                        })?;
                    if loan.mode != argument.mode || reservation.mode != argument.mode {
                        return Err(VmError::invariant(
                            "call argument mode differs from its loan reservation",
                        ));
                    }
                    consumed_loans.push(id);
                    Value::Loan(RuntimeLoan {
                        task: self.current_task,
                        frame,
                        place: loan.place,
                        mode: loan.mode,
                    })
                };
                self.temporary_roots.push(value.clone());
                evaluated.push((argument.target, value));
            }
            for loan in consumed_loans {
                let reservation = self.frames[frame]
                    .loans
                    .get_mut(loan.index() as usize)
                    .ok_or_else(|| VmError::invariant("consumed loan index became invalid"))?;
                if reservation.take().is_none() {
                    return Err(VmError::invariant(
                        "call loan reservation disappeared before consumption",
                    ));
                }
            }
            self.prepare_evaluated_call(callee, evaluated)
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn prepare_evaluated_call(
        &mut self,
        callee: Value,
        arguments: Vec<(BytecodeCallArgumentTarget, Value)>,
    ) -> Result<OperationResult, VmError> {
        let marker = self.temporary_roots.len();
        self.temporary_roots.push(callee.clone());
        self.temporary_roots
            .extend(arguments.iter().map(|(_, value)| value.clone()));
        let result = (|| {
            let (callable, environment) = match callee {
                Value::Function {
                    callable,
                    arguments: _type_arguments,
                } => (callable, None),
                Value::Heap(handle) => {
                    let HeapObject::Closure { callable, .. } = self.heap.get(handle)? else {
                        return Err(VmError::invariant(
                            "call callee is not a function or closure value",
                        ));
                    };
                    (*callable, Some(Value::Heap(handle)))
                }
                _ => {
                    return Err(VmError::invariant(
                        "call callee is not a function or closure value",
                    ));
                }
            };
            let metadata = self
                .program
                .callable(callable)
                .ok_or_else(|| VmError::invariant("callable metadata index is invalid"))?
                .clone();
            if metadata.closure.is_some() != environment.is_some() {
                return Err(VmError::invariant(
                    "callable kind differs from its runtime value",
                ));
            }
            let mut values = vec![None; metadata.parameters.len()];
            let variadic = metadata
                .parameters
                .iter()
                .position(|parameter| parameter.variadic_element.is_some());
            let receiver = metadata
                .parameters
                .iter()
                .position(|parameter| parameter.receiver);
            let mut variadic_values = Vec::new();
            for (target, value) in arguments {
                match target {
                    BytecodeCallArgumentTarget::Receiver => {
                        let index = receiver.ok_or_else(|| {
                            VmError::invariant("call provides a receiver to a free function")
                        })?;
                        values[index] = Some(value);
                    }
                    BytecodeCallArgumentTarget::Fixed(index) => {
                        let slot = values.get_mut(index as usize).ok_or_else(|| {
                            VmError::invariant("fixed call target index is invalid")
                        })?;
                        *slot = Some(value);
                    }
                    BytecodeCallArgumentTarget::VariadicElement => variadic_values.push(value),
                    BytecodeCallArgumentTarget::VariadicSpread => {
                        let Value::Heap(handle) = value else {
                            return Err(VmError::invariant("variadic spread is not Array"));
                        };
                        let HeapObject::Array(items) = self.heap.get(handle)?.clone() else {
                            return Err(VmError::invariant("variadic spread is not Array"));
                        };
                        for item in items {
                            let value = item.ok_or_else(|| {
                                VmError::invariant("variadic item was already moved")
                            })?;
                            self.temporary_roots.push(value.clone());
                            variadic_values.push(value);
                        }
                    }
                }
            }
            if let Some(index) = variadic {
                values[index] = Some(self.allocate(
                    metadata.parameters[index].ty,
                    HeapObject::Array(variadic_values.into_iter().map(Some).collect()),
                    &[],
                )?);
            }
            let mut values = values
                .into_iter()
                .map(|value| {
                    value.ok_or_else(|| VmError::invariant("call parameter is uninitialized"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(environment) = environment {
                values.insert(0, environment);
            }
            if let Some(function) = metadata.implementation {
                Ok(OperationResult::Call {
                    function,
                    arguments: values,
                })
            } else {
                if metadata.closure.is_some() {
                    return Err(VmError::invariant("closure callable has no implementation"));
                }
                if metadata.name == "std.testing.withVirtualTime"
                    || metadata.name.starts_with("std.testing.withVirtualTime[")
                {
                    let [body] = values.as_slice() else {
                        return Err(VmError::invariant(
                            "virtual-time boundary does not have one body operand",
                        ));
                    };
                    let controller = self.host.begin_virtual_time()?;
                    // This sealed host boundary initializes the callback's `ref`
                    // parameter directly. The bytecode verifier only permits the
                    // opaque controller as that borrowed receiver, so it never
                    // becomes an addressable or escaping Tondo value.
                    let call = self.prepare_evaluated_call(
                        body.clone(),
                        vec![(
                            BytecodeCallArgumentTarget::Fixed(0),
                            Value::Host(controller.clone()),
                        )],
                    );
                    let call = match call {
                        Ok(call) => call,
                        Err(error) => {
                            self.host.finish_virtual_time(&controller)?;
                            return Err(error);
                        }
                    };
                    let OperationResult::Call {
                        function,
                        arguments,
                    } = call
                    else {
                        self.host.finish_virtual_time(&controller)?;
                        return Err(VmError::invariant(
                            "virtual-time body is not a source closure",
                        ));
                    };
                    return Ok(OperationResult::VirtualTimeBoundaryCall {
                        function,
                        arguments,
                        controller,
                    });
                }
                if matches!(
                    metadata.name.as_str(),
                    "std.testing.__runLeaf" | "std.testing.__runSuite"
                ) {
                    let [id, body] = values.as_slice() else {
                        return Err(VmError::invariant(
                            "test boundary callable does not have id and body operands",
                        ));
                    };
                    let id = self.string_value(id)?.to_owned();
                    let kind = if metadata.name == "std.testing.__runLeaf" {
                        VmTestNodeKind::Leaf
                    } else {
                        VmTestNodeKind::Suite
                    };
                    let call = self.prepare_evaluated_call(body.clone(), Vec::new())?;
                    let OperationResult::Call {
                        function,
                        arguments,
                    } = call
                    else {
                        return Err(VmError::invariant(
                            "test boundary body is not a source closure",
                        ));
                    };
                    self.host.begin_test_node(kind, &id)?;
                    return Ok(OperationResult::TestBoundaryCall {
                        function,
                        arguments,
                        boundary: TestBoundary { kind, id },
                    });
                }
                if metadata.name == "std.testing.__beginSuiteCleanup" {
                    if !values.is_empty() {
                        return Err(VmError::invariant(
                            "suite cleanup boundary received arguments",
                        ));
                    }
                    self.host.begin_test_suite_cleanup()?;
                    return Ok(OperationResult::Value(Value::Unit));
                }
                let values = values
                    .into_iter()
                    .map(|value| match value {
                        Value::Loan(loan) => {
                            self.read_task_place(loan.task, loan.frame, &loan.place)
                        }
                        value => Ok(value),
                    })
                    .collect::<Result<Vec<_>, VmError>>()?;
                if metadata.name.starts_with("std.async.oneshot") {
                    if !values.is_empty() {
                        return Err(VmError::invariant(
                            "async.oneshot received unexpected arguments",
                        ));
                    }
                    return self.new_oneshot(metadata.outcome);
                }
                if let Some(result) = self.prepare_oneshot_method(&metadata, &values)? {
                    return Ok(result);
                }
                if metadata.name.starts_with("std.collections.")
                    && let Some(result) = self.prepare_collection_call(&metadata, &values)?
                {
                    return Ok(result);
                }
                if metadata.name.starts_with("std.iter.")
                    && let Some(result) = self.prepare_iterator_call(&metadata, &values)?
                {
                    return Ok(result);
                }
                let snapshots = values
                    .iter()
                    .map(|value| {
                        snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let function_type = self
                    .program
                    .ty(metadata.function_type)
                    .ok_or_else(|| VmError::invariant("host callable has no function type"))?;
                let BytecodeTypeKind::Function(function_type) = &function_type.kind else {
                    return Err(VmError::invariant(
                        "host callable metadata does not reference a function type",
                    ));
                };
                if function_type.is_async {
                    Ok(OperationResult::HostAsync {
                        name: metadata.name,
                        arguments: snapshots,
                        outcome: metadata.outcome,
                    })
                } else {
                    let returned = self.host.invoke(&metadata.name, &snapshots)?;
                    if matches!(
                        metadata.name.as_str(),
                        "std.testing.failNow" | "std.testing.skip"
                    ) {
                        let RuntimeValue::Unit = returned else {
                            return Err(VmError::Host(format!(
                                "{} returned a non-Unit terminal acknowledgement",
                                metadata.name
                            )));
                        };
                        return Ok(OperationResult::Panic(
                            PanicCode::ExplicitPanic,
                            format!("{} terminated the test", metadata.name),
                        ));
                    }
                    Ok(OperationResult::Value(
                        self.materialize_host_value(metadata.outcome, returned)?,
                    ))
                }
            }
        })();
        self.temporary_roots.truncate(marker);
        result
    }

    fn new_oneshot(&mut self, result_ty: BytecodeTypeId) -> Result<OperationResult, VmError> {
        let id = self.next_oneshot_id;
        self.next_oneshot_id =
            self.next_oneshot_id
                .checked_add(1)
                .ok_or(VmError::ResourceLimit {
                    resource: "one-shot handles",
                    limit: u64::MAX,
                })?;
        self.oneshots.insert(id, OneShotState::default());
        let waiter = Value::Host(RuntimeValue::Host {
            kind: RuntimeHostValueKind::Waiter,
            id,
        });
        let completer = Value::Host(RuntimeValue::Host {
            kind: RuntimeHostValueKind::Completer,
            id,
        });
        Ok(OperationResult::Value(self.allocate(
            result_ty,
            HeapObject::Tuple(vec![Some(waiter.clone()), Some(completer.clone())]),
            &[waiter, completer],
        )?))
    }

    fn prepare_oneshot_method(
        &mut self,
        metadata: &BytecodeCallable,
        values: &[Value],
    ) -> Result<Option<OperationResult>, VmError> {
        let name = metadata.name.as_str();
        if !matches!(
            name,
            name if name.starts_with("std.async.Waiter.wait")
                || name.starts_with("std.async.Completer.complete")
                || name.starts_with("std.async.Completer.fail")
                || name.starts_with("std.async.Completer.cancel")
        ) {
            return Ok(None);
        }
        let receiver = metadata
            .parameters
            .iter()
            .position(|parameter| parameter.receiver)
            .ok_or_else(|| VmError::invariant("one-shot method has no receiver"))?;
        let receiver_value = values
            .get(receiver)
            .ok_or_else(|| VmError::invariant("one-shot receiver is missing"))?;
        if name.starts_with("std.async.Waiter.wait") {
            let id = oneshot_handle(receiver_value, RuntimeHostValueKind::Waiter)?;
            let state = self
                .oneshots
                .get_mut(&id)
                .ok_or_else(|| VmError::invariant("waiter references an unknown one-shot"))?;
            if state.waiter_consumed {
                return Err(VmError::invariant("one-shot waiter was consumed twice"));
            }
            state.waiter_consumed = true;
            let completion = state.completion.clone();
            return match completion {
                Some(OneShotCompletion::Ok(value)) => Ok(Some(OperationResult::Value(
                    self.oneshot_result(metadata.outcome, Ok(value))?,
                ))),
                Some(OneShotCompletion::Err(value)) => Ok(Some(OperationResult::Value(
                    self.oneshot_result(metadata.outcome, Err(value))?,
                ))),
                Some(OneShotCompletion::Cancelled) => Ok(Some(OperationResult::OneShotWait {
                    id,
                    outcome: metadata.outcome,
                })),
                None => Ok(Some(OperationResult::OneShotWait {
                    id,
                    outcome: metadata.outcome,
                })),
            };
        }
        let expected_kind = if name.starts_with("std.async.Completer.complete")
            || name.starts_with("std.async.Completer.fail")
            || name.starts_with("std.async.Completer.cancel")
        {
            Some(RuntimeHostValueKind::Completer)
        } else {
            None
        };
        let Some(expected_kind) = expected_kind else {
            return Ok(None);
        };
        let id = oneshot_handle(receiver_value, expected_kind)?;
        let completion = match name {
            name if name.starts_with("std.async.Completer.complete") => {
                let value = values
                    .iter()
                    .enumerate()
                    .find_map(|(index, value)| (index != receiver).then_some(value))
                    .ok_or_else(|| VmError::invariant("one-shot complete value is missing"))?;
                OneShotCompletion::Ok(value.clone())
            }
            name if name.starts_with("std.async.Completer.fail") => {
                let value = values
                    .iter()
                    .enumerate()
                    .find_map(|(index, value)| (index != receiver).then_some(value))
                    .ok_or_else(|| VmError::invariant("one-shot failure value is missing"))?;
                OneShotCompletion::Err(value.clone())
            }
            name if name.starts_with("std.async.Completer.cancel") => OneShotCompletion::Cancelled,
            _ => unreachable!("one-shot completer name was checked"),
        };
        let already_completed = self.complete_oneshot(id, completion);
        let result = match already_completed? {
            true => self.oneshot_result(
                metadata.outcome,
                Err(Value::Host(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::AlreadyCompleted,
                    id: 0,
                })),
            )?,
            false => self.oneshot_result(metadata.outcome, Ok(Value::Unit))?,
        };
        Ok(Some(OperationResult::Value(result)))
    }

    fn complete_oneshot(
        &mut self,
        id: u64,
        completion: OneShotCompletion,
    ) -> Result<bool, VmError> {
        let waiters = {
            let state = self
                .oneshots
                .get_mut(&id)
                .ok_or_else(|| VmError::invariant("completer references an unknown one-shot"))?;
            if state.completion.is_some() {
                return Ok(true);
            }
            state.completion = Some(completion);
            std::mem::take(&mut state.waiter_tasks)
        };
        for task in waiters {
            self.wake_task(task)?;
        }
        Ok(false)
    }

    fn oneshot_result(
        &mut self,
        result_ty: BytecodeTypeId,
        result: Result<Value, Value>,
    ) -> Result<Value, VmError> {
        let kind = self
            .program
            .ty(result_ty)
            .ok_or_else(|| VmError::invariant("one-shot result type is missing"))?
            .kind
            .clone();
        if !matches!(kind, BytecodeTypeKind::Result { .. }) {
            return Err(VmError::invariant(
                "one-shot operation does not return Result",
            ));
        }
        match result {
            Ok(value) => self.allocate(
                result_ty,
                HeapObject::ResultOk(Some(value.clone())),
                &[value],
            ),
            Err(error) => self.allocate(
                result_ty,
                HeapObject::ResultErr(Some(error.clone())),
                &[error],
            ),
        }
    }

    fn collection_error_result(&mut self, result_ty: BytecodeTypeId) -> Result<Value, VmError> {
        self.allocate(
            result_ty,
            HeapObject::ResultErr(Some(Value::Host(RuntimeValue::Host {
                kind: RuntimeHostValueKind::CollectionError,
                id: 0,
            }))),
            &[],
        )
    }

    fn prepare_collection_call(
        &mut self,
        metadata: &BytecodeCallable,
        values: &[Value],
    ) -> Result<Option<OperationResult>, VmError> {
        let receiver_index = metadata
            .parameters
            .iter()
            .position(|parameter| parameter.receiver);
        let Some(receiver_index) = receiver_index else {
            return Ok(None);
        };
        let receiver = values
            .get(receiver_index)
            .ok_or_else(|| VmError::invariant("collection receiver slot is missing"))?;
        let name = metadata
            .name
            .split_once('[')
            .map_or(metadata.name.as_str(), |(base, _)| base);
        let result = match name {
            "std.collections.Array.length" => {
                let Value::Heap(handle) = receiver else {
                    return Err(VmError::invariant("Array.length receiver is not managed"));
                };
                let HeapObject::Array(items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Array.length receiver has the wrong heap shape",
                    ));
                };
                Some(Value::Integer(i128::try_from(items.len()).map_err(
                    |_| VmError::ResourceLimit {
                        resource: "array length",
                        limit: i64::MAX as u64,
                    },
                )?))
            }
            "std.collections.Array.get" => {
                let [Value::Heap(handle), Value::Integer(index)] = values else {
                    return Err(VmError::invariant(
                        "Array.get arguments have the wrong shape",
                    ));
                };
                let HeapObject::Array(items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Array.get receiver has the wrong heap shape",
                    ));
                };
                let position = normalize_array_index(*index, items.len());
                let output = match position {
                    Some(position) => {
                        let value = self.copy_value(present(&items[position], "array element")?)?;
                        self.allocate(
                            metadata.outcome,
                            HeapObject::OptionSome(Some(value.clone())),
                            &[value],
                        )?
                    }
                    None => self.allocate(metadata.outcome, HeapObject::OptionNone, &[])?,
                };
                Some(output)
            }
            "std.collections.Array.slice" => {
                let [
                    Value::Heap(handle),
                    Value::Integer(start),
                    Value::Integer(end),
                ] = values
                else {
                    return Err(VmError::invariant(
                        "Array.slice arguments have the wrong shape",
                    ));
                };
                let HeapObject::Array(items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Array.slice receiver has the wrong heap shape",
                    ));
                };
                let (Ok(start), Ok(end)) = (usize::try_from(*start), usize::try_from(*end)) else {
                    return Ok(Some(OperationResult::Value(
                        self.collection_error_result(metadata.outcome)?,
                    )));
                };
                if start > end || end > items.len() {
                    return Ok(Some(OperationResult::Value(
                        self.collection_error_result(metadata.outcome)?,
                    )));
                }
                let output = self.with_temporary_roots(|engine| {
                    let mut copied = Vec::with_capacity(end - start);
                    for item in &items[start..end] {
                        let value = engine.copy_value(present(item, "array element")?)?;
                        engine.retain_temporary(&value);
                        copied.push(Some(value));
                    }
                    let array =
                        engine.allocate_like(*handle, HeapObject::Array(copied.into()), &[])?;
                    engine.allocate(
                        metadata.outcome,
                        HeapObject::ResultOk(Some(array.clone())),
                        &[array],
                    )
                })?;
                Some(output)
            }
            "std.collections.Array.push" => {
                let [Value::Heap(handle), value] = values else {
                    return Err(VmError::invariant(
                        "Array.push arguments have the wrong shape",
                    ));
                };
                let HeapObject::Array(mut items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Array.push receiver has the wrong heap shape",
                    ));
                };
                if items.try_reserve(1).is_err() {
                    return Ok(Some(OperationResult::Value(
                        self.collection_error_result(metadata.outcome)?,
                    )));
                }
                items.push(Some(value.clone()));
                self.replace_object(
                    *handle,
                    HeapObject::Array(items),
                    std::slice::from_ref(value),
                )?;
                Some(self.allocate(
                    metadata.outcome,
                    HeapObject::ResultOk(Some(Value::Unit)),
                    &[],
                )?)
            }
            "std.collections.Array.pop" => {
                let Value::Heap(handle) = receiver else {
                    return Err(VmError::invariant("Array.pop receiver is not managed"));
                };
                let HeapObject::Array(mut items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Array.pop receiver has the wrong heap shape",
                    ));
                };
                let value = items.pop().flatten();
                let roots = value.iter().cloned().collect::<Vec<_>>();
                self.replace_object(*handle, HeapObject::Array(items), &roots)?;
                let output = match value {
                    Some(value) => self.allocate(
                        metadata.outcome,
                        HeapObject::OptionSome(Some(value.clone())),
                        &[value],
                    )?,
                    None => self.allocate(metadata.outcome, HeapObject::OptionNone, &[])?,
                };
                Some(output)
            }
            "std.collections.Map.insert" => {
                let [Value::Heap(handle), key, value] = values else {
                    return Err(VmError::invariant(
                        "Map.insert arguments have the wrong shape",
                    ));
                };
                let HeapObject::Map(mut entries) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Map.insert receiver has the wrong heap shape",
                    ));
                };
                let old = if let Some(position) = self.find_map_entry(&entries, key)? {
                    let old = entries[position]
                        .1
                        .replace(value.clone())
                        .ok_or_else(|| VmError::invariant("map value was already absent"))?;
                    Some(old)
                } else {
                    entries.try_reserve(1).map_err(|_| VmError::ResourceLimit {
                        resource: "map allocation bytes",
                        limit: self.limits.max_heap_bytes,
                    })?;
                    entries.push((Some(key.clone()), Some(value.clone())));
                    None
                };
                let mut roots = vec![key.clone(), value.clone()];
                if let Some(old) = &old {
                    roots.push(old.clone());
                }
                self.replace_object(*handle, HeapObject::Map(entries), &roots)?;
                let output = match old {
                    Some(old) => self.allocate(
                        metadata.outcome,
                        HeapObject::OptionSome(Some(old.clone())),
                        &[old],
                    )?,
                    None => self.allocate(metadata.outcome, HeapObject::OptionNone, &[])?,
                };
                Some(output)
            }
            "std.collections.Map.get" => {
                let [Value::Heap(handle), key] = values else {
                    return Err(VmError::invariant("Map.get arguments have the wrong shape"));
                };
                let HeapObject::Map(entries) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Map.get receiver has the wrong heap shape",
                    ));
                };
                let output = match self.find_map_entry(&entries, key)? {
                    Some(position) => {
                        let value = self.copy_value(present(&entries[position].1, "map value")?)?;
                        self.allocate(
                            metadata.outcome,
                            HeapObject::OptionSome(Some(value.clone())),
                            &[value],
                        )?
                    }
                    None => self.allocate(metadata.outcome, HeapObject::OptionNone, &[])?,
                };
                Some(output)
            }
            "std.collections.Map.contains" => {
                let [Value::Heap(handle), key] = values else {
                    return Err(VmError::invariant(
                        "Map.contains arguments have the wrong shape",
                    ));
                };
                let HeapObject::Map(entries) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Map.contains receiver has the wrong heap shape",
                    ));
                };
                Some(Value::Bool(self.find_map_entry(&entries, key)?.is_some()))
            }
            "std.collections.Map.remove" => {
                let [Value::Heap(_), key] = values else {
                    return Err(VmError::invariant(
                        "Map.remove arguments have the wrong shape",
                    ));
                };
                Some(self.map_remove(metadata.outcome, receiver.clone(), key)?)
            }
            "std.collections.Map.entries" => Some(self.allocate(
                metadata.outcome,
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(receiver.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(receiver),
            )?),
            "std.collections.Set.insert" => {
                let [Value::Heap(handle), value] = values else {
                    return Err(VmError::invariant(
                        "Set.insert arguments have the wrong shape",
                    ));
                };
                let HeapObject::Set(mut items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Set.insert receiver has the wrong heap shape",
                    ));
                };
                let mut present = false;
                for item in items.iter().flatten() {
                    if self.value_equal(item, value)? {
                        present = true;
                        break;
                    }
                }
                if present {
                    Some(Value::Bool(false))
                } else {
                    items.try_reserve(1).map_err(|_| VmError::ResourceLimit {
                        resource: "set allocation bytes",
                        limit: self.limits.max_heap_bytes,
                    })?;
                    items.push(Some(value.clone()));
                    self.replace_object(
                        *handle,
                        HeapObject::Set(items),
                        std::slice::from_ref(value),
                    )?;
                    Some(Value::Bool(true))
                }
            }
            "std.collections.Set.contains" => {
                let [Value::Heap(handle), value] = values else {
                    return Err(VmError::invariant(
                        "Set.contains arguments have the wrong shape",
                    ));
                };
                let HeapObject::Set(items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Set.contains receiver has the wrong heap shape",
                    ));
                };
                let mut present = false;
                for item in items.iter().flatten() {
                    if self.value_equal(item, value)? {
                        present = true;
                        break;
                    }
                }
                Some(Value::Bool(present))
            }
            "std.collections.Set.remove" => {
                let [Value::Heap(handle), value] = values else {
                    return Err(VmError::invariant(
                        "Set.remove arguments have the wrong shape",
                    ));
                };
                let HeapObject::Set(mut items) = self.heap.get(*handle)?.clone() else {
                    return Err(VmError::invariant(
                        "Set.remove receiver has the wrong heap shape",
                    ));
                };
                let mut position = None;
                for (index, item) in items.iter().enumerate() {
                    if let Some(item) = item
                        && self.value_equal(item, value)?
                    {
                        position = Some(index);
                        break;
                    }
                }
                if let Some(position) = position {
                    let removed = items.remove(position);
                    let roots = removed.clone().into_iter().collect::<Vec<_>>();
                    self.replace_object(*handle, HeapObject::Set(items), &roots)?;
                    Some(Value::Bool(true))
                } else {
                    Some(Value::Bool(false))
                }
            }
            "std.collections.Set.values" => Some(self.allocate(
                metadata.outcome,
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(receiver.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(receiver),
            )?),
            _ => None,
        };
        Ok(result.map(OperationResult::Value))
    }

    fn prepare_iterator_call(
        &mut self,
        metadata: &BytecodeCallable,
        values: &[Value],
    ) -> Result<Option<OperationResult>, VmError> {
        let receiver_index = metadata
            .parameters
            .iter()
            .position(|parameter| parameter.receiver)
            .ok_or_else(|| VmError::invariant("iterator operation has no receiver"))?;
        let receiver = values
            .get(receiver_index)
            .ok_or_else(|| VmError::invariant("iterator receiver slot is missing"))?
            .clone();
        let name = metadata
            .name
            .split_once('[')
            .map_or(metadata.name.as_str(), |(base, _)| base);
        let parameter_after_receiver = metadata
            .parameters
            .iter()
            .enumerate()
            .find(|(index, _)| *index != receiver_index)
            .map(|(index, _)| index);
        let cursor_descriptor = || {
            if matches!(
                self.program.ty(metadata.outcome).map(|ty| &ty.kind),
                Some(BytecodeTypeKind::Cursor {
                    mode: BytecodeCursorMode::Own,
                    ..
                })
            ) {
                Ok(metadata.outcome)
            } else {
                self.program
                    .types
                    .iter()
                    .position(|ty| {
                        matches!(
                            ty.kind,
                            BytecodeTypeKind::Cursor {
                                mode: BytecodeCursorMode::Own,
                                ..
                            }
                        )
                    })
                    .map(|index| BytecodeTypeId::new(index as u32))
                    .ok_or_else(|| VmError::invariant("no owning cursor descriptor is available"))
            }
        };
        let result = match name {
            "std.iter.map" | "std.iter.filter" => {
                let callback_index = parameter_after_receiver
                    .ok_or_else(|| VmError::invariant("iterator callback parameter is missing"))?;
                let callback = values
                    .get(callback_index)
                    .ok_or_else(|| VmError::invariant("iterator callback value is missing"))?
                    .clone();
                let callback_ty = metadata
                    .parameters
                    .get(callback_index)
                    .ok_or_else(|| VmError::invariant("iterator callback type is missing"))?
                    .ty;
                let BytecodeTypeKind::Function(signature) = &self
                    .program
                    .ty(callback_ty)
                    .ok_or_else(|| VmError::invariant("iterator callback type is invalid"))?
                    .kind
                else {
                    return Err(VmError::invariant(
                        "iterator callback parameter is not a function",
                    ));
                };
                let source_item = signature.parameters.first().map(|parameter| parameter.ty);
                let source_item = source_item.ok_or_else(|| {
                    VmError::invariant("iterator callback has no input parameter")
                })?;
                let source = self.normalize_iterator_source(&receiver, cursor_descriptor()?)?;
                let adapter = if name == "std.iter.map" {
                    IteratorAdapter::Map {
                        callback,
                        source_item,
                    }
                } else {
                    IteratorAdapter::Filter {
                        callback,
                        source_item,
                    }
                };
                Some(self.allocate(
                    metadata.outcome,
                    HeapObject::Iterator {
                        mode: BytecodeCursorMode::Own,
                        source: Some(source.clone()),
                        next: 0,
                        adapter: Some(adapter),
                    },
                    &[source, receiver],
                )?)
            }
            "std.iter.take" => {
                let count_index = parameter_after_receiver.ok_or_else(|| {
                    VmError::invariant("iterator take count parameter is missing")
                })?;
                let count = match values.get(count_index) {
                    Some(Value::Integer(value)) => usize::try_from((*value).max(0)).unwrap_or(0),
                    _ => return Err(VmError::invariant("iterator take count is not Int")),
                };
                let source = self.normalize_iterator_source(&receiver, cursor_descriptor()?)?;
                let source_item = self.cursor_array_element_type(metadata.outcome)?;
                Some(self.allocate(
                    metadata.outcome,
                    HeapObject::Iterator {
                        mode: BytecodeCursorMode::Own,
                        source: Some(source.clone()),
                        next: 0,
                        adapter: Some(IteratorAdapter::Take {
                            remaining: count,
                            source_item,
                        }),
                    },
                    &[source, receiver],
                )?)
            }
            "std.iter.collect" => {
                let source = self.normalize_iterator_source(&receiver, cursor_descriptor()?)?;
                let (array_ty, item_ty) = self.result_array_type(metadata.outcome)?;
                let marker = self.temporary_roots.len();
                self.retain_temporary(&source);
                let result = (|| {
                    let mut items = Vec::new();
                    loop {
                        if items.len() >= self.limits.max_heap_objects as usize {
                            return Ok(OperationResult::Value(
                                self.collection_error_result(metadata.outcome)?,
                            ));
                        }
                        match self.next_owned_iterator_value(&source, item_ty)? {
                            Ok(Some(value)) => {
                                self.retain_temporary(&value);
                                items.push(Some(value));
                            }
                            Ok(None) => break,
                            Err((code, message)) => {
                                return Ok(OperationResult::Panic(code, message));
                            }
                        }
                    }
                    let array = self.allocate(array_ty, HeapObject::Array(items.into()), &[])?;
                    Ok(OperationResult::Value(self.allocate(
                        metadata.outcome,
                        HeapObject::ResultOk(Some(array.clone())),
                        &[array],
                    )?))
                })();
                self.temporary_roots.truncate(marker);
                return result.map(Some);
            }
            _ => return Ok(None),
        };
        Ok(result.map(OperationResult::Value))
    }

    fn normalize_iterator_source(
        &mut self,
        source: &Value,
        descriptor: BytecodeTypeId,
    ) -> Result<Value, VmError> {
        if let Value::Heap(handle) = source {
            match self.heap.get(*handle)? {
                HeapObject::Iterator { mode, .. } if *mode == BytecodeCursorMode::Own => {
                    return Ok(source.clone());
                }
                HeapObject::Array(_)
                | HeapObject::Map(_)
                | HeapObject::Set(_)
                | HeapObject::String(_)
                | HeapObject::Range { .. } => {}
                _ => {
                    return Err(VmError::invariant(
                        "iterator source is not an owning iterable",
                    ));
                }
            }
        } else {
            return Err(VmError::invariant(
                "iterator source is not a managed iterable",
            ));
        }
        self.allocate(
            descriptor,
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Own,
                source: Some(source.clone()),
                next: 0,
                adapter: None,
            },
            std::slice::from_ref(source),
        )
    }

    fn cursor_array_element_type(
        &self,
        cursor_ty: BytecodeTypeId,
    ) -> Result<BytecodeTypeId, VmError> {
        let BytecodeTypeKind::Cursor { collection, .. } = &self
            .program
            .ty(cursor_ty)
            .ok_or_else(|| VmError::invariant("iterator result cursor type is missing"))?
            .kind
        else {
            return Err(VmError::invariant("iterator result is not a cursor"));
        };
        let BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Array,
            arguments,
        } = &self
            .program
            .ty(*collection)
            .ok_or_else(|| VmError::invariant("iterator cursor collection type is missing"))?
            .kind
        else {
            return Err(VmError::invariant(
                "iterator cursor collection is not an Array",
            ));
        };
        arguments
            .first()
            .copied()
            .ok_or_else(|| VmError::invariant("iterator cursor array has no element type"))
    }

    fn result_array_type(
        &self,
        result_ty: BytecodeTypeId,
    ) -> Result<(BytecodeTypeId, BytecodeTypeId), VmError> {
        let BytecodeTypeKind::Result { success, .. } = &self
            .program
            .ty(result_ty)
            .ok_or_else(|| VmError::invariant("iterator collect result type is missing"))?
            .kind
        else {
            return Err(VmError::invariant("iterator collect result is not Result"));
        };
        let BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Array,
            arguments,
        } = &self
            .program
            .ty(*success)
            .ok_or_else(|| VmError::invariant("iterator collect array type is missing"))?
            .kind
        else {
            return Err(VmError::invariant("iterator collect result is not Array"));
        };
        let item = arguments
            .first()
            .copied()
            .ok_or_else(|| VmError::invariant("iterator collect array has no element type"))?;
        Ok((*success, item))
    }

    fn materialize_host_value(
        &mut self,
        ty: BytecodeTypeId,
        value: RuntimeValue,
    ) -> Result<Value, VmError> {
        self.materialize_host_value_as(ty, ty, value)
    }

    fn materialize_host_value_as(
        &mut self,
        representation: BytecodeTypeId,
        descriptor: BytecodeTypeId,
        value: RuntimeValue,
    ) -> Result<Value, VmError> {
        let kind = self
            .program
            .ty(representation)
            .ok_or_else(|| VmError::invariant("host result type is missing"))?
            .kind
            .clone();
        match (kind, value) {
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Unit), RuntimeValue::Unit) => {
                Ok(Value::Unit)
            }
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Bool), RuntimeValue::Bool(value)) => {
                Ok(Value::Bool(value))
            }
            (
                BytecodeTypeKind::Scalar(
                    BytecodeScalarType::Int
                    | BytecodeScalarType::Int8
                    | BytecodeScalarType::Int16
                    | BytecodeScalarType::Int32
                    | BytecodeScalarType::UInt8
                    | BytecodeScalarType::UInt16
                    | BytecodeScalarType::UInt32
                    | BytecodeScalarType::UInt64,
                ),
                RuntimeValue::Integer(value),
            ) => Ok(Value::Integer(value)),
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Float), RuntimeValue::Float(value)) => {
                Ok(Value::Float(value))
            }
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Float32), RuntimeValue::Float(value)) => {
                Ok(Value::Float(f64::from(value as f32)))
            }
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Byte), RuntimeValue::Byte(value)) => {
                Ok(Value::Byte(value))
            }
            (BytecodeTypeKind::Scalar(BytecodeScalarType::Char), RuntimeValue::Char(value)) => {
                Ok(Value::Char(value))
            }
            (BytecodeTypeKind::Scalar(BytecodeScalarType::String), RuntimeValue::String(value)) => {
                self.allocate(descriptor, HeapObject::String(value), &[])
            }
            (BytecodeTypeKind::Tuple(fields), RuntimeValue::Tuple(values))
                if fields.len() == values.len() =>
            {
                let values = self.materialize_host_values(&fields, values)?;
                self.allocate(
                    descriptor,
                    HeapObject::Tuple(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            (
                BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Array,
                    arguments,
                },
                RuntimeValue::Array(values),
            ) => {
                let element = arguments
                    .first()
                    .copied()
                    .ok_or_else(|| VmError::invariant("verified array type has no element type"))?;
                self.with_temporary_roots(|engine| {
                    let mut materialized = Vec::with_capacity(values.len());
                    for value in values {
                        let value = engine.materialize_host_value(element, value)?;
                        engine.retain_temporary(&value);
                        materialized.push(Some(value));
                    }
                    engine.allocate(descriptor, HeapObject::Array(materialized.into()), &[])
                })
            }
            (
                BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Map,
                    arguments,
                },
                RuntimeValue::Map(entries),
            ) => {
                let [key_type, value_type] = arguments.as_slice() else {
                    return Err(VmError::invariant("verified map type has the wrong arity"));
                };
                self.with_temporary_roots(|engine| {
                    let mut materialized = Vec::with_capacity(entries.len());
                    for (key, value) in entries {
                        let key = engine.materialize_host_value(*key_type, key)?;
                        engine.retain_temporary(&key);
                        let value = engine.materialize_host_value(*value_type, value)?;
                        engine.retain_temporary(&value);
                        materialized.push((Some(key), Some(value)));
                    }
                    engine.allocate(descriptor, HeapObject::Map(materialized.into()), &[])
                })
            }
            (
                BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Set,
                    arguments,
                },
                RuntimeValue::Set(values),
            ) => {
                let [element_type] = arguments.as_slice() else {
                    return Err(VmError::invariant("verified set type has the wrong arity"));
                };
                self.with_temporary_roots(|engine| {
                    let mut materialized = Vec::with_capacity(values.len());
                    for value in values {
                        let value = engine.materialize_host_value(*element_type, value)?;
                        engine.retain_temporary(&value);
                        materialized.push(Some(value));
                    }
                    engine.allocate(descriptor, HeapObject::Set(materialized.into()), &[])
                })
            }
            (BytecodeTypeKind::Intrinsic { constructor, .. }, RuntimeValue::Host { kind, id })
                if runtime_host_kind(constructor) == Some(kind) =>
            {
                Ok(Value::Host(RuntimeValue::Host { kind, id }))
            }
            (
                BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Duration,
                    ..
                },
                RuntimeValue::Integer(value),
            ) if (i64::MIN as i128..=i64::MAX as i128).contains(&value) => {
                Ok(Value::Integer(value))
            }
            (BytecodeTypeKind::Option(_), RuntimeValue::OptionNone) => {
                self.allocate(descriptor, HeapObject::OptionNone, &[])
            }
            (BytecodeTypeKind::Option(item), RuntimeValue::OptionSome(value)) => {
                let value = self.materialize_host_value(item, *value)?;
                self.allocate(
                    descriptor,
                    HeapObject::OptionSome(Some(value.clone())),
                    &[value],
                )
            }
            (BytecodeTypeKind::Result { success, .. }, RuntimeValue::ResultOk(value)) => {
                let value = self.materialize_host_value(success, *value)?;
                self.allocate(
                    descriptor,
                    HeapObject::ResultOk(Some(value.clone())),
                    &[value],
                )
            }
            (BytecodeTypeKind::Result { error, .. }, RuntimeValue::ResultErr(value)) => {
                let value = self.materialize_host_value(error, *value)?;
                self.allocate(
                    descriptor,
                    HeapObject::ResultErr(Some(value.clone())),
                    &[value],
                )
            }
            (BytecodeTypeKind::Union(members), RuntimeValue::Union { member, value }) => {
                let member = members
                    .into_iter()
                    .find(|candidate| candidate.index() == member)
                    .ok_or_else(|| {
                        VmError::Host(
                            "bootstrap host selected a value outside the return union".into(),
                        )
                    })?;
                let value = self.materialize_host_value(member, *value)?;
                self.allocate(
                    descriptor,
                    HeapObject::Union {
                        member,
                        value: Some(value.clone()),
                    },
                    &[value],
                )
            }
            (
                BytecodeTypeKind::Union(members),
                RuntimeValue::Host {
                    kind: host_kind,
                    id,
                },
            ) => {
                let member = members
                    .into_iter()
                    .find(|member| {
                        self.program.ty(*member).and_then(|ty| match ty.kind {
                            BytecodeTypeKind::Intrinsic { constructor, .. } => {
                                runtime_host_kind(constructor)
                            }
                            _ => None,
                        }) == Some(host_kind)
                    })
                    .ok_or_else(|| {
                        VmError::Host(
                            "bootstrap host value does not belong to the return union".into(),
                        )
                    })?;
                let value = Value::Host(RuntimeValue::Host {
                    kind: host_kind,
                    id,
                });
                self.allocate(
                    descriptor,
                    HeapObject::Union {
                        member,
                        value: Some(value.clone()),
                    },
                    &[value],
                )
            }
            (BytecodeTypeKind::OpaqueResult { witness, .. }, value) => {
                self.materialize_host_value_as(witness, descriptor, value)
            }
            _ => Err(VmError::Host(
                "bootstrap host result does not match its verified return type".into(),
            )),
        }
    }

    fn materialize_host_values(
        &mut self,
        types: &[BytecodeTypeId],
        values: Vec<RuntimeValue>,
    ) -> Result<Vec<Value>, VmError> {
        if types.len() != values.len() {
            return Err(VmError::Host(
                "bootstrap host result has the wrong aggregate arity".into(),
            ));
        }
        self.with_temporary_roots(|engine| {
            let mut materialized = Vec::with_capacity(values.len());
            for (ty, value) in types.iter().copied().zip(values) {
                let value = engine.materialize_host_value(ty, value)?;
                engine.retain_temporary(&value);
                materialized.push(value);
            }
            Ok(materialized)
        })
    }

    fn value_tag(&self, value: &Value) -> Result<BytecodeTag, VmError> {
        let Value::Heap(handle) = value else {
            return Err(VmError::invariant("tagged value is not managed"));
        };
        match self.heap.get(*handle)? {
            HeapObject::OptionNone => Ok(BytecodeTag::OptionNone),
            HeapObject::OptionSome(_) => Ok(BytecodeTag::OptionSome),
            HeapObject::ResultOk(_) => Ok(BytecodeTag::ResultOk),
            HeapObject::ResultErr(_) => Ok(BytecodeTag::ResultErr),
            HeapObject::Variant { variant, .. } => Ok(BytecodeTag::Variant(*variant)),
            HeapObject::Union { member, .. } => Ok(BytecodeTag::Union(*member)),
            _ => Err(VmError::invariant("value has no discriminant tag")),
        }
    }

    fn iterator_next(
        &mut self,
        frame: usize,
        state: &BytecodePlace,
        borrowed_source: Option<&BytecodePlace>,
        item_ty: BytecodeTypeId,
        _span: BytecodeSpan,
    ) -> Result<Result<Option<IteratorStep>, (PanicCode, String)>, VmError> {
        let iterator = self.read_place(frame, state)?;
        let Value::Heap(handle) = iterator else {
            return Err(VmError::invariant("iterator state is not managed"));
        };
        let HeapObject::Iterator {
            mode,
            source,
            next,
            adapter,
        } = self.heap.get(handle)?.clone()
        else {
            return Err(VmError::invariant(
                "iterator state has the wrong heap shape",
            ));
        };
        if next == usize::MAX {
            return Ok(Ok(None));
        }
        let source = present(&source, "iterator source")?.clone();
        if let Some(adapter) = adapter {
            if mode != BytecodeCursorMode::Own || borrowed_source.is_some() {
                return Err(VmError::invariant(
                    "lazy iterator adapter is not an owning cursor",
                ));
            }
            let marker = self.temporary_roots.len();
            self.retain_temporary(&source);
            let result: Result<Option<IteratorStep>, VmError> = (|| {
                let (item, adapter, next_index) =
                    self.iterator_adapter_next(&source, adapter, item_ty)?;
                let mut roots = vec![source.clone()];
                if let Some(IteratorStep::Value(value)) = &item {
                    roots.push(value.clone());
                }
                self.replace_object(
                    handle,
                    HeapObject::Iterator {
                        mode,
                        source: Some(source),
                        next: next_index,
                        adapter: Some(adapter),
                    },
                    &roots,
                )?;
                Ok(item)
            })();
            self.temporary_roots.truncate(marker);
            return result.map(Ok);
        }
        let (item, next_index) = match mode {
            BytecodeCursorMode::Own => {
                if borrowed_source.is_some() {
                    return Err(VmError::invariant(
                        "owning iterator received a borrowed source",
                    ));
                }
                let (item, next_index) = self.iterator_item(&source, item_ty, next)?;
                (item.map(IteratorStep::Value), next_index)
            }
            BytecodeCursorMode::Ref | BytecodeCursorMode::Mut => {
                let borrowed_source = borrowed_source
                    .ok_or_else(|| VmError::invariant("borrowed iterator has no source place"))?;
                if self.read_place(frame, borrowed_source)? != source {
                    return Err(VmError::invariant(
                        "borrowed iterator source differs from its cursor",
                    ));
                }
                let has_item = self.borrowed_iterator_has_item(&source, next)?;
                (
                    has_item.then_some(IteratorStep::Position(next)),
                    if has_item {
                        next.saturating_add(1)
                    } else {
                        usize::MAX
                    },
                )
            }
        };
        let mut roots = vec![source.clone()];
        if let Some(IteratorStep::Value(value)) = &item {
            roots.push(value.clone());
        }
        self.replace_object(
            handle,
            HeapObject::Iterator {
                mode,
                source: Some(source),
                next: next_index,
                adapter: None,
            },
            &roots,
        )?;
        Ok(Ok(item))
    }

    /// Consume an owning cursor nested inside a lazy adapter.  Nested cursors
    /// are advanced in place, so adapter chains never allocate an intermediate
    /// array and retain the observable one-shot consumption contract.
    fn next_owned_iterator_value(
        &mut self,
        iterator: &Value,
        item_ty: BytecodeTypeId,
    ) -> Result<Result<Option<Value>, (PanicCode, String)>, VmError> {
        let Value::Heap(handle) = iterator else {
            return Err(VmError::invariant(
                "lazy iterator source is not a managed cursor",
            ));
        };
        let HeapObject::Iterator {
            mode,
            source,
            next,
            adapter,
        } = self.heap.get(*handle)?.clone()
        else {
            return Err(VmError::invariant(
                "lazy iterator source has the wrong heap shape",
            ));
        };
        if mode != BytecodeCursorMode::Own {
            return Err(VmError::invariant(
                "lazy iterator source is not an owning cursor",
            ));
        }
        if next == usize::MAX {
            return Ok(Ok(None));
        }
        let source = present(&source, "iterator source")?.clone();
        let marker = self.temporary_roots.len();
        self.retain_temporary(iterator);
        let result: Result<Option<Value>, VmError> = (|| {
            if let Some(adapter) = adapter {
                let (item, adapter, next_index) =
                    self.iterator_adapter_next(&source, adapter, item_ty)?;
                let mut roots = vec![source.clone()];
                if let Some(IteratorStep::Value(value)) = &item {
                    roots.push(value.clone());
                }
                self.replace_object(
                    *handle,
                    HeapObject::Iterator {
                        mode,
                        source: Some(source),
                        next: next_index,
                        adapter: Some(adapter),
                    },
                    &roots,
                )?;
                return Ok(item.map(|step| match step {
                    IteratorStep::Value(value) => value,
                    IteratorStep::Position(_) => {
                        unreachable!("nested owning iterator adapter produced a borrowed position")
                    }
                }));
            }
            let (item, next_index) = self.iterator_item(&source, item_ty, next)?;
            let mut roots = vec![source.clone()];
            if let Some(value) = &item {
                roots.push(value.clone());
            }
            self.replace_object(
                *handle,
                HeapObject::Iterator {
                    mode,
                    source: Some(source),
                    next: next_index,
                    adapter: None,
                },
                &roots,
            )?;
            Ok(item)
        })();
        self.temporary_roots.truncate(marker);
        result.map(Ok)
    }

    fn iterator_adapter_next(
        &mut self,
        source: &Value,
        adapter: IteratorAdapter,
        _output_ty: BytecodeTypeId,
    ) -> Result<(Option<IteratorStep>, IteratorAdapter, usize), VmError> {
        match adapter {
            IteratorAdapter::Map {
                callback,
                source_item,
            } => {
                let marker = self.temporary_roots.len();
                self.retain_temporary(source);
                self.retain_temporary(&callback);
                let result = (|| match self.next_owned_iterator_value(source, source_item)? {
                    Ok(Some(value)) => match self.invoke_sync_value(
                        callback.clone(),
                        vec![(BytecodeCallArgumentTarget::Fixed(0), value)],
                    )? {
                        Ok(value) => Ok((
                            Some(IteratorStep::Value(value)),
                            IteratorAdapter::Map {
                                callback,
                                source_item,
                            },
                            0,
                        )),
                        Err(panic) => Err(VmError::invariant(format!(
                            "iterator map callback panicked: {}",
                            panic.1
                        ))),
                    },
                    Ok(None) => Ok((
                        None,
                        IteratorAdapter::Map {
                            callback,
                            source_item,
                        },
                        usize::MAX,
                    )),
                    Err(panic) => Err(VmError::invariant(format!(
                        "iterator source panicked: {}",
                        panic.1
                    ))),
                })();
                self.temporary_roots.truncate(marker);
                result
            }
            IteratorAdapter::Filter {
                callback,
                source_item,
            } => {
                let marker = self.temporary_roots.len();
                self.retain_temporary(source);
                self.retain_temporary(&callback);
                let result = (|| loop {
                    match self.next_owned_iterator_value(source, source_item)? {
                        Ok(Some(value)) => match self.invoke_sync_value(
                            callback.clone(),
                            vec![(BytecodeCallArgumentTarget::Fixed(0), value.clone())],
                        )? {
                            Ok(Value::Bool(true)) => {
                                break Ok((
                                    Some(IteratorStep::Value(value)),
                                    IteratorAdapter::Filter {
                                        callback,
                                        source_item,
                                    },
                                    0,
                                ));
                            }
                            Ok(Value::Bool(false)) => continue,
                            Ok(_) => {
                                break Err(VmError::invariant(
                                    "iterator filter callback did not return Bool",
                                ));
                            }
                            Err(panic) => {
                                break Err(VmError::invariant(format!(
                                    "iterator filter callback panicked: {}",
                                    panic.1
                                )));
                            }
                        },
                        Ok(None) => {
                            break Ok((
                                None,
                                IteratorAdapter::Filter {
                                    callback,
                                    source_item,
                                },
                                usize::MAX,
                            ));
                        }
                        Err(panic) => {
                            break Err(VmError::invariant(format!(
                                "iterator source panicked: {}",
                                panic.1
                            )));
                        }
                    }
                })();
                self.temporary_roots.truncate(marker);
                result
            }
            IteratorAdapter::Take {
                remaining,
                source_item,
            } => {
                if remaining == 0 {
                    return Ok((
                        None,
                        IteratorAdapter::Take {
                            remaining,
                            source_item,
                        },
                        usize::MAX,
                    ));
                }
                let marker = self.temporary_roots.len();
                self.retain_temporary(source);
                let result = (|| match self.next_owned_iterator_value(source, source_item)? {
                    Ok(Some(value)) => Ok((
                        Some(IteratorStep::Value(value)),
                        IteratorAdapter::Take {
                            remaining: remaining - 1,
                            source_item,
                        },
                        0,
                    )),
                    Ok(None) => Ok((
                        None,
                        IteratorAdapter::Take {
                            remaining: 0,
                            source_item,
                        },
                        usize::MAX,
                    )),
                    Err(panic) => Err(VmError::invariant(format!(
                        "iterator source panicked: {}",
                        panic.1
                    ))),
                })();
                self.temporary_roots.truncate(marker);
                result
            }
        }
    }

    /// Run a synchronous callback to completion while the iterator terminator
    /// remains on the caller's frame.  This keeps callback invocation lazy
    /// without introducing a second public async protocol.
    fn invoke_sync_value(
        &mut self,
        callee: Value,
        arguments: Vec<(BytecodeCallArgumentTarget, Value)>,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let operation = self.prepare_evaluated_call(callee, arguments)?;
        let OperationResult::Call {
            function,
            arguments,
        } = operation
        else {
            return match operation {
                OperationResult::Value(value) => Ok(Ok(value)),
                OperationResult::Panic(code, message) => Ok(Err((code, message))),
                OperationResult::HostAsync { name, .. } => Err(VmError::invariant(format!(
                    "async iterator callback `{name}` is not allowed"
                ))),
                OperationResult::OneShotWait { .. } => Err(VmError::invariant(
                    "one-shot iterator callback is not allowed",
                )),
                OperationResult::TestBoundaryCall { .. }
                | OperationResult::VirtualTimeBoundaryCall { .. } => Err(VmError::invariant(
                    "iterator callback crossed an internal async/test boundary",
                )),
                OperationResult::Call { .. } => unreachable!(),
            };
        };
        let base_depth = self.frames.len();
        self.push_frame(function, arguments, None)?;
        loop {
            self.step_budget()?;
            let frame = self
                .frames
                .len()
                .checked_sub(1)
                .ok_or_else(|| VmError::invariant("iterator callback lost its frame"))?;
            let (function_id, block_id, instruction_index) = {
                let frame = &self.frames[frame];
                (frame.function, frame.block, frame.instruction)
            };
            let function = self
                .program
                .function(function_id)
                .ok_or_else(|| VmError::invariant("iterator callback frame is invalid"))?;
            let block = function
                .block(block_id)
                .ok_or_else(|| VmError::invariant("iterator callback block is invalid"))?;
            if let Some(instruction) = block.instructions.get(instruction_index).cloned() {
                self.frames[frame].instruction += 1;
                self.execute_instruction(frame, &instruction)?;
            } else {
                let terminator = block.terminator.clone();
                if let Some(completion) = self.execute_terminator(frame, &terminator)? {
                    if self.frames.len() != base_depth {
                        return Err(VmError::invariant(
                            "iterator callback completed with an unexpected frame depth",
                        ));
                    }
                    return Ok(match completion {
                        TaskCompletion::Returned(value) => Ok(value),
                        TaskCompletion::Panicked(panic) => Err((panic.code, panic.message)),
                        TaskCompletion::Cancelled => Err((
                            PanicCode::ExplicitPanic,
                            "iterator callback was cancelled".into(),
                        )),
                    });
                }
            }
        }
    }

    fn borrowed_iterator_has_item(&self, source: &Value, next: usize) -> Result<bool, VmError> {
        let Value::Heap(handle) = source else {
            return Err(VmError::invariant(
                "borrowed iterator source is not managed",
            ));
        };
        match self.heap.get(*handle)? {
            HeapObject::Array(values) | HeapObject::Set(values) => Ok(next < values.len()),
            HeapObject::Map(entries) => Ok(next < entries.len()),
            _ => Err(VmError::invariant(
                "borrowed iterator source is not Array, Map, or Set",
            )),
        }
    }

    fn iterator_item(
        &mut self,
        source: &Value,
        item_ty: BytecodeTypeId,
        next: usize,
    ) -> Result<(Option<Value>, usize), VmError> {
        let Value::Heap(handle) = source else {
            return Err(VmError::invariant("iterator source is not managed"));
        };
        match self.heap.get(*handle)?.clone() {
            HeapObject::Array(mut values) => {
                if next != 0 {
                    return Err(VmError::invariant(
                        "owning Array iterator has a nonzero compact position",
                    ));
                }
                if values.is_empty() {
                    return Ok((None, usize::MAX));
                }
                let value = present(&values.remove(0), "iterator item")?.clone();
                self.replace_object(
                    *handle,
                    HeapObject::Array(values),
                    std::slice::from_ref(&value),
                )?;
                Ok((Some(value), 0))
            }
            HeapObject::Set(mut values) => {
                if next != 0 {
                    return Err(VmError::invariant(
                        "owning Set iterator has a nonzero compact position",
                    ));
                }
                if values.is_empty() {
                    return Ok((None, usize::MAX));
                }
                let value = present(&values.remove(0), "iterator item")?.clone();
                self.replace_object(
                    *handle,
                    HeapObject::Set(values),
                    std::slice::from_ref(&value),
                )?;
                Ok((Some(value), 0))
            }
            HeapObject::Map(mut entries) => {
                if next != 0 {
                    return Err(VmError::invariant(
                        "owning Map iterator has a nonzero compact position",
                    ));
                }
                if entries.is_empty() {
                    return Ok((None, usize::MAX));
                }
                let (key, value) = entries.remove(0);
                let key = present(&key, "map iterator key")?.clone();
                let value = present(&value, "map iterator value")?.clone();
                self.replace_object(
                    *handle,
                    HeapObject::Map(entries),
                    &[key.clone(), value.clone()],
                )?;
                let tuple = self.allocate(
                    item_ty,
                    HeapObject::Tuple(vec![Some(key.clone()), Some(value.clone())]),
                    &[key, value],
                )?;
                Ok((Some(tuple), 0))
            }
            HeapObject::String(text) => {
                let suffix = text.get(next..).ok_or_else(|| {
                    VmError::invariant("String iterator offset is not a UTF-8 boundary")
                })?;
                let Some(value) = suffix.chars().next() else {
                    return Ok((None, usize::MAX));
                };
                let next = next
                    .checked_add(value.len_utf8())
                    .ok_or_else(|| VmError::invariant("String iterator offset overflowed"))?;
                Ok((Some(Value::Char(value)), next))
            }
            HeapObject::Range { kind, start, end } => {
                let start = present(&start, "range start")?;
                let end = present(&end, "range end")?;
                self.range_item(kind, start, end, next)
            }
            _ => Err(VmError::invariant(
                "value is not an iterable bootstrap object",
            )),
        }
    }

    fn range_item(
        &self,
        kind: BytecodeRangeKind,
        start: &Value,
        end: &Value,
        next: usize,
    ) -> Result<(Option<Value>, usize), VmError> {
        match (start, end) {
            (Value::Integer(start), Value::Integer(end)) => {
                let offset = i128::try_from(next).map_err(|_| {
                    VmError::invariant("range iterator index exceeds the integer domain")
                })?;
                let Some(current) = start.checked_add(offset) else {
                    return Ok((None, usize::MAX));
                };
                let in_range = match kind {
                    BytecodeRangeKind::Exclusive => current < *end,
                    BytecodeRangeKind::Inclusive => current <= *end,
                } && start <= end;
                if !in_range {
                    return Ok((None, usize::MAX));
                }
                let finished = kind == BytecodeRangeKind::Inclusive && current == *end;
                Ok((
                    Some(Value::Integer(current)),
                    if finished {
                        usize::MAX
                    } else {
                        next.saturating_add(1)
                    },
                ))
            }
            (Value::Char(start), Value::Char(end)) => {
                let start_code = u32::from(*start);
                let mut current = start_code;
                let mut remaining = next;
                while remaining > 0 {
                    current = next_unicode_scalar(current).ok_or_else(|| {
                        VmError::invariant("Char range advanced past Unicode maximum")
                    })?;
                    remaining -= 1;
                }
                let end = u32::from(*end);
                let in_range = match kind {
                    BytecodeRangeKind::Exclusive => current < end,
                    BytecodeRangeKind::Inclusive => current <= end,
                } && start_code <= end;
                if !in_range {
                    return Ok((None, usize::MAX));
                }
                let value = char::from_u32(current)
                    .ok_or_else(|| VmError::invariant("Char range produced a surrogate"))?;
                let finished = kind == BytecodeRangeKind::Inclusive && current == end;
                Ok((
                    Some(Value::Char(value)),
                    if finished {
                        usize::MAX
                    } else {
                        next.saturating_add(1)
                    },
                ))
            }
            _ => Err(VmError::invariant("range endpoints have invalid values")),
        }
    }

    fn string_value<'a>(&'a self, value: &'a Value) -> Result<&'a str, VmError> {
        let Value::Heap(handle) = value else {
            return Err(VmError::invariant("String value is not managed"));
        };
        match self.heap.get(*handle)? {
            HeapObject::String(value) => Ok(value),
            _ => Err(VmError::invariant("String value has the wrong heap shape")),
        }
    }

    fn type_name(&self, ty: BytecodeTypeId) -> &str {
        self.program
            .ty(ty)
            .map_or("<invalid-type>", |ty| ty.name.as_str())
    }
}

fn clone_present(value: &Option<Value>, label: &str) -> Result<Value, VmError> {
    present(value, label).cloned()
}

fn next_unicode_scalar(value: u32) -> Option<u32> {
    let mut next = value.checked_add(1)?;
    if (0xd800..=0xdfff).contains(&next) {
        next = 0xe000;
    }
    (next <= 0x10ffff).then_some(next)
}

#[derive(Debug, Clone, PartialEq)]
struct ResolvedPlacePath {
    root: (usize, usize, u32),
    components: Vec<PlaceComponent>,
}

#[derive(Debug, Clone, PartialEq)]
enum PlaceComponent {
    Field(u32),
    Variant(u32),
    Index(i128),
    MapKey(RuntimeValue),
    Slice(Vec<usize>),
}

fn operation_access_place(operation: &BytecodeOperation) -> Result<Option<BytecodePlace>, VmError> {
    let (base, projection) = match &operation.kind {
        BytecodeOperationKind::Index {
            base,
            index,
            access,
            ..
        } => (
            base,
            BytecodeProjectionKind::Index {
                index: operand_materialized_slot(index)?,
                access: *access,
            },
        ),
        BytecodeOperationKind::Slice { base, bounds, .. } => (
            base,
            BytecodeProjectionKind::Slice {
                start: bounds
                    .start
                    .as_ref()
                    .map(operand_materialized_slot)
                    .transpose()?,
                end: bounds
                    .end
                    .as_ref()
                    .map(operand_materialized_slot)
                    .transpose()?,
                step: bounds
                    .step
                    .as_ref()
                    .map(operand_materialized_slot)
                    .transpose()?,
            },
        ),
        _ => return Ok(None),
    };
    let BytecodeOperandKind::Borrow(base) = &base.kind else {
        return Err(VmError::invariant(
            "indexed operation has no borrowed base place",
        ));
    };
    let mut place = base.clone();
    place.ty = operation.ty;
    place.projections.push(BytecodeProjection {
        ty: operation.ty,
        kind: projection,
    });
    Ok(Some(place))
}

fn operand_materialized_slot(operand: &BytecodeOperand) -> Result<BytecodeSlotId, VmError> {
    match &operand.kind {
        BytecodeOperandKind::Copy(place)
        | BytecodeOperandKind::Move(place)
        | BytecodeOperandKind::Borrow(place)
            if place.projections.is_empty() && place.source_loan.is_none() =>
        {
            Ok(place.slot)
        }
        _ => Err(VmError::invariant(
            "index or slice input is not a materialized slot",
        )),
    }
}

fn paths_overlap(left: &ResolvedPlacePath, right: &ResolvedPlacePath) -> bool {
    if left.root != right.root {
        return false;
    }
    for (left, right) in left.components.iter().zip(&right.components) {
        if left == right {
            continue;
        }
        return match (left, right) {
            (PlaceComponent::Slice(left), PlaceComponent::Slice(right)) => {
                left.iter().any(|index| right.contains(index))
            }
            (PlaceComponent::Slice(indices), PlaceComponent::Index(index))
            | (PlaceComponent::Index(index), PlaceComponent::Slice(indices)) => {
                usize::try_from(*index).is_ok_and(|index| indices.contains(&index))
            }
            _ => false,
        };
    }
    true
}

fn slice_indices(
    start: Option<i128>,
    end: Option<i128>,
    step: Option<i128>,
    length: usize,
) -> Result<Vec<usize>, (PanicCode, String)> {
    normalize_array_slice_indices(start, end, step, length).map_err(|error| match error {
        ArraySliceError::ZeroStep => (PanicCode::ZeroSliceStep, "slice step cannot be zero".into()),
        ArraySliceError::LengthNotRepresentable => (
            PanicCode::Bounds,
            "sequence length is not representable as Int".into(),
        ),
    })
}

fn clone_index(values: &[Option<Value>], index: u32, label: &str) -> Result<Value, VmError> {
    values
        .get(index as usize)
        .ok_or_else(|| VmError::invariant(format!("{label} index is invalid")))
        .and_then(|value| present(value, label))
        .cloned()
}

fn clone_field(fields: &[(u32, Option<Value>)], field: u32, label: &str) -> Result<Value, VmError> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == field)
        .ok_or_else(|| VmError::invariant(format!("{label} ID is invalid")))
        .and_then(|(_, value)| present(value, label))
        .cloned()
}

fn take_option(value: &mut Option<Value>, label: &str) -> Result<Value, VmError> {
    value
        .take()
        .ok_or_else(|| VmError::invariant(format!("{label} was already moved")))
}

fn take_index(values: &mut [Option<Value>], index: u32, label: &str) -> Result<Value, VmError> {
    values
        .get_mut(index as usize)
        .ok_or_else(|| VmError::invariant(format!("{label} index is invalid")))
        .and_then(|value| take_option(value, label))
}

fn take_field(
    fields: &mut [(u32, Option<Value>)],
    field: u32,
    label: &str,
) -> Result<Value, VmError> {
    fields
        .iter_mut()
        .find(|(candidate, _)| *candidate == field)
        .ok_or_else(|| VmError::invariant(format!("{label} ID is invalid")))
        .and_then(|(_, value)| take_option(value, label))
}

fn set_index(
    values: &mut [Option<Value>],
    index: u32,
    value: Value,
    label: &str,
) -> Result<(), VmError> {
    *values
        .get_mut(index as usize)
        .ok_or_else(|| VmError::invariant(format!("{label} index is invalid")))? = Some(value);
    Ok(())
}

fn set_field(fields: &mut [(u32, Option<Value>)], field: u32, value: Value) -> Result<(), VmError> {
    let slot = fields
        .iter_mut()
        .find(|(candidate, _)| *candidate == field)
        .ok_or_else(|| VmError::invariant("record field ID is invalid"))?;
    slot.1 = Some(value);
    Ok(())
}

fn present<'a>(value: &'a Option<Value>, label: &str) -> Result<&'a Value, VmError> {
    value
        .as_ref()
        .ok_or_else(|| VmError::invariant(format!("moved {label} used at runtime")))
}

fn queue_object_equality(
    left: &HeapObject,
    right: &HeapObject,
    pending: &mut Vec<(Value, Value)>,
) -> Result<bool, VmError> {
    let queue_options = |left: &[Option<Value>],
                         right: &[Option<Value>],
                         pending: &mut Vec<(Value, Value)>|
     -> Result<bool, VmError> {
        if left.len() != right.len() {
            return Ok(false);
        }
        for (left, right) in left.iter().zip(right) {
            pending.push((
                present(left, "aggregate element")?.clone(),
                present(right, "aggregate element")?.clone(),
            ));
        }
        Ok(true)
    };
    Ok(match (left, right) {
        (HeapObject::String(left), HeapObject::String(right)) => left == right,
        (HeapObject::Tuple(left), HeapObject::Tuple(right)) => queue_options(left, right, pending)?,
        (HeapObject::Array(left), HeapObject::Array(right)) => queue_options(left, right, pending)?,
        (
            HeapObject::Newtype {
                nominal: left_nominal,
                value: left,
            },
            HeapObject::Newtype {
                nominal: right_nominal,
                value: right,
            },
        ) => {
            if left_nominal != right_nominal {
                false
            } else {
                pending.push((
                    present(left, "newtype value")?.clone(),
                    present(right, "newtype value")?.clone(),
                ));
                true
            }
        }
        (
            HeapObject::Record {
                nominal: left_nominal,
                fields: left,
            },
            HeapObject::Record {
                nominal: right_nominal,
                fields: right,
            },
        ) => {
            if left_nominal != right_nominal
                || left.len() != right.len()
                || left
                    .iter()
                    .zip(right)
                    .any(|(left, right)| left.0 != right.0)
            {
                false
            } else {
                for ((_, left), (_, right)) in left.iter().zip(right) {
                    pending.push((
                        present(left, "record field")?.clone(),
                        present(right, "record field")?.clone(),
                    ));
                }
                true
            }
        }
        (
            HeapObject::Variant {
                variant: left_variant,
                payload: left,
            },
            HeapObject::Variant {
                variant: right_variant,
                payload: right,
            },
        ) => left_variant == right_variant && queue_payload_equality(left, right, pending)?,
        (HeapObject::OptionNone, HeapObject::OptionNone) => true,
        (HeapObject::OptionSome(left), HeapObject::OptionSome(right))
        | (HeapObject::ResultOk(left), HeapObject::ResultOk(right))
        | (HeapObject::ResultErr(left), HeapObject::ResultErr(right)) => {
            pending.push((
                present(left, "sum payload")?.clone(),
                present(right, "sum payload")?.clone(),
            ));
            true
        }
        (
            HeapObject::Union {
                member: left_member,
                value: left,
            },
            HeapObject::Union {
                member: right_member,
                value: right,
            },
        ) => {
            if left_member != right_member {
                false
            } else {
                pending.push((
                    present(left, "union value")?.clone(),
                    present(right, "union value")?.clone(),
                ));
                true
            }
        }
        (
            HeapObject::Range {
                kind: left_kind,
                start: left_start,
                end: left_end,
            },
            HeapObject::Range {
                kind: right_kind,
                start: right_start,
                end: right_end,
            },
        ) => {
            if left_kind != right_kind {
                false
            } else {
                pending.push((
                    present(left_start, "range start")?.clone(),
                    present(right_start, "range start")?.clone(),
                ));
                pending.push((
                    present(left_end, "range end")?.clone(),
                    present(right_end, "range end")?.clone(),
                ));
                true
            }
        }
        (HeapObject::Ref(_), HeapObject::Ref(_)) => false,
        (HeapObject::Closure { .. }, HeapObject::Closure { .. }) => {
            return Err(VmError::invariant("closure equality is not defined"));
        }
        (HeapObject::Iterator { .. }, HeapObject::Iterator { .. }) => {
            return Err(VmError::invariant("iterator equality is not defined"));
        }
        _ => false,
    })
}

fn queue_payload_equality(
    left: &AggregatePayload,
    right: &AggregatePayload,
    pending: &mut Vec<(Value, Value)>,
) -> Result<bool, VmError> {
    Ok(match (left, right) {
        (AggregatePayload::Unit, AggregatePayload::Unit) => true,
        (AggregatePayload::Tuple(left), AggregatePayload::Tuple(right)) => {
            if left.len() != right.len() {
                false
            } else {
                for (left, right) in left.iter().zip(right) {
                    pending.push((
                        present(left, "variant tuple item")?.clone(),
                        present(right, "variant tuple item")?.clone(),
                    ));
                }
                true
            }
        }
        (AggregatePayload::Record(left), AggregatePayload::Record(right)) => {
            if left.len() != right.len()
                || left
                    .iter()
                    .zip(right)
                    .any(|(left, right)| left.0 != right.0)
            {
                false
            } else {
                for ((_, left), (_, right)) in left.iter().zip(right) {
                    pending.push((
                        present(left, "variant field")?.clone(),
                        present(right, "variant field")?.clone(),
                    ));
                }
                true
            }
        }
        _ => false,
    })
}

fn place_contains(outer: &BytecodePlace, inner: &BytecodePlace) -> bool {
    outer.slot == inner.slot
        && outer.source_loan == inner.source_loan
        && outer.projections.len() <= inner.projections.len()
        && outer
            .projections
            .iter()
            .zip(&inner.projections)
            .all(|(left, right)| left == right)
}

fn convert_numeric(
    target: BytecodeScalarType,
    value: &Value,
) -> Result<Value, BytecodeNumericConversionError> {
    use BytecodeNumericConversionError as Error;

    let integer_target = integer_bounds(target);
    match value {
        Value::Integer(value) => {
            if let Some((minimum, maximum)) = integer_target {
                if (minimum..=maximum).contains(value) {
                    Ok(Value::Integer(*value))
                } else {
                    Err(Error::OutOfRange)
                }
            } else if target == BytecodeScalarType::Byte {
                u8::try_from(*value)
                    .map(Value::Byte)
                    .map_err(|_| Error::OutOfRange)
            } else if target == BytecodeScalarType::Float32 {
                Ok(Value::Float(f64::from(*value as f32)))
            } else if target == BytecodeScalarType::Float {
                Ok(Value::Float(*value as f64))
            } else {
                Err(Error::OutOfRange)
            }
        }
        Value::Byte(value) => {
            if target == BytecodeScalarType::Byte {
                Ok(Value::Byte(*value))
            } else {
                convert_numeric(target, &Value::Integer(i128::from(*value)))
            }
        }
        Value::Float(value) => {
            if target == BytecodeScalarType::Float {
                Ok(Value::Float(*value))
            } else if target == BytecodeScalarType::Float32 {
                let converted = *value as f32;
                if value.is_finite() && converted.is_infinite() {
                    Err(Error::OutOfRange)
                } else {
                    Ok(Value::Float(f64::from(converted)))
                }
            } else {
                if !value.is_finite() {
                    return Err(Error::NotFinite);
                }
                if value.fract() != 0.0 {
                    return Err(Error::NotIntegral);
                }
                if target == BytecodeScalarType::Byte {
                    if (0.0..=255.0).contains(value) {
                        Ok(Value::Byte(*value as u8))
                    } else {
                        Err(Error::OutOfRange)
                    }
                } else if let Some((minimum, maximum)) = integer_target {
                    if *value >= minimum as f64 && *value <= maximum as f64 {
                        let converted = *value as i128;
                        if converted >= minimum && converted <= maximum {
                            Ok(Value::Integer(converted))
                        } else {
                            Err(Error::OutOfRange)
                        }
                    } else {
                        Err(Error::OutOfRange)
                    }
                } else {
                    Err(Error::OutOfRange)
                }
            }
        }
        Value::Unit
        | Value::Bool(_)
        | Value::Char(_)
        | Value::Function { .. }
        | Value::Loan(_)
        | Value::Join(_)
        | Value::Host(_)
        | Value::Heap(_) => Err(Error::OutOfRange),
    }
}

fn integer_shape(scalar: BytecodeScalarType) -> Option<(bool, u32)> {
    Some(match scalar {
        BytecodeScalarType::Int => (true, 64),
        BytecodeScalarType::Int8 => (true, 8),
        BytecodeScalarType::Int16 => (true, 16),
        BytecodeScalarType::Int32 => (true, 32),
        BytecodeScalarType::UInt8 => (false, 8),
        BytecodeScalarType::UInt16 => (false, 16),
        BytecodeScalarType::UInt32 => (false, 32),
        BytecodeScalarType::UInt64 => (false, 64),
        BytecodeScalarType::Bool
        | BytecodeScalarType::Float
        | BytecodeScalarType::Byte
        | BytecodeScalarType::Char
        | BytecodeScalarType::String
        | BytecodeScalarType::Unit
        | BytecodeScalarType::Never
        | BytecodeScalarType::Float32 => return None,
    })
}

fn integer_bounds(scalar: BytecodeScalarType) -> Option<(i128, i128)> {
    let (signed, bits) = integer_shape(scalar)?;
    Some(if signed {
        let magnitude = 1_i128 << (bits - 1);
        (-magnitude, magnitude - 1)
    } else {
        (0, (1_i128 << bits) - 1)
    })
}

fn collection_length_fits_int(length: usize) -> bool {
    let (_, maximum) =
        integer_bounds(BytecodeScalarType::Int).expect("the closed Int scalar is always integral");
    i128::try_from(length).is_ok_and(|length| length <= maximum)
}

#[cfg(test)]
mod tests {
    use crate::bytecode::{
        BytecodeAggregateKind, BytecodeBinaryOperator, BytecodeBlockId, BytecodeCallArgumentTarget,
        BytecodeCallable, BytecodeCallableId, BytecodeCapabilitySet, BytecodeClosure,
        BytecodeClosureProtocols, BytecodeConstant, BytecodeConstantValue,
        BytecodeConstantValueKind, BytecodeConstantVariantValue, BytecodeContainmentKind,
        BytecodeCursorMode, BytecodeField, BytecodeFrameTraceDescriptor, BytecodeFunction,
        BytecodeFunctionId, BytecodeFunctionParameter, BytecodeFunctionType, BytecodeIndexAccess,
        BytecodeIntrinsicType, BytecodeLoanId, BytecodeNominal, BytecodeNominalId,
        BytecodeNominalShape, BytecodeNumericConversionError, BytecodeOperand, BytecodeOperandKind,
        BytecodeOperation, BytecodeOperationKind, BytecodeParameter, BytecodeParameterMode,
        BytecodePlace, BytecodePrefixOperator, BytecodeProgram, BytecodeRangeKind,
        BytecodeScalarType, BytecodeScopeId, BytecodeSliceBounds, BytecodeSlotId, BytecodeSpan,
        BytecodeTraceDescriptor, BytecodeType, BytecodeTypeId, BytecodeTypeKind, BytecodeVariant,
        BytecodeVariantPayload, derive_trace_metadata,
    };

    use super::{
        AggregatePayload, DeferredOperation, DeferredValue, Engine, Frame, HeapObject,
        IteratorAdapter, OneShotCompletion, OneShotState, OperationResult, PanicCode,
        PlaceComponent, PlaceFailure, RejectingHost, ResolvedPlacePath, RuntimeCleanup,
        RuntimeDefer, RuntimeFallback, RuntimeHostValueKind, RuntimeJoin, RuntimeLoan,
        RuntimeTaskScope, RuntimeType, RuntimeValue, SlotState, TaskCompletion, TaskRecord,
        TaskStatus, TaskWait, Value, ValueCopyStrategy, VmError, VmHost, VmLimits, VmTestNodeKind,
        VmTestNodeOutcome, clone_field, clone_index, clone_present, collection_length_fits_int,
        convert_numeric, integer_bounds, integer_shape, next_unicode_scalar,
        operand_materialized_slot, operation_access_place, paths_overlap, present,
        queue_object_equality, queue_payload_equality, runtime_host_kind, set_field, set_index,
        slice_indices, snapshot_value, take_field, take_index, take_option,
    };

    fn root_pressure_program() -> BytecodeProgram {
        let string = BytecodeTypeId::new(0);
        let strings = BytecodeTypeId::new(1);
        BytecodeProgram {
            types: vec![
                BytecodeType {
                    name: "String".into(),
                    kind: BytecodeTypeKind::Scalar(BytecodeScalarType::String),
                },
                BytecodeType {
                    name: "Array[String]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Array,
                        arguments: vec![string],
                    },
                },
                BytecodeType {
                    name: "Map[String, Array[String]]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Map,
                        arguments: vec![string, strings],
                    },
                },
                BytecodeType {
                    name: "Range[String]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Range,
                        arguments: vec![string],
                    },
                },
                BytecodeType {
                    name: "(Array[String], Array[String])".into(),
                    kind: BytecodeTypeKind::Tuple(vec![strings, strings]),
                },
                BytecodeType {
                    name: "Int".into(),
                    kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Int),
                },
                BytecodeType {
                    name: "Array[Int]".into(),
                    kind: BytecodeTypeKind::Intrinsic {
                        constructor: BytecodeIntrinsicType::Array,
                        arguments: vec![BytecodeTypeId::new(5)],
                    },
                },
                BytecodeType {
                    name: "cursor[own, Array[Int]]".into(),
                    kind: BytecodeTypeKind::Cursor {
                        mode: BytecodeCursorMode::Own,
                        collection: BytecodeTypeId::new(6),
                    },
                },
            ],
            nominals: Vec::new(),
            callables: Vec::new(),
            constants: Vec::new(),
            functions: Vec::new(),
        }
    }

    fn terminal_fallback_program() -> BytecodeProgram {
        let mut program = root_pressure_program();
        let string = BytecodeTypeId::new(0);
        let strings = BytecodeTypeId::new(1);
        let int = BytecodeTypeId::new(5);
        let ints = BytecodeTypeId::new(6);
        let never = BytecodeTypeId::new(19);

        program.types.extend([
            BytecodeType {
                name: "Set[String]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Set,
                    arguments: vec![string],
                },
            },
            BytecodeType {
                name: "String?".into(),
                kind: BytecodeTypeKind::Option(string),
            },
            BytecodeType {
                name: "String ! Array[String]".into(),
                kind: BytecodeTypeKind::Result {
                    success: string,
                    error: strings,
                },
            },
            BytecodeType {
                name: "String | Array[String]".into(),
                kind: BytecodeTypeKind::Union(vec![string, strings]),
            },
            BytecodeType {
                name: "$0".into(),
                kind: BytecodeTypeKind::GenericParameter(0),
            },
            BytecodeType {
                name: "TextBox".into(),
                kind: BytecodeTypeKind::Nominal {
                    nominal: Some(BytecodeNominalId::new(0)),
                    identity: "test::TextBox".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Message".into(),
                kind: BytecodeTypeKind::Nominal {
                    nominal: Some(BytecodeNominalId::new(1)),
                    identity: "test::Message".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Event".into(),
                kind: BytecodeTypeKind::Nominal {
                    nominal: Some(BytecodeNominalId::new(2)),
                    identity: "test::Event".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "closure-environment".into(),
                kind: BytecodeTypeKind::Generated {
                    identity: "test::closure".into(),
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "cursor[ref, Array[Int]]".into(),
                kind: BytecodeTypeKind::Cursor {
                    mode: BytecodeCursorMode::Ref,
                    collection: ints,
                },
            },
            BytecodeType {
                name: "ProcessHandle".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::ProcessHandle,
                    arguments: Vec::new(),
                },
            },
            BytecodeType {
                name: "Never".into(),
                kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Never),
            },
            BytecodeType {
                name: "Join[String, Never]".into(),
                kind: BytecodeTypeKind::Intrinsic {
                    constructor: BytecodeIntrinsicType::Join,
                    arguments: vec![string, never],
                },
            },
        ]);
        program.nominals.extend([
            BytecodeNominal {
                name: "TextBox".into(),
                identity: "test::TextBox".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Newtype { underlying: string },
            },
            BytecodeNominal {
                name: "Message".into(),
                identity: "test::Message".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Record {
                    fields: vec![
                        BytecodeField {
                            member: 0,
                            ty: string,
                        },
                        BytecodeField {
                            member: 1,
                            ty: strings,
                        },
                    ],
                },
            },
            BytecodeNominal {
                name: "Event".into(),
                identity: "test::Event".into(),
                generic_arity: 0,
                shape: BytecodeNominalShape::Enum {
                    variants: vec![
                        BytecodeVariant {
                            member: 0,
                            payload: BytecodeVariantPayload::Unit,
                        },
                        BytecodeVariant {
                            member: 1,
                            payload: BytecodeVariantPayload::Tuple(vec![string]),
                        },
                        BytecodeVariant {
                            member: 2,
                            payload: BytecodeVariantPayload::Record(vec![BytecodeField {
                                member: 0,
                                ty: strings,
                            }]),
                        },
                    ],
                },
            },
        ]);
        program.callables.push(BytecodeCallable {
            name: "closure".into(),
            generic_arity: 0,
            parameters: Vec::new(),
            outcome: int,
            function_type: int,
            implementation: None,
            closure: Some(BytecodeClosure {
                environment: BytecodeTypeId::new(16),
                captures: vec![string, strings],
                protocols: BytecodeClosureProtocols {
                    call: true,
                    call_mut: false,
                    call_once: true,
                },
            }),
        });
        program
    }

    fn install_fallback_frame(
        engine: &mut Engine<'_, '_>,
        ty: BytecodeTypeId,
        value: Value,
    ) -> RuntimeFallback {
        let function = BytecodeFunctionId::new(0);
        engine.frame_traces.push(BytecodeFrameTraceDescriptor {
            function,
            slots: vec![ty],
        });
        engine.frames.push(Frame {
            function,
            block: BytecodeBlockId::new(0),
            instruction: 0,
            slots: vec![SlotState::Value(value)],
            loans: Vec::new(),
            cleanups: Vec::new(),
            task_scopes: Vec::new(),
            continuation: None,
        });
        RuntimeFallback {
            scope: BytecodeScopeId::new(0),
            owner: BytecodePlace {
                slot: BytecodeSlotId::new(0),
                ty,
                projections: Vec::new(),
                source_loan: None,
            },
        }
    }

    fn assert_fallback_error(
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
        value: Value,
        expected: &str,
    ) {
        let trace = derive_trace_metadata(program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let fallback = install_fallback_frame(&mut engine, ty, value);
        let error = engine.execute_terminal_fallback(0, fallback).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected `{expected}`, got `{error}`"
        );
    }

    fn assert_fallback_heap_error(
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
        object: HeapObject,
        expected: &str,
    ) {
        let mut trace = derive_trace_metadata(program).unwrap();
        trace.types[ty.index() as usize] = match &object {
            HeapObject::String(_) => BytecodeTraceDescriptor::String,
            HeapObject::Tuple(values) => BytecodeTraceDescriptor::Tuple {
                fields: vec![BytecodeTypeId::new(0); values.len()],
            },
            HeapObject::Record { nominal, fields } => BytecodeTraceDescriptor::Record {
                nominal: *nominal,
                arguments: Vec::new(),
                fields: fields
                    .iter()
                    .map(|(member, _)| BytecodeField {
                        member: *member,
                        ty: BytecodeTypeId::new(0),
                    })
                    .collect(),
            },
            HeapObject::Variant { variant, payload } => BytecodeTraceDescriptor::Variant {
                nominal: Some(BytecodeNominalId::new(2)),
                arguments: Vec::new(),
                variants: vec![BytecodeVariant {
                    member: *variant,
                    payload: match payload {
                        AggregatePayload::Unit => BytecodeVariantPayload::Unit,
                        AggregatePayload::Tuple(values) => BytecodeVariantPayload::Tuple(vec![
                            BytecodeTypeId::new(0);
                            values.len()
                        ]),
                        AggregatePayload::Record(fields) => BytecodeVariantPayload::Record(
                            fields
                                .iter()
                                .map(|(member, _)| BytecodeField {
                                    member: *member,
                                    ty: BytecodeTypeId::new(0),
                                })
                                .collect(),
                        ),
                    },
                }],
            },
            HeapObject::Closure { callable, captures } => BytecodeTraceDescriptor::Closure {
                callable: *callable,
                captures: vec![BytecodeTypeId::new(0); captures.len()],
            },
            HeapObject::Iterator { mode, .. } => BytecodeTraceDescriptor::Cursor {
                mode: *mode,
                collection: BytecodeTypeId::new(6),
            },
            _ => panic!("error fixture must provide an explicit trace-compatible object"),
        };
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let value = engine.allocate(ty, object, &[]).unwrap();
        let fallback = install_fallback_frame(&mut engine, ty, value);
        let error = engine.execute_terminal_fallback(0, fallback).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected `{expected}`, got `{error}`"
        );
    }

    fn execute_fallback_object(
        program: &BytecodeProgram,
        ty: BytecodeTypeId,
        object: HeapObject,
    ) -> HeapObject {
        let trace = derive_trace_metadata(program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let value = engine.allocate(ty, object, &[]).unwrap();
        let handle = value.heap_handle().unwrap();
        let fallback = install_fallback_frame(&mut engine, ty, value);
        engine.execute_terminal_fallback(0, fallback).unwrap();
        engine.heap.get(handle).unwrap().clone()
    }

    fn pressure_limits() -> VmLimits {
        VmLimits {
            max_heap_objects: 64,
            max_heap_bytes: 64 * 1024,
            initial_gc_threshold: 1,
            ..VmLimits::default()
        }
    }

    fn scheduler_task(status: TaskStatus) -> TaskRecord {
        TaskRecord {
            frames: Vec::new(),
            pending_unwind: None,
            status,
            resume: None,
            queued: false,
            cancel_requested: false,
            waiters: Vec::new(),
            parent_scope: None,
            join_consumed: true,
            panic_observed: false,
        }
    }

    #[test]
    fn completed_and_cancelled_tasks_register_in_their_parent_scope() {
        let mut program = root_pressure_program();
        let waiter_ty = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Waiter".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Waiter,
                arguments: vec![BytecodeTypeId::new(5), BytecodeTypeId::new(0)],
            },
        });
        let completer_ty = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Completer".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Completer,
                arguments: vec![BytecodeTypeId::new(5), BytecodeTypeId::new(0)],
            },
        });
        let pair_ty = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "(Waiter, Completer)".into(),
            kind: BytecodeTypeKind::Tuple(vec![waiter_ty, completer_ty]),
        });
        let result_ty = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Int ! Int".into(),
            kind: BytecodeTypeKind::Result {
                success: BytecodeTypeId::new(5),
                error: BytecodeTypeId::new(5),
            },
        });
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );
        engine.task_scopes.push(Some(RuntimeTaskScope {
            source: BytecodeScopeId::new(0),
            owner: 0,
            children: Vec::new(),
            closed: false,
        }));

        let OperationResult::Value(Value::Heap(_)) = engine.new_oneshot(pair_ty).unwrap() else {
            panic!("new one-shot did not return its pair")
        };
        engine.tasks.push(scheduler_task(TaskStatus::Running));
        engine
            .park_oneshot(
                1,
                TaskWait::OneShotTask {
                    id: 1,
                    outcome: result_ty,
                },
            )
            .unwrap();
        assert!(
            engine
                .park_oneshot(
                    99,
                    TaskWait::OneShotTask {
                        id: 99,
                        outcome: result_ty,
                    },
                )
                .is_err()
        );
        assert!(
            !engine
                .complete_oneshot(1, OneShotCompletion::Ok(Value::Integer(7)))
                .unwrap()
        );
        assert!(
            engine
                .complete_oneshot(1, OneShotCompletion::Cancelled)
                .unwrap()
        );
        assert!(
            engine
                .oneshot_result(result_ty, Ok(Value::Integer(1)))
                .is_ok()
        );
        assert!(engine.oneshot_result(result_ty, Err(Value::Unit)).is_ok());
        assert!(
            engine
                .oneshot_result(BytecodeTypeId::new(5), Ok(Value::Unit))
                .is_err()
        );
        assert!(
            engine
                .oneshot_result(BytecodeTypeId::new(999), Ok(Value::Unit))
                .is_err()
        );

        engine.oneshots.insert(2, OneShotState::default());
        let pending = engine.spawn_oneshot_task(2, result_ty, 0).unwrap();
        assert!(matches!(
            engine.tasks[pending].status,
            TaskStatus::Waiting(_)
        ));
        assert!(engine.spawn_oneshot_task(2, result_ty, 99).is_err());
        engine
            .complete_oneshot(2, OneShotCompletion::Cancelled)
            .unwrap();
        let cancelled_child = engine.spawn_oneshot_task(2, result_ty, 0).unwrap();
        assert!(matches!(
            engine.tasks[cancelled_child].status,
            TaskStatus::Complete(Some(TaskCompletion::Cancelled))
        ));
        engine.oneshots.insert(
            3,
            OneShotState {
                completion: Some(OneShotCompletion::Ok(Value::Integer(1))),
                ..OneShotState::default()
            },
        );
        assert!(engine.spawn_oneshot_task(3, result_ty, 0).is_err());
        assert!(engine.spawn_oneshot_task(999, result_ty, 0).is_err());
        assert!(
            engine
                .complete_oneshot(999, OneShotCompletion::Cancelled)
                .is_err()
        );

        let waiter = |name: &str, parameters: Vec<BytecodeParameter>| BytecodeCallable {
            name: name.into(),
            generic_arity: 0,
            parameters,
            outcome: result_ty,
            function_type: result_ty,
            implementation: None,
            closure: None,
        };
        let receiver = BytecodeParameter {
            mode: BytecodeParameterMode::Value,
            ty: waiter_ty,
            variadic_element: None,
            receiver: true,
        };
        assert!(
            engine
                .prepare_oneshot_method(&waiter("std.async.Waiter.wait", Vec::new()), &[])
                .is_err()
        );
        assert!(
            engine
                .prepare_oneshot_method(&waiter("std.async.Waiter.wait", vec![receiver]), &[])
                .is_err()
        );
        let waiter_value = Value::Host(RuntimeValue::Host {
            kind: RuntimeHostValueKind::Waiter,
            id: 6,
        });
        engine.oneshots.insert(6, OneShotState::default());
        assert!(matches!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Waiter.wait", vec![receiver]),
                    std::slice::from_ref(&waiter_value),
                )
                .unwrap(),
            Some(OperationResult::OneShotWait { id: 6, .. })
        ));
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Waiter.wait", vec![receiver]),
                    &[waiter_value],
                )
                .is_err()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Waiter.wait", vec![receiver]),
                    &[Value::Host(RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Waiter,
                        id: 99,
                    })],
                )
                .is_err()
        );

        let completer_receiver = BytecodeParameter {
            ty: completer_ty,
            ..receiver
        };
        assert!(
            engine
                .prepare_oneshot_method(&waiter("std.async.Completer.complete", Vec::new()), &[],)
                .is_err()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Completer.complete", vec![completer_receiver]),
                    &[],
                )
                .is_err()
        );
        engine.oneshots.insert(4, OneShotState::default());
        let completer_value = Value::Host(RuntimeValue::Host {
            kind: RuntimeHostValueKind::Completer,
            id: 4,
        });
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Completer.complete", vec![completer_receiver],),
                    std::slice::from_ref(&completer_value),
                )
                .is_err()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter(
                        "std.async.Completer.complete",
                        vec![completer_receiver, receiver],
                    ),
                    &[completer_value.clone(), Value::Integer(9)],
                )
                .unwrap()
                .is_some()
        );
        engine.oneshots.insert(7, OneShotState::default());
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter(
                        "std.async.Completer.fail",
                        vec![completer_receiver, receiver],
                    ),
                    &[Value::Host(RuntimeValue::Host {
                        kind: RuntimeHostValueKind::Completer,
                        id: 7,
                    })],
                )
                .is_err()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter(
                        "std.async.Completer.complete",
                        vec![completer_receiver, receiver],
                    ),
                    &[completer_value.clone(), Value::Integer(9)],
                )
                .unwrap()
                .is_some()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter(
                        "std.async.Completer.fail",
                        vec![completer_receiver, receiver],
                    ),
                    &[completer_value.clone(), Value::Integer(0)],
                )
                .unwrap()
                .is_some()
        );
        assert!(
            engine
                .prepare_oneshot_method(
                    &waiter("std.async.Completer.cancel", vec![completer_receiver]),
                    &[completer_value],
                )
                .unwrap()
                .is_some()
        );

        let completed = engine.spawn_completed_task(Value::Integer(42), 0).unwrap();
        let cancelled = engine.spawn_cancelled_task(0).unwrap();
        assert!(engine.spawn_completed_task(Value::Unit, 99).is_err());
        assert!(engine.spawn_cancelled_task(99).is_err());

        assert!(matches!(
            engine.tasks[completed].status,
            TaskStatus::Complete(Some(TaskCompletion::Returned(Value::Integer(42))))
        ));
        assert!(matches!(
            engine.tasks[cancelled].status,
            TaskStatus::Complete(Some(TaskCompletion::Cancelled))
        ));
        let children = &engine.task_scopes[0].as_ref().unwrap().children;
        assert!(children.contains(&completed));
        assert!(children.contains(&cancelled));
    }

    #[test]
    fn entry_and_limit_contracts_reject_each_invalid_boundary() {
        let mut program = root_pressure_program();
        let int = BytecodeTypeId::new(5);
        let function_type = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "fn main()".into(),
            kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                is_async: false,
                is_unsafe: false,
                parameters: Vec::new(),
                variadic: None,
                outcome: int,
            }),
        });
        program.callables.push(BytecodeCallable {
            name: "main".into(),
            generic_arity: 0,
            parameters: Vec::new(),
            outcome: int,
            function_type,
            implementation: Some(BytecodeFunctionId::new(0)),
            closure: None,
        });
        program.functions.push(BytecodeFunction {
            callable: BytecodeCallableId::new(0),
            source: BytecodeSpan {
                file: 0,
                start: 0,
                end: 0,
            },
            types: Vec::new(),
            spans: vec![BytecodeSpan {
                file: 0,
                start: 0,
                end: 0,
            }],
            slots: Vec::new(),
            loans: Vec::new(),
            parameters: Vec::new(),
            return_slot: BytecodeSlotId::new(0),
            entry: BytecodeBlockId::new(0),
            unwind: BytecodeBlockId::new(0),
            blocks: Vec::new(),
        });
        let entry = BytecodeFunctionId::new(0);
        super::validate_entry_contract(&program, entry).unwrap();

        let error =
            super::validate_entry_contract(&program, BytecodeFunctionId::new(99)).unwrap_err();
        assert!(
            matches!(error, VmError::InvalidEntry(message) if message.contains("unknown function"))
        );

        let mut missing_callable = program.clone();
        missing_callable.functions[0].callable = BytecodeCallableId::new(99);
        let error = super::validate_entry_contract(&missing_callable, entry).unwrap_err();
        assert!(matches!(error, VmError::InvalidEntry(message) if message.contains("no callable")));

        let mut missing_signature = program.clone();
        missing_signature.callables[0].function_type = BytecodeTypeId::new(99);
        let error = super::validate_entry_contract(&missing_signature, entry).unwrap_err();
        assert!(
            matches!(error, VmError::InvalidEntry(message) if message.contains("no function type"))
        );

        let mut non_function = program.clone();
        non_function.callables[0].function_type = int;
        let error = super::validate_entry_contract(&non_function, entry).unwrap_err();
        assert!(
            matches!(error, VmError::InvalidEntry(message) if message.contains("not a function"))
        );

        let mut unsafe_entry = program.clone();
        let BytecodeTypeKind::Function(signature) =
            &mut unsafe_entry.types[function_type.index() as usize].kind
        else {
            unreachable!()
        };
        signature.is_unsafe = true;
        let error = super::validate_entry_contract(&unsafe_entry, entry).unwrap_err();
        assert!(matches!(error, VmError::InvalidEntry(message) if message.contains("unsafe")));

        for (name, limits) in [
            (
                "max_verification_steps",
                VmLimits {
                    max_verification_steps: 0,
                    ..VmLimits::default()
                },
            ),
            (
                "max_steps",
                VmLimits {
                    max_steps: 0,
                    ..VmLimits::default()
                },
            ),
            (
                "max_stack_depth",
                VmLimits {
                    max_stack_depth: 0,
                    ..VmLimits::default()
                },
            ),
            (
                "max_heap_objects",
                VmLimits {
                    max_heap_objects: 0,
                    ..VmLimits::default()
                },
            ),
            (
                "max_heap_bytes",
                VmLimits {
                    max_heap_bytes: 0,
                    ..VmLimits::default()
                },
            ),
            (
                "initial_gc_threshold",
                VmLimits {
                    initial_gc_threshold: 0,
                    ..VmLimits::default()
                },
            ),
        ] {
            let error = super::validate_limits(limits).unwrap_err();
            assert!(matches!(error, VmError::InvalidLimits(candidate) if candidate == name));
        }
    }

    #[test]
    fn deferred_operation_and_cleanup_roots_cover_all_runtime_shapes() {
        let ty = BytecodeTypeId::new(5);
        let place = BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty,
            projections: Vec::new(),
            source_loan: None,
        };
        let captured = Value::Integer(7);
        let mut roots = Vec::new();
        DeferredValue::Captured(captured.clone()).roots(&mut roots);
        DeferredValue::Guard.roots(&mut roots);
        assert_eq!(roots, vec![captured.clone()]);

        let assertion = DeferredOperation::Assert {
            condition: DeferredValue::Captured(captured.clone()),
            condition_repr: "value".into(),
            message_parts: vec![(DeferredValue::Guard, false)],
        };
        assertion.roots(&mut roots);
        assert_eq!(roots, vec![captured.clone(), captured.clone()]);

        let host_call = DeferredOperation::BootstrapHostCall {
            function: crate::bytecode::BytecodeBootstrapHostFunction::ConsolePrint,
            arguments: vec![
                DeferredValue::Captured(captured.clone()),
                DeferredValue::Guard,
            ],
        };
        host_call.roots(&mut roots);
        assert_eq!(roots, vec![captured.clone(), captured.clone(), captured]);

        let scope = BytecodeScopeId::new(3);
        let mut explicit = RuntimeCleanup::Explicit(RuntimeDefer {
            scope,
            span: BytecodeSpan {
                file: 0,
                start: 0,
                end: 0,
            },
            operation: assertion,
            guard: Some(place.clone()),
            async_cleanup: false,
        });
        assert_eq!(explicit.scope(), scope);
        assert_eq!(explicit.guard(), Some(&place));
        explicit.guard_mut().unwrap().slot = BytecodeSlotId::new(1);
        explicit.roots(&mut roots);

        let mut fallback = RuntimeCleanup::Fallback(RuntimeFallback {
            scope,
            owner: place,
        });
        assert_eq!(fallback.scope(), scope);
        assert!(fallback.guard().is_some());
        fallback.guard_mut().unwrap().slot = BytecodeSlotId::new(2);
        let before = roots.len();
        fallback.roots(&mut roots);
        assert_eq!(roots.len(), before);
    }

    #[test]
    fn terminal_fallback_dismantles_every_managed_shape_without_retaining_children() {
        let program = terminal_fallback_program();

        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(4),
                HeapObject::Tuple(vec![None, None]),
            ),
            HeapObject::Tuple(vec![None, None])
        );
        assert_eq!(
            execute_fallback_object(&program, BytecodeTypeId::new(9), HeapObject::OptionNone,),
            HeapObject::OptionNone
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(9),
                HeapObject::OptionSome(Some(Value::Unit)),
            ),
            HeapObject::OptionSome(None)
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(10),
                HeapObject::ResultOk(Some(Value::Unit)),
            ),
            HeapObject::ResultOk(None)
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(10),
                HeapObject::ResultErr(None),
            ),
            HeapObject::ResultErr(None)
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(11),
                HeapObject::Union {
                    member: BytecodeTypeId::new(0),
                    value: Some(Value::Unit),
                },
            ),
            HeapObject::Union {
                member: BytecodeTypeId::new(0),
                value: None,
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(1),
                HeapObject::Array(vec![Some(Value::Unit)].into()),
            ),
            HeapObject::Array(vec![None].into())
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(8),
                HeapObject::Set(vec![Some(Value::Unit)].into()),
            ),
            HeapObject::Set(vec![None].into())
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(2),
                HeapObject::Map(vec![(Some(Value::Unit), None)].into()),
            ),
            HeapObject::Map(vec![(None, None)].into())
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(3),
                HeapObject::Range {
                    kind: BytecodeRangeKind::Inclusive,
                    start: Some(Value::Unit),
                    end: Some(Value::Unit),
                },
            ),
            HeapObject::Range {
                kind: BytecodeRangeKind::Inclusive,
                start: None,
                end: None,
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(13),
                HeapObject::Newtype {
                    nominal: BytecodeNominalId::new(0),
                    value: Some(Value::Unit),
                },
            ),
            HeapObject::Newtype {
                nominal: BytecodeNominalId::new(0),
                value: None,
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(14),
                HeapObject::Record {
                    nominal: BytecodeNominalId::new(1),
                    fields: vec![(0, Some(Value::Unit)), (1, None)],
                },
            ),
            HeapObject::Record {
                nominal: BytecodeNominalId::new(1),
                fields: vec![(0, None), (1, None)],
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(15),
                HeapObject::Variant {
                    variant: 0,
                    payload: AggregatePayload::Unit,
                },
            ),
            HeapObject::Variant {
                variant: 0,
                payload: AggregatePayload::Unit,
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(15),
                HeapObject::Variant {
                    variant: 1,
                    payload: AggregatePayload::Tuple(vec![Some(Value::Unit)]),
                },
            ),
            HeapObject::Variant {
                variant: 1,
                payload: AggregatePayload::Tuple(vec![None]),
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(15),
                HeapObject::Variant {
                    variant: 2,
                    payload: AggregatePayload::Record(vec![(0, None)]),
                },
            ),
            HeapObject::Variant {
                variant: 2,
                payload: AggregatePayload::Record(vec![(0, None)]),
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(16),
                HeapObject::Closure {
                    callable: BytecodeCallableId::new(0),
                    captures: vec![Some(Value::Unit), None],
                },
            ),
            HeapObject::Closure {
                callable: BytecodeCallableId::new(0),
                captures: vec![None, None],
            }
        );
        assert_eq!(
            execute_fallback_object(
                &program,
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: None,
                    next: 3,
                    adapter: None,
                },
            ),
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Own,
                source: None,
                next: 3,
                adapter: None,
            }
        );

        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let fallback = install_fallback_frame(&mut engine, BytecodeTypeId::new(17), Value::Unit);
        engine.execute_terminal_fallback(0, fallback).unwrap();
    }

    #[test]
    fn terminal_fallback_rejects_every_incompatible_runtime_shape() {
        let program = terminal_fallback_program();

        assert_fallback_error(
            &program,
            BytecodeTypeId::new(999),
            Value::Unit,
            "unknown type",
        );
        assert_fallback_error(
            &program,
            BytecodeTypeId::new(12),
            Value::Unit,
            "cannot resolve a generic nominal component",
        );
        for (ty, expected) in [
            (4, "tuple fallback found a non-managed value"),
            (9, "Option fallback found a non-managed value"),
            (10, "Result fallback found a non-managed value"),
            (11, "union fallback found a non-managed value"),
            (1, "collection fallback found a non-managed value"),
            (8, "collection fallback found a non-managed value"),
            (2, "Map fallback found a non-managed value"),
            (3, "Range fallback found a non-managed value"),
            (13, "nominal fallback found a non-managed value"),
            (16, "closure fallback found a non-managed value"),
            (7, "cursor fallback found a non-managed value"),
        ] {
            assert_fallback_error(&program, BytecodeTypeId::new(ty), Value::Unit, expected);
        }
        assert_fallback_error(
            &program,
            BytecodeTypeId::new(18),
            Value::Unit,
            "ProcessHandle fallback found a non-host value",
        );
        assert_fallback_error(
            &program,
            BytecodeTypeId::new(20),
            Value::Unit,
            "Join fallback found no task handle",
        );

        for (ty, expected) in [
            (4, "tuple fallback found a different heap object"),
            (9, "Option fallback found a different heap object"),
            (10, "Result fallback found a different heap object"),
            (11, "union fallback found a different heap object"),
            (1, "collection fallback found a different heap object"),
            (8, "collection fallback found a different heap object"),
            (2, "Map fallback found a different heap object"),
            (3, "Range fallback found a different heap object"),
            (13, "nominal fallback found a different heap object"),
            (16, "closure fallback found a different heap object"),
            (7, "cursor fallback found a different heap object"),
        ] {
            assert_fallback_heap_error(
                &program,
                BytecodeTypeId::new(ty),
                HeapObject::String("wrong".into()),
                expected,
            );
        }
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(4),
            HeapObject::Tuple(vec![None]),
            "tuple fallback found the wrong arity",
        );
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(14),
            HeapObject::Record {
                nominal: BytecodeNominalId::new(1),
                fields: vec![(999, None)],
            },
            "record fallback found an unknown field",
        );
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(15),
            HeapObject::Variant {
                variant: 999,
                payload: AggregatePayload::Unit,
            },
            "enum fallback found an unknown variant",
        );
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(15),
            HeapObject::Variant {
                variant: 1,
                payload: AggregatePayload::Unit,
            },
            "enum fallback found the wrong payload shape",
        );
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(16),
            HeapObject::Closure {
                callable: BytecodeCallableId::new(0),
                captures: vec![None],
            },
            "closure fallback found the wrong capture arity",
        );
        assert_fallback_heap_error(
            &program,
            BytecodeTypeId::new(7),
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Ref,
                source: None,
                next: 0,
                adapter: None,
            },
            "cursor fallback found a ref iterator",
        );
    }

    #[test]
    fn terminal_process_fallback_invokes_the_host_cleanup_boundary() {
        #[derive(Default)]
        struct CleanupHost {
            cleaned: Vec<RuntimeValue>,
        }

        impl VmHost for CleanupHost {
            fn invoke(
                &mut self,
                name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                Err(VmError::UnsupportedHostCall(name.into()))
            }

            fn cleanup(&mut self, value: &RuntimeValue) -> Result<(), VmError> {
                self.cleaned.push(value.clone());
                Ok(())
            }
        }

        let program = terminal_fallback_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = CleanupHost::default();
        {
            let mut engine = Engine::new(
                &program,
                &mut host,
                pressure_limits(),
                ValueCopyStrategy::default(),
                trace,
            );
            let fallback = install_fallback_frame(
                &mut engine,
                BytecodeTypeId::new(18),
                Value::Host(RuntimeValue::String("process".into())),
            );
            engine.execute_terminal_fallback(0, fallback).unwrap();
        }
        assert_eq!(host.cleaned, [RuntimeValue::String("process".into())]);
    }

    #[test]
    fn default_host_and_closed_runtime_helpers_have_explicit_boundaries() {
        #[derive(Default)]
        struct MinimalHost;

        impl VmHost for MinimalHost {
            fn invoke(
                &mut self,
                name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                Err(VmError::UnsupportedHostCall(name.into()))
            }
        }

        let mut host = MinimalHost;
        assert!(matches!(
            host.start_async("work", &[]),
            Err(VmError::UnsupportedHostCall(name)) if name == "work"
        ));
        assert_eq!(host.poll_async(7).unwrap(), None);
        assert!(matches!(host.wait_async(&[]), Err(VmError::Invariant(_))));
        assert!(matches!(
            host.wait_async(&[7]),
            Err(VmError::UnsupportedHostCall(name)) if name == "async host call #7"
        ));
        host.cancel_async(7).unwrap();
        host.cleanup(&RuntimeValue::Unit).unwrap();
        assert!(matches!(
            host.begin_virtual_time(),
            Err(VmError::UnsupportedHostCall(name)) if name == "std.testing.withVirtualTime"
        ));
        assert!(matches!(
            host.finish_virtual_time(&RuntimeValue::Unit),
            Err(VmError::UnsupportedHostCall(name)) if name == "std.testing.withVirtualTime"
        ));
        assert!(!host.is_virtual_quiescence_call(7));
        assert!(matches!(
            host.begin_test_node(VmTestNodeKind::Leaf, "unit::leaf"),
            Err(VmError::UnsupportedHostCall(name)) if name == "test node `unit::leaf`"
        ));
        assert!(matches!(
            host.finish_test_node(
                VmTestNodeKind::Suite,
                "unit::suite",
                VmTestNodeOutcome::Passed,
            ),
            Err(VmError::UnsupportedHostCall(name)) if name == "test node `unit::suite`"
        ));
        assert!(matches!(
            host.begin_test_suite_cleanup(),
            Err(VmError::UnsupportedHostCall(name)) if name == "test suite cleanup"
        ));

        let mut rejecting = RejectingHost;
        assert!(matches!(
            rejecting.invoke("missing", &[]),
            Err(VmError::UnsupportedHostCall(name)) if name == "missing"
        ));

        for (constructor, expected) in [
            (
                BytecodeIntrinsicType::Command,
                RuntimeHostValueKind::Command,
            ),
            (
                BytecodeIntrinsicType::Pipeline,
                RuntimeHostValueKind::Pipeline,
            ),
            (BytecodeIntrinsicType::Bytes, RuntimeHostValueKind::Bytes),
            (
                BytecodeIntrinsicType::BytesBuilder,
                RuntimeHostValueKind::BytesBuilder,
            ),
            (
                BytecodeIntrinsicType::BytesError,
                RuntimeHostValueKind::BytesError,
            ),
            (
                BytecodeIntrinsicType::ExitStatus,
                RuntimeHostValueKind::ExitStatus,
            ),
            (
                BytecodeIntrinsicType::ProcessOutput,
                RuntimeHostValueKind::ProcessOutput,
            ),
            (
                BytecodeIntrinsicType::ProcessHandle,
                RuntimeHostValueKind::ProcessHandle,
            ),
            (
                BytecodeIntrinsicType::ProcessError,
                RuntimeHostValueKind::ProcessError,
            ),
            (
                BytecodeIntrinsicType::ProcessExitError,
                RuntimeHostValueKind::ProcessExitError,
            ),
            (
                BytecodeIntrinsicType::Utf8Error,
                RuntimeHostValueKind::Utf8Error,
            ),
            (
                BytecodeIntrinsicType::Instant,
                RuntimeHostValueKind::Instant,
            ),
            (BytecodeIntrinsicType::Timer, RuntimeHostValueKind::Timer),
            (
                BytecodeIntrinsicType::DurationError,
                RuntimeHostValueKind::DurationError,
            ),
            (
                BytecodeIntrinsicType::ClockError,
                RuntimeHostValueKind::ClockError,
            ),
            (
                BytecodeIntrinsicType::EnvSnapshot,
                RuntimeHostValueKind::EnvSnapshot,
            ),
            (
                BytecodeIntrinsicType::EnvName,
                RuntimeHostValueKind::EnvName,
            ),
            (
                BytecodeIntrinsicType::EnvValue,
                RuntimeHostValueKind::EnvValue,
            ),
            (
                BytecodeIntrinsicType::EnvError,
                RuntimeHostValueKind::EnvError,
            ),
        ] {
            assert_eq!(runtime_host_kind(constructor), Some(expected));
        }
        for constructor in [
            BytecodeIntrinsicType::Array,
            BytecodeIntrinsicType::Map,
            BytecodeIntrinsicType::Set,
            BytecodeIntrinsicType::Range,
            BytecodeIntrinsicType::Ref,
            BytecodeIntrinsicType::Pointer,
            BytecodeIntrinsicType::Join,
            BytecodeIntrinsicType::Duration,
            BytecodeIntrinsicType::NumericConversionError,
        ] {
            assert_eq!(runtime_host_kind(constructor), None);
        }

        assert_eq!(next_unicode_scalar(0x41), Some(0x42));
        assert_eq!(next_unicode_scalar(0xd7ff), Some(0xe000));
        assert_eq!(next_unicode_scalar(0x10ffff), None);
        assert_eq!(next_unicode_scalar(u32::MAX), None);
        assert!(collection_length_fits_int(0));
        assert!(!collection_length_fits_int(usize::MAX));

        let failure = PlaceFailure::from(VmError::invariant("closed"));
        assert!(matches!(
            failure,
            PlaceFailure::Vm(VmError::Invariant(message)) if message == "closed"
        ));
    }

    #[test]
    fn format_builder_host_boundaries_are_materialized_atomically() {
        #[derive(Clone, Copy)]
        enum Mode {
            Success,
            InvalidBuilder,
            InvalidAppend,
            AppendError,
            InvalidFinish,
        }

        struct FormatHost {
            mode: Mode,
        }

        impl VmHost for FormatHost {
            fn invoke(
                &mut self,
                name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                match (name, self.mode) {
                    ("std.format.Builder.new", Mode::InvalidBuilder) => {
                        Ok(RuntimeValue::String("wrong".into()))
                    }
                    ("std.format.Builder.new", _) => Ok(RuntimeValue::Host {
                        kind: RuntimeHostValueKind::FormatBuilder,
                        id: 7,
                    }),
                    ("std.format.Builder.append", Mode::InvalidAppend) => Ok(RuntimeValue::Unit),
                    ("std.format.Builder.append", Mode::AppendError) => {
                        Ok(RuntimeValue::ResultErr(Box::new(RuntimeValue::Host {
                            kind: RuntimeHostValueKind::FormatError,
                            id: 8,
                        })))
                    }
                    ("std.format.Builder.append", _) => {
                        Ok(RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)))
                    }
                    ("std.format.Builder.finish", Mode::InvalidFinish) => Ok(RuntimeValue::Unit),
                    ("std.format.Builder.finish", _) => Ok(RuntimeValue::String("joined".into())),
                    _ => Err(VmError::UnsupportedHostCall(name.into())),
                }
            }
        }

        let mut program = root_pressure_program();
        let format_error = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "FormatError".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::FormatError,
                arguments: Vec::new(),
            },
        });
        let result = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "String ! FormatError".into(),
            kind: BytecodeTypeKind::Result {
                success: BytecodeTypeId::new(0),
                error: format_error,
            },
        });
        let trace = derive_trace_metadata(&program).unwrap();

        for (mode, result_ty, expected_error) in [
            (Mode::Success, BytecodeTypeId::new(0), None),
            (
                Mode::InvalidBuilder,
                BytecodeTypeId::new(0),
                Some("non-builder"),
            ),
            (
                Mode::InvalidAppend,
                BytecodeTypeId::new(0),
                Some("invalid result"),
            ),
            (Mode::AppendError, result, None),
            (
                Mode::InvalidFinish,
                BytecodeTypeId::new(0),
                Some("bootstrap host result"),
            ),
        ] {
            let mut host = FormatHost { mode };
            let mut engine = Engine::new(
                &program,
                &mut host,
                pressure_limits(),
                ValueCopyStrategy::default(),
                trace.clone(),
            );
            let outcome = engine.format_text_result(result_ty, ["a".to_owned(), "b".to_owned()]);
            if let Some(expected_error) = expected_error {
                assert!(
                    matches!(&outcome, Err(VmError::Host(message)) if message.contains(expected_error))
                        || matches!(&outcome, Err(VmError::Invariant(message)) if message.contains(expected_error)),
                    "unexpected format boundary result: {outcome:?}"
                );
            } else {
                let value = outcome.unwrap();
                if matches!(mode, Mode::Success) {
                    assert_eq!(engine.string_value(&value).unwrap(), "joined");
                } else {
                    let handle = value.heap_handle().expect("result is heap allocated");
                    assert!(matches!(
                        engine.heap.get(handle),
                        Ok(HeapObject::ResultErr(Some(Value::Host(
                            RuntimeValue::Host {
                                kind: RuntimeHostValueKind::FormatError,
                                ..
                            }
                        ))))
                    ));
                }
            }
        }
    }

    #[test]
    fn virtual_time_boundary_restores_the_host_after_malformed_callback_values() {
        #[derive(Default)]
        struct BoundaryHost {
            begun: usize,
            finished: usize,
            invoked: usize,
        }

        impl VmHost for BoundaryHost {
            fn invoke(
                &mut self,
                _name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                self.invoked += 1;
                Ok(RuntimeValue::Unit)
            }

            fn begin_virtual_time(&mut self) -> Result<RuntimeValue, VmError> {
                self.begun += 1;
                Ok(RuntimeValue::Host {
                    kind: RuntimeHostValueKind::VirtualTime,
                    id: 1,
                })
            }

            fn finish_virtual_time(&mut self, _controller: &RuntimeValue) -> Result<(), VmError> {
                self.finished += 1;
                Ok(())
            }
        }

        let mut program = root_pressure_program();
        let int = BytecodeTypeId::new(5);
        let unit = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Unit".into(),
            kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
        });
        let virtual_time = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "VirtualTime".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::VirtualTime,
                arguments: Vec::new(),
            },
        });
        let body_function = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "fn(ref VirtualTime): Unit".into(),
            kind: BytecodeTypeKind::Function(BytecodeFunctionType {
                is_async: false,
                is_unsafe: false,
                parameters: vec![BytecodeFunctionParameter {
                    mode: BytecodeParameterMode::Ref,
                    ty: virtual_time,
                }],
                variadic: None,
                outcome: unit,
            }),
        });
        let parameter = BytecodeParameter {
            mode: BytecodeParameterMode::Value,
            ty: int,
            variadic_element: None,
            receiver: false,
        };
        program.callables.extend([
            BytecodeCallable {
                name: "std.testing.withVirtualTime".into(),
                generic_arity: 0,
                parameters: vec![parameter],
                outcome: int,
                function_type: body_function,
                implementation: None,
                closure: None,
            },
            BytecodeCallable {
                name: "host.body".into(),
                generic_arity: 0,
                parameters: vec![BytecodeParameter {
                    mode: BytecodeParameterMode::Ref,
                    ty: virtual_time,
                    variadic_element: None,
                    receiver: false,
                }],
                outcome: unit,
                function_type: body_function,
                implementation: None,
                closure: None,
            },
        ]);
        let trace = derive_trace_metadata(&program).unwrap();
        let boundary = Value::Function {
            callable: BytecodeCallableId::new(0),
            arguments: Vec::new(),
        };

        for (body, invoked) in [
            (Value::Integer(1), 0),
            (
                Value::Function {
                    callable: BytecodeCallableId::new(1),
                    arguments: Vec::new(),
                },
                1,
            ),
        ] {
            let mut host = BoundaryHost::default();
            let error = {
                let mut engine = Engine::new(
                    &program,
                    &mut host,
                    pressure_limits(),
                    ValueCopyStrategy::default(),
                    trace.clone(),
                );
                match engine.prepare_evaluated_call(
                    boundary.clone(),
                    vec![(BytecodeCallArgumentTarget::Fixed(0), body)],
                ) {
                    Err(error) => error,
                    Ok(_) => panic!("malformed virtual-time callback was accepted"),
                }
            };
            assert!(matches!(error, VmError::Invariant(_)));
            assert_eq!((host.begun, host.finished, host.invoked), (1, 1, invoked));
        }

        let mut malformed = program.clone();
        malformed.callables[0].parameters.clear();
        let mut host = BoundaryHost::default();
        let trace = derive_trace_metadata(&malformed).unwrap();
        let result = Engine::new(
            &malformed,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        )
        .prepare_evaluated_call(boundary, Vec::new());
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("malformed virtual-time boundary was accepted"),
        };
        assert!(
            matches!(error, VmError::Invariant(message) if message.contains("one body operand"))
        );
        assert_eq!((host.begun, host.finished, host.invoked), (0, 0, 0));
    }

    #[test]
    fn host_materialization_accepts_every_supported_shape_and_rejects_drift() {
        let mut program = root_pressure_program();
        let append = |program: &mut BytecodeProgram, name: &str, kind: BytecodeTypeKind| {
            let id = BytecodeTypeId::new(program.types.len() as u32);
            program.types.push(BytecodeType {
                name: name.into(),
                kind,
            });
            id
        };
        let unit = append(
            &mut program,
            "Unit",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Unit),
        );
        let boolean = append(
            &mut program,
            "Bool",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Bool),
        );
        let float = append(
            &mut program,
            "Float",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Float),
        );
        let float32 = append(
            &mut program,
            "Float32",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Float32),
        );
        let byte = append(
            &mut program,
            "Byte",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Byte),
        );
        let character = append(
            &mut program,
            "Char",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Char),
        );
        let option = append(
            &mut program,
            "Int?",
            BytecodeTypeKind::Option(BytecodeTypeId::new(5)),
        );
        let result = append(
            &mut program,
            "Int ! String",
            BytecodeTypeKind::Result {
                success: BytecodeTypeId::new(5),
                error: BytecodeTypeId::new(0),
            },
        );
        let union = append(
            &mut program,
            "Int | String",
            BytecodeTypeKind::Union(vec![BytecodeTypeId::new(5), BytecodeTypeId::new(0)]),
        );
        let opaque = append(
            &mut program,
            "impl Copy",
            BytecodeTypeKind::OpaqueResult {
                identity: "test::opaque".into(),
                arguments: Vec::new(),
                witness: BytecodeTypeId::new(5),
                capabilities: BytecodeCapabilitySet {
                    copy: true,
                    discard: true,
                    equatable: true,
                    key: true,
                    send: true,
                    share: true,
                },
            },
        );
        let scalars = append(
            &mut program,
            "(Unit, Bool, Int, Float, Byte, Char)",
            BytecodeTypeKind::Tuple(vec![
                unit,
                boolean,
                BytecodeTypeId::new(5),
                float,
                byte,
                character,
            ]),
        );

        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );

        for (ty, runtime, expected) in [
            (unit, RuntimeValue::Unit, Value::Unit),
            (boolean, RuntimeValue::Bool(true), Value::Bool(true)),
            (
                BytecodeTypeId::new(5),
                RuntimeValue::Integer(42),
                Value::Integer(42),
            ),
            (float, RuntimeValue::Float(1.5), Value::Float(1.5)),
            (
                float32,
                RuntimeValue::Float(16_777_217.0),
                Value::Float(16_777_216.0),
            ),
            (byte, RuntimeValue::Byte(255), Value::Byte(255)),
            (character, RuntimeValue::Char('λ'), Value::Char('λ')),
        ] {
            assert_eq!(
                engine.materialize_host_value(ty, runtime).unwrap(),
                expected
            );
        }

        let tuple = RuntimeValue::Tuple(vec![
            RuntimeValue::Unit,
            RuntimeValue::Bool(true),
            RuntimeValue::Integer(7),
            RuntimeValue::Float(2.5),
            RuntimeValue::Byte(8),
            RuntimeValue::Char('x'),
        ]);
        let value = engine
            .materialize_host_value(scalars, tuple.clone())
            .unwrap();
        assert_eq!(
            snapshot_value(
                &value,
                &engine.heap,
                &engine.callable_names,
                &engine.nominal_names,
            )
            .unwrap(),
            tuple
        );

        for (runtime, expected) in [
            (RuntimeValue::OptionNone, RuntimeValue::OptionNone),
            (
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(9))),
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(9))),
            ),
        ] {
            let value = engine.materialize_host_value(option, runtime).unwrap();
            assert_eq!(
                snapshot_value(
                    &value,
                    &engine.heap,
                    &engine.callable_names,
                    &engine.nominal_names,
                )
                .unwrap(),
                expected
            );
        }
        for runtime in [
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(10))),
            RuntimeValue::ResultErr(Box::new(RuntimeValue::String("error".into()))),
        ] {
            let value = engine
                .materialize_host_value(result, runtime.clone())
                .unwrap();
            assert_eq!(
                snapshot_value(
                    &value,
                    &engine.heap,
                    &engine.callable_names,
                    &engine.nominal_names,
                )
                .unwrap(),
                runtime
            );
        }

        let selected = RuntimeValue::Union {
            member: 5,
            value: Box::new(RuntimeValue::Integer(11)),
        };
        let value = engine
            .materialize_host_value(union, selected.clone())
            .unwrap();
        assert_eq!(
            snapshot_value(
                &value,
                &engine.heap,
                &engine.callable_names,
                &engine.nominal_names,
            )
            .unwrap(),
            selected
        );
        assert_eq!(
            engine
                .materialize_host_value(opaque, RuntimeValue::Integer(12))
                .unwrap(),
            Value::Integer(12)
        );

        assert!(matches!(
            engine.materialize_host_value(boolean, RuntimeValue::Integer(1)),
            Err(VmError::Host(_))
        ));
        assert!(matches!(
            engine.materialize_host_value(
                union,
                RuntimeValue::Union {
                    member: u32::MAX,
                    value: Box::new(RuntimeValue::Integer(1)),
                },
            ),
            Err(VmError::Host(_))
        ));
        assert!(matches!(
            engine.materialize_host_value(scalars, RuntimeValue::Tuple(vec![RuntimeValue::Unit]),),
            Err(VmError::Host(_))
        ));
        assert!(matches!(
            engine.value_tag(&Value::Integer(1)),
            Err(VmError::Invariant(_))
        ));
        let untagged = engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("untagged".into()),
                &[],
            )
            .unwrap();
        assert!(matches!(
            engine.value_tag(&untagged),
            Err(VmError::Invariant(_))
        ));
    }

    #[test]
    fn numeric_conversion_and_structural_equality_helpers_are_closed() {
        use BytecodeNumericConversionError::{NotFinite, NotIntegral, OutOfRange};

        assert_eq!(
            convert_numeric(BytecodeScalarType::Bool, &Value::Integer(1)),
            Err(OutOfRange)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Byte, &Value::Byte(7)),
            Ok(Value::Byte(7))
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Byte, &Value::Float(255.0)),
            Ok(Value::Byte(255))
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Byte, &Value::Float(256.0)),
            Err(OutOfRange)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Int8, &Value::Float(42.0)),
            Ok(Value::Integer(42))
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Int8, &Value::Float(128.0)),
            Err(OutOfRange)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Bool, &Value::Float(1.0)),
            Err(OutOfRange)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Int, &Value::Float(f64::INFINITY)),
            Err(NotFinite)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Int, &Value::Float(1.5)),
            Err(NotIntegral)
        );
        assert_eq!(
            convert_numeric(BytecodeScalarType::Int, &Value::Unit),
            Err(OutOfRange)
        );

        let mut pending = Vec::new();
        assert!(
            !queue_object_equality(
                &super::HeapObject::Tuple(vec![Some(Value::Integer(1))]),
                &super::HeapObject::Tuple(Vec::new()),
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_object_equality(
                &super::HeapObject::Newtype {
                    nominal: BytecodeNominalId::new(0),
                    value: Some(Value::Integer(1)),
                },
                &super::HeapObject::Newtype {
                    nominal: BytecodeNominalId::new(1),
                    value: Some(Value::Integer(1)),
                },
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_object_equality(
                &super::HeapObject::Record {
                    nominal: BytecodeNominalId::new(0),
                    fields: vec![(0, Some(Value::Integer(1)))],
                },
                &super::HeapObject::Record {
                    nominal: BytecodeNominalId::new(0),
                    fields: vec![(1, Some(Value::Integer(1)))],
                },
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_object_equality(
                &super::HeapObject::Range {
                    kind: BytecodeRangeKind::Exclusive,
                    start: Some(Value::Integer(1)),
                    end: Some(Value::Integer(2)),
                },
                &super::HeapObject::Range {
                    kind: BytecodeRangeKind::Inclusive,
                    start: Some(Value::Integer(1)),
                    end: Some(Value::Integer(2)),
                },
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            queue_object_equality(
                &super::HeapObject::Range {
                    kind: BytecodeRangeKind::Inclusive,
                    start: Some(Value::Integer(1)),
                    end: Some(Value::Integer(2)),
                },
                &super::HeapObject::Range {
                    kind: BytecodeRangeKind::Inclusive,
                    start: Some(Value::Integer(1)),
                    end: Some(Value::Integer(2)),
                },
                &mut pending,
            )
            .unwrap()
        );
        assert!(matches!(
            queue_object_equality(
                &super::HeapObject::Closure {
                    callable: BytecodeCallableId::new(0),
                    captures: Vec::new(),
                },
                &super::HeapObject::Closure {
                    callable: BytecodeCallableId::new(0),
                    captures: Vec::new(),
                },
                &mut pending,
            ),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            queue_object_equality(
                &super::HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(Value::Integer(1)),
                    next: 0,
                    adapter: None,
                },
                &super::HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(Value::Integer(1)),
                    next: 0,
                    adapter: None,
                },
                &mut pending,
            ),
            Err(VmError::Invariant(_))
        ));
        assert!(
            !queue_object_equality(
                &super::HeapObject::OptionNone,
                &super::HeapObject::String("different".into()),
                &mut pending,
            )
            .unwrap()
        );

        assert!(
            !queue_payload_equality(
                &AggregatePayload::Tuple(vec![Some(Value::Integer(1))]),
                &AggregatePayload::Tuple(Vec::new()),
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_payload_equality(
                &AggregatePayload::Record(vec![(0, Some(Value::Integer(1)))]),
                &AggregatePayload::Record(vec![(1, Some(Value::Integer(1)))]),
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_payload_equality(
                &AggregatePayload::Unit,
                &AggregatePayload::Tuple(Vec::new()),
                &mut pending,
            )
            .unwrap()
        );

        macro_rules! equal_object {
            ($object:expr) => {{
                let object = $object;
                let clone = object.clone();
                assert!(queue_object_equality(&object, &clone, &mut pending).unwrap());
            }};
        }
        equal_object!(super::HeapObject::String("same".into()));
        equal_object!(super::HeapObject::Tuple(vec![Some(Value::Integer(1))]));
        equal_object!(super::HeapObject::Array(
            vec![Some(Value::Integer(1))].into()
        ));
        equal_object!(super::HeapObject::Newtype {
            nominal: BytecodeNominalId::new(0),
            value: Some(Value::Integer(1)),
        });
        equal_object!(super::HeapObject::Record {
            nominal: BytecodeNominalId::new(0),
            fields: vec![(0, Some(Value::Integer(1)))],
        });
        for payload in [
            AggregatePayload::Unit,
            AggregatePayload::Tuple(vec![Some(Value::Integer(1))]),
            AggregatePayload::Record(vec![(0, Some(Value::Integer(1)))]),
        ] {
            equal_object!(super::HeapObject::Variant {
                variant: 0,
                payload,
            });
        }
        equal_object!(super::HeapObject::OptionNone);
        equal_object!(super::HeapObject::OptionSome(Some(Value::Integer(1))));
        equal_object!(super::HeapObject::ResultOk(Some(Value::Integer(1))));
        equal_object!(super::HeapObject::ResultErr(Some(Value::Integer(1))));
        equal_object!(super::HeapObject::Union {
            member: BytecodeTypeId::new(0),
            value: Some(Value::Integer(1)),
        });
        assert!(
            !queue_object_equality(
                &super::HeapObject::Union {
                    member: BytecodeTypeId::new(0),
                    value: Some(Value::Integer(1)),
                },
                &super::HeapObject::Union {
                    member: BytecodeTypeId::new(1),
                    value: Some(Value::Integer(1)),
                },
                &mut pending,
            )
            .unwrap()
        );
        assert!(
            !queue_object_equality(
                &super::HeapObject::Ref(Some(Value::Integer(1))),
                &super::HeapObject::Ref(Some(Value::Integer(1))),
                &mut pending,
            )
            .unwrap()
        );
    }

    #[test]
    fn projection_and_storage_helper_boundaries_are_closed() {
        let ty = BytecodeTypeId::new(0);
        let slot = |index| BytecodeSlotId::new(index);
        let place = |index| BytecodePlace {
            slot: slot(index),
            ty,
            projections: Vec::new(),
            source_loan: None,
        };
        let operand = |kind| BytecodeOperand { ty, kind };
        for kind in [
            BytecodeOperandKind::Copy(place(1)),
            BytecodeOperandKind::Move(place(2)),
            BytecodeOperandKind::Borrow(place(3)),
        ] {
            let expected = match &kind {
                BytecodeOperandKind::Copy(place)
                | BytecodeOperandKind::Move(place)
                | BytecodeOperandKind::Borrow(place) => place.slot,
                _ => unreachable!(),
            };
            assert_eq!(operand_materialized_slot(&operand(kind)).unwrap(), expected);
        }
        assert!(matches!(
            operand_materialized_slot(&operand(BytecodeOperandKind::Constant(
                BytecodeConstant::Integer("0".into()),
            ))),
            Err(VmError::Invariant(_))
        ));
        let mut projected = place(1);
        projected
            .projections
            .push(crate::bytecode::BytecodeProjection {
                ty,
                kind: crate::bytecode::BytecodeProjectionKind::TupleField(0),
            });
        assert!(matches!(
            operand_materialized_slot(&operand(BytecodeOperandKind::Copy(projected))),
            Err(VmError::Invariant(_))
        ));
        let mut sourced = place(1);
        sourced.source_loan = Some(BytecodeLoanId::new(0));
        assert!(matches!(
            operand_materialized_slot(&operand(BytecodeOperandKind::Copy(sourced))),
            Err(VmError::Invariant(_))
        ));

        let index = BytecodeOperation {
            ty,
            kind: BytecodeOperationKind::Index {
                base: operand(BytecodeOperandKind::Borrow(place(0))),
                index: operand(BytecodeOperandKind::Copy(place(1))),
                access: BytecodeIndexAccess::Array,
                against: Vec::new(),
            },
        };
        let access = operation_access_place(&index).unwrap().unwrap();
        assert!(matches!(
            access.projections.last().map(|projection| &projection.kind),
            Some(crate::bytecode::BytecodeProjectionKind::Index { index, .. })
                if *index == slot(1)
        ));

        let slice = BytecodeOperation {
            ty,
            kind: BytecodeOperationKind::Slice {
                base: operand(BytecodeOperandKind::Borrow(place(0))),
                bounds: Box::new(BytecodeSliceBounds {
                    start: Some(operand(BytecodeOperandKind::Copy(place(1)))),
                    end: Some(operand(BytecodeOperandKind::Move(place(2)))),
                    step: Some(operand(BytecodeOperandKind::Borrow(place(3)))),
                }),
                against: Vec::new(),
            },
        };
        let access = operation_access_place(&slice).unwrap().unwrap();
        assert!(matches!(
            access.projections.last().map(|projection| &projection.kind),
            Some(crate::bytecode::BytecodeProjectionKind::Slice {
                start: Some(start),
                end: Some(end),
                step: Some(step),
            }) if *start == slot(1) && *end == slot(2) && *step == slot(3)
        ));

        let mut invalid_base = index.clone();
        let BytecodeOperationKind::Index { base, .. } = &mut invalid_base.kind else {
            unreachable!()
        };
        base.kind = BytecodeOperandKind::Copy(place(0));
        assert!(matches!(
            operation_access_place(&invalid_base),
            Err(VmError::Invariant(_))
        ));
        let mut invalid_index = index.clone();
        let BytecodeOperationKind::Index { index, .. } = &mut invalid_index.kind else {
            unreachable!()
        };
        index.kind = BytecodeOperandKind::Constant(BytecodeConstant::Integer("0".into()));
        assert!(matches!(
            operation_access_place(&invalid_index),
            Err(VmError::Invariant(_))
        ));
        let unrelated = BytecodeOperation {
            ty,
            kind: BytecodeOperationKind::ExplicitPanic {
                message: operand(BytecodeOperandKind::Constant(BytecodeConstant::String(
                    "\"stop\"".into(),
                ))),
            },
        };
        assert_eq!(operation_access_place(&unrelated).unwrap(), None);

        let path = |root, components| ResolvedPlacePath { root, components };
        let root = (0, 1, 2);
        assert!(paths_overlap(
            &path(root, Vec::new()),
            &path(root, Vec::new())
        ));
        assert!(!paths_overlap(
            &path(root, Vec::new()),
            &path((1, 1, 2), Vec::new())
        ));
        assert!(!paths_overlap(
            &path(root, vec![PlaceComponent::Field(0)]),
            &path(root, vec![PlaceComponent::Field(1)])
        ));
        assert!(paths_overlap(
            &path(root, vec![PlaceComponent::Slice(vec![1, 3])]),
            &path(root, vec![PlaceComponent::Slice(vec![2, 3])])
        ));
        assert!(!paths_overlap(
            &path(root, vec![PlaceComponent::Slice(vec![1, 3])]),
            &path(root, vec![PlaceComponent::Slice(vec![2, 4])])
        ));
        assert!(paths_overlap(
            &path(root, vec![PlaceComponent::Slice(vec![2])]),
            &path(root, vec![PlaceComponent::Index(2)])
        ));
        assert!(paths_overlap(
            &path(root, vec![PlaceComponent::Index(2)]),
            &path(root, vec![PlaceComponent::Slice(vec![2])])
        ));
        assert!(!paths_overlap(
            &path(root, vec![PlaceComponent::Index(-1)]),
            &path(root, vec![PlaceComponent::Slice(vec![usize::MAX])])
        ));
        assert!(!paths_overlap(
            &path(
                root,
                vec![PlaceComponent::MapKey(RuntimeValue::String("a".into()))]
            ),
            &path(
                root,
                vec![PlaceComponent::MapKey(RuntimeValue::String("b".into()))]
            )
        ));

        assert_eq!(slice_indices(None, None, None, 3).unwrap(), [0, 1, 2]);
        assert_eq!(
            slice_indices(Some(-1), None, Some(-1), 3).unwrap(),
            [2, 1, 0]
        );
        assert!(matches!(
            slice_indices(None, None, Some(0), 3),
            Err((PanicCode::ZeroSliceStep, _))
        ));

        let mut values = vec![Some(Value::Integer(1)), None];
        assert_eq!(
            clone_present(&values[0], "value").unwrap(),
            Value::Integer(1)
        );
        assert_eq!(clone_index(&values, 0, "item").unwrap(), Value::Integer(1));
        assert!(clone_index(&values, 1, "item").is_err());
        assert!(clone_index(&values, 2, "item").is_err());
        assert_eq!(
            take_index(&mut values, 0, "item").unwrap(),
            Value::Integer(1)
        );
        assert!(take_index(&mut values, 0, "item").is_err());
        set_index(&mut values, 1, Value::Integer(2), "item").unwrap();
        assert!(set_index(&mut values, 2, Value::Integer(3), "item").is_err());

        let mut fields = vec![(7, Some(Value::Bool(true))), (8, None)];
        assert_eq!(clone_field(&fields, 7, "field").unwrap(), Value::Bool(true));
        assert!(clone_field(&fields, 8, "field").is_err());
        assert!(clone_field(&fields, 9, "field").is_err());
        assert_eq!(
            take_field(&mut fields, 7, "field").unwrap(),
            Value::Bool(true)
        );
        assert!(take_field(&mut fields, 7, "field").is_err());
        set_field(&mut fields, 8, Value::Bool(false)).unwrap();
        assert!(set_field(&mut fields, 9, Value::Bool(false)).is_err());

        let mut optional = Some(Value::Char('T'));
        assert_eq!(present(&optional, "value").unwrap(), &Value::Char('T'));
        assert_eq!(
            take_option(&mut optional, "value").unwrap(),
            Value::Char('T')
        );
        assert!(present(&optional, "value").is_err());
        assert!(take_option(&mut optional, "value").is_err());

        for (scalar, expected) in [
            (BytecodeScalarType::Int, Some((true, 64))),
            (BytecodeScalarType::Int8, Some((true, 8))),
            (BytecodeScalarType::Int16, Some((true, 16))),
            (BytecodeScalarType::Int32, Some((true, 32))),
            (BytecodeScalarType::UInt8, Some((false, 8))),
            (BytecodeScalarType::UInt16, Some((false, 16))),
            (BytecodeScalarType::UInt32, Some((false, 32))),
            (BytecodeScalarType::UInt64, Some((false, 64))),
            (BytecodeScalarType::Float, None),
        ] {
            assert_eq!(integer_shape(scalar), expected);
            assert_eq!(integer_bounds(scalar).is_some(), expected.is_some());
        }
    }

    #[test]
    fn scope_join_search_traverses_every_managed_shape_and_terminates_on_cycles() {
        let mut program = terminal_fallback_program();
        let reference = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Ref[String]".into(),
            kind: BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Ref,
                arguments: vec![BytecodeTypeId::new(0)],
            },
        });
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );
        let join = Value::Join(RuntimeJoin { task: 3, scope: 7 });

        assert!(
            engine
                .value_contains_scope_join(&join, 7, &mut Default::default())
                .unwrap()
        );
        assert!(
            !engine
                .value_contains_scope_join(&join, 8, &mut Default::default())
                .unwrap()
        );
        assert!(
            !engine
                .any_value_contains_scope_join(
                    [Value::Integer(1), Value::Bool(false)].iter(),
                    7,
                    &mut Default::default(),
                )
                .unwrap()
        );

        macro_rules! assert_nested_join {
            ($ty:expr, $object:expr) => {{
                let value = engine
                    .allocate($ty, $object, std::slice::from_ref(&join))
                    .unwrap();
                assert!(
                    engine
                        .value_contains_scope_join(&value, 7, &mut Default::default())
                        .unwrap()
                );
                assert!(
                    !engine
                        .value_contains_scope_join(&value, 8, &mut Default::default())
                        .unwrap()
                );
            }};
        }

        assert_nested_join!(
            BytecodeTypeId::new(4),
            HeapObject::Tuple(vec![None, Some(join.clone())])
        );
        assert_nested_join!(
            BytecodeTypeId::new(1),
            HeapObject::Array(vec![None, Some(join.clone())].into())
        );
        assert_nested_join!(
            BytecodeTypeId::new(8),
            HeapObject::Set(vec![Some(join.clone())].into())
        );
        assert_nested_join!(
            BytecodeTypeId::new(2),
            HeapObject::Map(vec![(None, Some(join.clone()))].into())
        );
        assert_nested_join!(
            BytecodeTypeId::new(16),
            HeapObject::Closure {
                callable: BytecodeCallableId::new(0),
                captures: vec![None, Some(join.clone())],
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(13),
            HeapObject::Newtype {
                nominal: BytecodeNominalId::new(0),
                value: Some(join.clone()),
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(9),
            HeapObject::OptionSome(Some(join.clone()))
        );
        assert_nested_join!(
            BytecodeTypeId::new(10),
            HeapObject::ResultOk(Some(join.clone()))
        );
        assert_nested_join!(
            BytecodeTypeId::new(10),
            HeapObject::ResultErr(Some(join.clone()))
        );
        assert_nested_join!(
            BytecodeTypeId::new(11),
            HeapObject::Union {
                member: BytecodeTypeId::new(0),
                value: Some(join.clone()),
            }
        );
        assert_nested_join!(reference, HeapObject::Ref(Some(join.clone())));
        assert_nested_join!(
            BytecodeTypeId::new(17),
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Ref,
                source: Some(join.clone()),
                next: 0,
                adapter: None,
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(14),
            HeapObject::Record {
                nominal: BytecodeNominalId::new(1),
                fields: vec![(0, None), (1, Some(join.clone()))],
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(15),
            HeapObject::Variant {
                variant: 1,
                payload: AggregatePayload::Tuple(vec![Some(join.clone())]),
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(15),
            HeapObject::Variant {
                variant: 2,
                payload: AggregatePayload::Record(vec![(0, Some(join.clone()))]),
            }
        );
        assert_nested_join!(
            BytecodeTypeId::new(3),
            HeapObject::Range {
                kind: BytecodeRangeKind::Inclusive,
                start: None,
                end: Some(join.clone()),
            }
        );

        for (ty, object) in [
            (
                BytecodeTypeId::new(0),
                HeapObject::String("join-free".into()),
            ),
            (BytecodeTypeId::new(9), HeapObject::OptionNone),
            (
                BytecodeTypeId::new(15),
                HeapObject::Variant {
                    variant: 0,
                    payload: AggregatePayload::Unit,
                },
            ),
            (
                BytecodeTypeId::new(17),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Ref,
                    source: None,
                    next: usize::MAX,
                    adapter: None,
                },
            ),
        ] {
            let value = engine.allocate(ty, object, &[]).unwrap();
            assert!(
                !engine
                    .value_contains_scope_join(&value, 7, &mut Default::default())
                    .unwrap()
            );
        }

        let cycle = engine
            .allocate(BytecodeTypeId::new(9), HeapObject::OptionNone, &[])
            .unwrap();
        let Value::Heap(cycle_handle) = cycle else {
            unreachable!()
        };
        engine
            .replace_object(
                cycle_handle,
                HeapObject::OptionSome(Some(Value::Heap(cycle_handle))),
                &[],
            )
            .unwrap();
        assert!(
            !engine
                .value_contains_scope_join(&cycle, 7, &mut Default::default())
                .unwrap()
        );
    }

    #[test]
    fn cooperative_wakeups_are_idempotent_and_preserve_progress() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );
        engine.tasks.push(scheduler_task(TaskStatus::Running));
        engine.tasks.push(scheduler_task(TaskStatus::Runnable));

        engine.park_current(TaskWait::Scope, &[1, 1]).unwrap();
        assert_eq!(engine.tasks[1].waiters, [0]);

        engine.wake_task(0).unwrap();
        engine.wake_task(0).unwrap();
        engine.enqueue_task(0).unwrap();
        assert_eq!(engine.runnable.iter().copied().collect::<Vec<_>>(), [0]);
        assert!(engine.tasks[0].queued);

        assert!(engine.schedule_next().unwrap().is_none());
        assert_eq!(engine.current_task, 0);
        assert!(matches!(engine.tasks[0].status, TaskStatus::Running));
        assert!(!engine.tasks[0].queued);
        assert!(engine.runnable.is_empty());
        assert!(engine.resume_current_task().unwrap());
        assert!(engine.tasks[0].resume.is_none());
    }

    #[test]
    fn runnable_tasks_poll_host_completions_without_blocking_the_executor() {
        #[derive(Default)]
        struct ReadyHost {
            polls: usize,
        }

        impl VmHost for ReadyHost {
            fn invoke(
                &mut self,
                name: &str,
                _arguments: &[RuntimeValue],
            ) -> Result<RuntimeValue, VmError> {
                Err(VmError::UnsupportedHostCall(name.into()))
            }

            fn poll_async(&mut self, call: u64) -> Result<Option<RuntimeValue>, VmError> {
                self.polls += 1;
                Ok((call == 7).then_some(RuntimeValue::Integer(42)))
            }

            fn wait_async(&mut self, _calls: &[u64]) -> Result<(u64, RuntimeValue), VmError> {
                panic!("the host must not block while a language task is runnable")
            }
        }

        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = ReadyHost::default();
        {
            let mut engine = Engine::new(
                &program,
                &mut host,
                VmLimits::default(),
                ValueCopyStrategy::default(),
                trace,
            );
            engine.tasks.push(scheduler_task(TaskStatus::Running));
            engine
                .tasks
                .push(scheduler_task(TaskStatus::Waiting(TaskWait::HostTask {
                    call: 7,
                    outcome: BytecodeTypeId::new(5),
                })));

            assert!(engine.schedule_next().unwrap().is_none());
            assert!(matches!(
                engine.tasks[1].status,
                TaskStatus::Complete(Some(TaskCompletion::Returned(Value::Integer(42))))
            ));
            assert_eq!(engine.current_task, 0);
        }
        assert_eq!(host.polls, 1);
    }

    #[test]
    fn float32_rounds_each_operation_and_preserves_ieee_special_values() {
        use BytecodeBinaryOperator::{Add, Divide, Equal, Less, Multiply, NotEqual};

        fn binary(
            engine: &mut Engine<'_, '_>,
            operator: BytecodeBinaryOperator,
            ty: BytecodeTypeId,
            left: f64,
            right: f64,
        ) -> Value {
            engine
                .pure_binary(operator, ty, ty, Value::Float(left), Value::Float(right))
                .unwrap()
        }

        fn float(value: Value) -> f64 {
            let Value::Float(value) = value else {
                panic!("operation did not produce a float")
            };
            value
        }

        let mut program = root_pressure_program();
        let float32 = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Float32".into(),
            kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Float32),
        });
        let float64 = BytecodeTypeId::new(program.types.len() as u32);
        program.types.push(BytecodeType {
            name: "Float".into(),
            kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Float),
        });
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );

        assert_eq!(
            float(binary(&mut engine, Add, float32, 16_777_216.0, 1.0)),
            16_777_216.0
        );
        assert_eq!(
            float(binary(&mut engine, Add, float64, 16_777_216.0, 1.0)),
            16_777_217.0
        );

        let half_ulp = f64::from(f32::from_bits(0x3380_0000));
        assert_eq!(
            float(binary(&mut engine, Add, float32, 1.0, half_ulp)),
            f64::from(f32::from_bits(0x3f80_0000))
        );
        assert_eq!(
            float(binary(
                &mut engine,
                Add,
                float32,
                f64::from(f32::from_bits(0x3f80_0001)),
                half_ulp,
            )),
            f64::from(f32::from_bits(0x3f80_0002))
        );

        let factor = f64::from(f32::from_bits(0x3f80_0001));
        let product = float(binary(&mut engine, Multiply, float32, factor, factor));
        assert_eq!(product, f64::from(f32::from_bits(0x3f80_0002)));
        let separate = float(binary(
            &mut engine,
            Add,
            float32,
            product,
            f64::from(f32::from_bits(0xbf80_0002)),
        ));
        assert_eq!(separate.to_bits(), 0.0_f64.to_bits());

        let factor64 = f64::from_bits(0x3ff0_0000_0000_0001);
        let product64 = float(binary(&mut engine, Multiply, float64, factor64, factor64));
        assert_eq!(product64.to_bits(), 0x3ff0_0000_0000_0002);
        let separate64 = float(binary(
            &mut engine,
            Add,
            float64,
            product64,
            f64::from_bits(0xbff0_0000_0000_0002),
        ));
        assert_eq!(separate64.to_bits(), 0.0_f64.to_bits());

        let subnormal = float(binary(
            &mut engine,
            Divide,
            float32,
            f64::from(f32::from_bits(0x0080_0000)),
            2.0,
        ));
        assert_eq!((subnormal as f32).to_bits(), 0x0040_0000);

        let overflow = float(binary(
            &mut engine,
            Multiply,
            float32,
            f64::from(f32::MAX),
            2.0,
        ));
        assert!(overflow.is_infinite() && overflow.is_sign_positive());

        let negative_infinity = float(binary(&mut engine, Divide, float32, 1.0, -0.0));
        assert!(negative_infinity.is_infinite() && negative_infinity.is_sign_negative());

        let nan = float(binary(&mut engine, Divide, float32, 0.0, 0.0));
        assert!(matches!(
            binary(&mut engine, Equal, float32, nan, nan),
            Value::Bool(false)
        ));
        assert!(matches!(
            binary(&mut engine, NotEqual, float32, nan, nan),
            Value::Bool(true)
        ));
        assert!(matches!(
            binary(&mut engine, Less, float32, nan, 0.0),
            Value::Bool(false)
        ));

        assert_eq!(
            float(
                engine
                    .materialize_host_value(float32, RuntimeValue::Float(16_777_217.0))
                    .unwrap()
            ),
            16_777_216.0
        );
    }

    #[test]
    fn fixed_width_integer_shifts_wrap_and_all_invalid_counts_use_p0010() {
        use BytecodeBinaryOperator::{ShiftLeft, ShiftRight};

        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );
        for (scalar, signed, bits, minimum, maximum) in [
            (BytecodeScalarType::Byte, false, 8, 0, 255),
            (BytecodeScalarType::UInt8, false, 8, 0, u8::MAX as i128),
            (BytecodeScalarType::UInt16, false, 16, 0, u16::MAX as i128),
            (BytecodeScalarType::UInt32, false, 32, 0, u32::MAX as i128),
            (BytecodeScalarType::UInt64, false, 64, 0, u64::MAX as i128),
            (
                BytecodeScalarType::Int8,
                true,
                8,
                i8::MIN as i128,
                i8::MAX as i128,
            ),
            (
                BytecodeScalarType::Int16,
                true,
                16,
                i16::MIN as i128,
                i16::MAX as i128,
            ),
            (
                BytecodeScalarType::Int32,
                true,
                32,
                i32::MIN as i128,
                i32::MAX as i128,
            ),
            (
                BytecodeScalarType::Int,
                true,
                64,
                i64::MIN as i128,
                i64::MAX as i128,
            ),
        ] {
            let high_bit = 1_i128 << (bits - 1);
            assert_eq!(
                engine
                    .checked_integer_binary(ShiftLeft, scalar, 1, bits - 1)
                    .unwrap(),
                if signed { -high_bit } else { high_bit }
            );
            assert_eq!(
                engine
                    .checked_integer_binary(ShiftLeft, scalar, maximum, 1)
                    .unwrap(),
                if signed { -2 } else { maximum - 1 }
            );
            assert_eq!(
                engine
                    .checked_integer_binary(ShiftLeft, scalar, minimum, 1)
                    .unwrap(),
                0
            );
            assert_eq!(
                engine
                    .checked_integer_binary(
                        ShiftRight,
                        scalar,
                        if signed { -2 } else { maximum },
                        bits - 1,
                    )
                    .unwrap(),
                if signed { -1 } else { 1 }
            );
            assert_eq!(
                engine
                    .checked_integer_binary(ShiftLeft, scalar, maximum, 0)
                    .unwrap(),
                maximum
            );
            for invalid in [-1, bits, i128::from(u64::MAX)] {
                assert_eq!(
                    engine
                        .checked_integer_binary(ShiftLeft, scalar, 1, invalid)
                        .unwrap_err()
                        .0,
                    PanicCode::InvalidShiftCount
                );
            }
        }
    }

    #[test]
    fn checked_integer_arithmetic_uses_the_normative_panic_classes() {
        use BytecodeBinaryOperator::{Add, Divide, Remainder, Subtract};

        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );
        for (scalar, minimum, maximum) in [
            (BytecodeScalarType::UInt8, 0, u8::MAX as i128),
            (BytecodeScalarType::UInt16, 0, u16::MAX as i128),
            (BytecodeScalarType::UInt32, 0, u32::MAX as i128),
            (BytecodeScalarType::UInt64, 0, u64::MAX as i128),
            (BytecodeScalarType::Int8, i8::MIN as i128, i8::MAX as i128),
            (
                BytecodeScalarType::Int16,
                i16::MIN as i128,
                i16::MAX as i128,
            ),
            (
                BytecodeScalarType::Int32,
                i32::MIN as i128,
                i32::MAX as i128,
            ),
            (BytecodeScalarType::Int, i64::MIN as i128, i64::MAX as i128),
        ] {
            assert_eq!(
                engine
                    .checked_integer_binary(Add, scalar, maximum, 1)
                    .unwrap_err()
                    .0,
                PanicCode::CheckedOverflow
            );
            assert_eq!(
                engine
                    .checked_integer_binary(Subtract, scalar, minimum, 1)
                    .unwrap_err()
                    .0,
                PanicCode::CheckedOverflow
            );
            assert_eq!(
                engine
                    .checked_integer_binary(Divide, scalar, maximum, 0)
                    .unwrap_err()
                    .0,
                PanicCode::IntegerDivisionByZero
            );
            assert_eq!(
                engine
                    .checked_integer_binary(Remainder, scalar, maximum, 0)
                    .unwrap_err()
                    .0,
                PanicCode::IntegerDivisionByZero
            );
            if minimum < 0 {
                assert_eq!(
                    engine
                        .checked_integer_binary(Divide, scalar, minimum, -1)
                        .unwrap_err()
                        .0,
                    PanicCode::CheckedOverflow
                );
                assert_eq!(
                    engine
                        .checked_integer_binary(Remainder, scalar, minimum, -1)
                        .unwrap(),
                    0
                );
            }
        }
    }

    fn string_constant(value: &str) -> BytecodeConstantValue {
        BytecodeConstantValue {
            ty: BytecodeTypeId::new(0),
            kind: BytecodeConstantValueKind::String(value.into()),
        }
    }

    fn string_array_constant(values: &[&str]) -> BytecodeConstantValue {
        BytecodeConstantValue {
            ty: BytecodeTypeId::new(1),
            kind: BytecodeConstantValueKind::Array(
                values.iter().map(|value| string_constant(value)).collect(),
            ),
        }
    }

    #[test]
    fn constant_materialization_roots_completed_children_under_gc_pressure() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let constant = BytecodeConstantValue {
            ty: BytecodeTypeId::new(4),
            kind: BytecodeConstantValueKind::Tuple(vec![
                string_array_constant(&["left", "right"]),
                BytecodeConstantValue {
                    ty: BytecodeTypeId::new(2),
                    kind: BytecodeConstantValueKind::Map(vec![
                        (
                            string_constant("first"),
                            string_array_constant(&["one", "two"]),
                        ),
                        (
                            string_constant("second"),
                            string_array_constant(&["three", "four"]),
                        ),
                    ]),
                },
            ]),
        };

        let value = engine.materialize_constant(&constant).unwrap();
        let snapshot = snapshot_value(
            &value,
            &engine.heap,
            &engine.callable_names,
            &engine.nominal_names,
        )
        .unwrap();
        assert_eq!(
            snapshot,
            RuntimeValue::Tuple(vec![
                RuntimeValue::Array(vec![
                    RuntimeValue::String("left".into()),
                    RuntimeValue::String("right".into()),
                ]),
                RuntimeValue::Map(vec![
                    (
                        RuntimeValue::String("first".into()),
                        RuntimeValue::Array(vec![
                            RuntimeValue::String("one".into()),
                            RuntimeValue::String("two".into()),
                        ]),
                    ),
                    (
                        RuntimeValue::String("second".into()),
                        RuntimeValue::Array(vec![
                            RuntimeValue::String("three".into()),
                            RuntimeValue::String("four".into()),
                        ]),
                    ),
                ]),
            ])
        );

        let range = BytecodeConstantValue {
            ty: BytecodeTypeId::new(3),
            kind: BytecodeConstantValueKind::Range {
                kind: BytecodeRangeKind::Inclusive,
                start: Box::new(string_constant("a")),
                end: Box::new(string_constant("z")),
            },
        };
        let value = engine.materialize_constant(&range).unwrap();
        let snapshot = snapshot_value(
            &value,
            &engine.heap,
            &engine.callable_names,
            &engine.nominal_names,
        )
        .unwrap();
        assert_eq!(
            snapshot,
            RuntimeValue::Range {
                inclusive: true,
                start: Box::new(RuntimeValue::String("a".into())),
                end: Box::new(RuntimeValue::String("z".into())),
            }
        );
        assert!(engine.statistics.collections > 0);
    }

    #[test]
    fn normalized_constant_materialization_preserves_every_closed_value_shape() {
        let mut program = terminal_fallback_program();
        let append_scalar =
            |program: &mut BytecodeProgram, name: &str, scalar: BytecodeScalarType| {
                let ty = BytecodeTypeId::new(program.types.len() as u32);
                program.types.push(BytecodeType {
                    name: name.into(),
                    kind: BytecodeTypeKind::Scalar(scalar),
                });
                ty
            };
        let unit = append_scalar(&mut program, "Unit", BytecodeScalarType::Unit);
        let boolean = append_scalar(&mut program, "Bool", BytecodeScalarType::Bool);
        let float = append_scalar(&mut program, "Float", BytecodeScalarType::Float);
        let byte = append_scalar(&mut program, "Byte", BytecodeScalarType::Byte);
        let character = append_scalar(&mut program, "Char", BytecodeScalarType::Char);
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let value = |ty, kind| BytecodeConstantValue { ty, kind };
        let string = |text: &str| {
            value(
                BytecodeTypeId::new(0),
                BytecodeConstantValueKind::String(text.into()),
            )
        };
        let strings = |items: &[&str]| {
            value(
                BytecodeTypeId::new(1),
                BytecodeConstantValueKind::Array(items.iter().map(|item| string(item)).collect()),
            )
        };

        for (constant, expected) in [
            (
                value(unit, BytecodeConstantValueKind::Unit),
                RuntimeValue::Unit,
            ),
            (
                value(boolean, BytecodeConstantValueKind::Bool(true)),
                RuntimeValue::Bool(true),
            ),
            (
                value(
                    BytecodeTypeId::new(5),
                    BytecodeConstantValueKind::Integer(42),
                ),
                RuntimeValue::Integer(42),
            ),
            (
                value(byte, BytecodeConstantValueKind::Integer(255)),
                RuntimeValue::Byte(255),
            ),
            (
                value(float, BytecodeConstantValueKind::Float(1.5_f64.to_bits())),
                RuntimeValue::Float(1.5),
            ),
            (
                value(character, BytecodeConstantValueKind::Char('T')),
                RuntimeValue::Char('T'),
            ),
            (
                value(
                    BytecodeTypeId::new(5),
                    BytecodeConstantValueKind::Function {
                        callable: BytecodeCallableId::new(0),
                        arguments: vec![BytecodeTypeId::new(5)],
                    },
                ),
                RuntimeValue::Function {
                    name: "closure".into(),
                    type_arguments: vec![5],
                },
            ),
            (
                value(
                    BytecodeTypeId::new(8),
                    BytecodeConstantValueKind::Set(vec![string("one"), string("two")]),
                ),
                RuntimeValue::Set(vec![
                    RuntimeValue::String("one".into()),
                    RuntimeValue::String("two".into()),
                ]),
            ),
            (
                value(
                    BytecodeTypeId::new(13),
                    BytecodeConstantValueKind::Newtype {
                        nominal: BytecodeNominalId::new(0),
                        value: Box::new(string("boxed")),
                    },
                ),
                RuntimeValue::Newtype {
                    name: "TextBox".into(),
                    value: Box::new(RuntimeValue::String("boxed".into())),
                },
            ),
            (
                value(
                    BytecodeTypeId::new(14),
                    BytecodeConstantValueKind::Record {
                        nominal: BytecodeNominalId::new(1),
                        fields: vec![(0, string("message")), (1, strings(&["a", "b"]))],
                    },
                ),
                RuntimeValue::Record {
                    name: "Message".into(),
                    fields: vec![
                        (0, RuntimeValue::String("message".into())),
                        (
                            1,
                            RuntimeValue::Array(vec![
                                RuntimeValue::String("a".into()),
                                RuntimeValue::String("b".into()),
                            ]),
                        ),
                    ],
                },
            ),
            (
                value(
                    BytecodeTypeId::new(15),
                    BytecodeConstantValueKind::Variant {
                        variant: 1,
                        payload: BytecodeConstantVariantValue::Tuple(vec![string("tuple")]),
                    },
                ),
                RuntimeValue::Variant {
                    variant: 1,
                    payload: vec![(None, RuntimeValue::String("tuple".into()))],
                },
            ),
            (
                value(
                    BytecodeTypeId::new(15),
                    BytecodeConstantValueKind::Variant {
                        variant: 2,
                        payload: BytecodeConstantVariantValue::Record(vec![(
                            0,
                            strings(&["record"]),
                        )]),
                    },
                ),
                RuntimeValue::Variant {
                    variant: 2,
                    payload: vec![(
                        Some(0),
                        RuntimeValue::Array(vec![RuntimeValue::String("record".into())]),
                    )],
                },
            ),
            (
                value(
                    BytecodeTypeId::new(9),
                    BytecodeConstantValueKind::OptionNone,
                ),
                RuntimeValue::OptionNone,
            ),
            (
                value(
                    BytecodeTypeId::new(9),
                    BytecodeConstantValueKind::OptionSome(Box::new(string("some"))),
                ),
                RuntimeValue::OptionSome(Box::new(RuntimeValue::String("some".into()))),
            ),
            (
                value(
                    BytecodeTypeId::new(10),
                    BytecodeConstantValueKind::ResultOk(Box::new(string("ok"))),
                ),
                RuntimeValue::ResultOk(Box::new(RuntimeValue::String("ok".into()))),
            ),
            (
                value(
                    BytecodeTypeId::new(10),
                    BytecodeConstantValueKind::ResultErr(Box::new(strings(&["error"]))),
                ),
                RuntimeValue::ResultErr(Box::new(RuntimeValue::Array(vec![RuntimeValue::String(
                    "error".into(),
                )]))),
            ),
        ] {
            let materialized = engine.materialize_constant(&constant).unwrap();
            assert_eq!(
                snapshot_value(
                    &materialized,
                    &engine.heap,
                    &engine.callable_names,
                    &engine.nominal_names,
                )
                .unwrap(),
                expected
            );
        }

        assert_eq!(
            engine
                .inline_constant(byte, &BytecodeConstant::Integer("255".into()))
                .unwrap(),
            Value::Byte(255)
        );
        assert!(matches!(
            engine.inline_constant(byte, &BytecodeConstant::Integer("256".into())),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.materialize_constant(&value(byte, BytecodeConstantValueKind::Integer(256),)),
            Err(VmError::Invariant(_))
        ));
    }

    #[test]
    fn runtime_value_algorithms_reject_corruption_and_cover_closed_collection_semantics() {
        let mut program = terminal_fallback_program();
        let append = |program: &mut BytecodeProgram, name: &str, kind: BytecodeTypeKind| {
            let ty = BytecodeTypeId::new(program.types.len() as u32);
            program.types.push(BytecodeType {
                name: name.into(),
                kind,
            });
            ty
        };
        let boolean = append(
            &mut program,
            "Bool",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Bool),
        );
        let byte = append(
            &mut program,
            "Byte",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Byte),
        );
        let float = append(
            &mut program,
            "Float",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Float),
        );
        let character = append(
            &mut program,
            "Char",
            BytecodeTypeKind::Scalar(BytecodeScalarType::Char),
        );
        let reference = append(
            &mut program,
            "Ref[String]",
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Ref,
                arguments: vec![BytecodeTypeId::new(0)],
            },
        );
        let mut_cursor = append(
            &mut program,
            "cursor[mut, Array[Int]]",
            BytecodeTypeKind::Cursor {
                mode: BytecodeCursorMode::Mut,
                collection: BytecodeTypeId::new(6),
            },
        );
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );

        let place = BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty: BytecodeTypeId::new(5),
            projections: Vec::new(),
            source_loan: None,
        };
        assert!(matches!(
            engine.copy_value(&Value::Loan(RuntimeLoan {
                task: 0,
                frame: 0,
                place,
                mode: BytecodeParameterMode::Ref,
            })),
            Err(VmError::Invariant(_))
        ));

        let text = engine
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::String("éclair".into()),
                &[],
            )
            .unwrap();
        let reference_value = engine
            .allocate(
                reference,
                HeapObject::Ref(Some(text.clone())),
                std::slice::from_ref(&text),
            )
            .unwrap();
        assert_eq!(
            engine.copy_value(&reference_value).unwrap(),
            reference_value
        );

        let empty_array = engine
            .allocate(
                BytecodeTypeId::new(6),
                HeapObject::Array(Vec::new().into()),
                &[],
            )
            .unwrap();
        let ref_cursor = engine
            .allocate(
                BytecodeTypeId::new(17),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Ref,
                    source: Some(empty_array.clone()),
                    next: 3,
                    adapter: None,
                },
                std::slice::from_ref(&empty_array),
            )
            .unwrap();
        let copied_cursor = engine.copy_value(&ref_cursor).unwrap();
        assert_ne!(copied_cursor, ref_cursor);
        let exclusive_cursor = engine
            .allocate(
                mut_cursor,
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Mut,
                    source: Some(empty_array.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(&empty_array),
            )
            .unwrap();
        assert!(matches!(
            engine.copy_value(&exclusive_cursor),
            Err(VmError::Invariant(_))
        ));

        for value in [
            Value::Unit,
            Value::Bool(true),
            Value::Integer(42),
            Value::Float(1.5),
            Value::Byte(7),
            Value::Char('T'),
            Value::Function {
                callable: BytecodeCallableId::new(0),
                arguments: vec![BytecodeTypeId::new(5)],
            },
        ] {
            assert!(engine.value_equal(&value, &value).unwrap());
        }
        assert!(
            !engine
                .value_equal(&Value::Unit, &Value::Bool(true))
                .unwrap()
        );
        assert!(
            !engine
                .value_equal(&Value::Integer(1), &Value::Integer(2))
                .unwrap()
        );

        let string = |engine: &mut Engine<'_, '_>, value: &str| {
            engine
                .allocate(
                    BytecodeTypeId::new(0),
                    HeapObject::String(value.into()),
                    &[],
                )
                .unwrap()
        };
        let one = string(&mut engine, "one");
        let two = string(&mut engine, "two");
        let three = string(&mut engine, "three");
        let left_set = engine
            .allocate(
                BytecodeTypeId::new(8),
                HeapObject::Set(vec![Some(one.clone()), Some(two.clone())].into()),
                &[one.clone(), two.clone()],
            )
            .unwrap();
        let reordered_set = engine
            .allocate(
                BytecodeTypeId::new(8),
                HeapObject::Set(vec![Some(two.clone()), Some(one.clone())].into()),
                &[one.clone(), two.clone()],
            )
            .unwrap();
        let different_set = engine
            .allocate(
                BytecodeTypeId::new(8),
                HeapObject::Set(vec![Some(one.clone()), Some(three.clone())].into()),
                &[one.clone(), three.clone()],
            )
            .unwrap();
        assert!(engine.value_equal(&left_set, &reordered_set).unwrap());
        assert!(!engine.value_equal(&left_set, &different_set).unwrap());

        let left_map = engine
            .allocate(
                BytecodeTypeId::new(2),
                HeapObject::Map(
                    vec![
                        (Some(one.clone()), Some(two.clone())),
                        (Some(two.clone()), Some(three.clone())),
                    ]
                    .into(),
                ),
                &[one.clone(), two.clone(), three.clone()],
            )
            .unwrap();
        let reordered_map = engine
            .allocate(
                BytecodeTypeId::new(2),
                HeapObject::Map(
                    vec![
                        (Some(two.clone()), Some(three.clone())),
                        (Some(one.clone()), Some(two.clone())),
                    ]
                    .into(),
                ),
                &[one.clone(), two.clone(), three.clone()],
            )
            .unwrap();
        let changed_map = engine
            .allocate(
                BytecodeTypeId::new(2),
                HeapObject::Map(
                    vec![
                        (Some(two.clone()), Some(one.clone())),
                        (Some(one.clone()), Some(two.clone())),
                    ]
                    .into(),
                ),
                &[one.clone(), two.clone()],
            )
            .unwrap();
        assert!(engine.value_equal(&left_map, &reordered_map).unwrap());
        assert!(!engine.value_equal(&left_map, &changed_map).unwrap());

        let first_cycle = engine
            .allocate(BytecodeTypeId::new(9), HeapObject::OptionNone, &[])
            .unwrap();
        let second_cycle = engine
            .allocate(BytecodeTypeId::new(9), HeapObject::OptionNone, &[])
            .unwrap();
        for value in [&first_cycle, &second_cycle] {
            let Value::Heap(handle) = value else {
                unreachable!()
            };
            engine
                .replace_object(
                    *handle,
                    HeapObject::OptionSome(Some(Value::Heap(*handle))),
                    &[],
                )
                .unwrap();
        }
        assert!(engine.value_equal(&first_cycle, &second_cycle).unwrap());

        assert_eq!(
            engine
                .value_order(&Value::Integer(1), &Value::Integer(2))
                .unwrap(),
            Some(std::cmp::Ordering::Less)
        );
        assert_eq!(
            engine
                .value_order(&Value::Float(f64::NAN), &Value::Float(0.0))
                .unwrap(),
            None
        );
        assert_eq!(
            engine
                .value_order(&Value::Byte(2), &Value::Byte(1))
                .unwrap(),
            Some(std::cmp::Ordering::Greater)
        );
        assert_eq!(
            engine
                .value_order(&Value::Char('a'), &Value::Char('a'))
                .unwrap(),
            Some(std::cmp::Ordering::Equal)
        );
        assert_eq!(
            engine.value_order(&one, &two).unwrap(),
            Some(std::cmp::Ordering::Less)
        );
        assert!(matches!(
            engine.value_order(&left_set, &reordered_set),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.value_order(&Value::Unit, &Value::Unit),
            Err(VmError::Invariant(_))
        ));

        let array = engine
            .allocate(
                BytecodeTypeId::new(1),
                HeapObject::Array(vec![Some(one.clone()), Some(two.clone())].into()),
                &[one.clone(), two.clone()],
            )
            .unwrap();
        assert!(
            engine
                .contains(BytecodeContainmentKind::Array, &one, &array)
                .unwrap()
        );
        assert!(
            !engine
                .contains(BytecodeContainmentKind::Array, &three, &array)
                .unwrap()
        );
        assert!(
            engine
                .contains(BytecodeContainmentKind::Set, &two, &left_set)
                .unwrap()
        );
        assert!(
            engine
                .contains(BytecodeContainmentKind::MapKey, &two, &left_map)
                .unwrap()
        );
        let range = engine
            .allocate(
                BytecodeTypeId::new(3),
                HeapObject::Range {
                    kind: BytecodeRangeKind::Exclusive,
                    start: Some(Value::Integer(1)),
                    end: Some(Value::Integer(3)),
                },
                &[],
            )
            .unwrap();
        assert!(
            engine
                .contains(BytecodeContainmentKind::Range, &Value::Integer(2), &range)
                .unwrap()
        );
        assert!(
            !engine
                .contains(BytecodeContainmentKind::Range, &Value::Integer(3), &range)
                .unwrap()
        );
        assert!(
            engine
                .contains(
                    BytecodeContainmentKind::StringChar,
                    &Value::Char('é'),
                    &text
                )
                .unwrap()
        );
        assert!(matches!(
            engine.contains(
                BytecodeContainmentKind::StringChar,
                &Value::Integer(1),
                &text
            ),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.contains(BytecodeContainmentKind::Set, &one, &array),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.contains(BytecodeContainmentKind::Array, &one, &Value::Integer(1),),
            Err(VmError::Invariant(_))
        ));

        assert_eq!(engine.string_value(&text).unwrap(), "éclair");
        assert!(matches!(
            engine.string_value(&Value::Integer(1)),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.string_value(&left_set),
            Err(VmError::Invariant(_))
        ));
        assert_eq!(engine.type_name(BytecodeTypeId::new(5)), "Int");
        assert_eq!(engine.type_name(BytecodeTypeId::new(999)), "<invalid-type>");

        assert!(matches!(
            engine.borrowed_iterator_has_item(&Value::Integer(1), 0),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.borrowed_iterator_has_item(&text, 0),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&Value::Integer(1), BytecodeTypeId::new(5), 0),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&array, BytecodeTypeId::new(5), 1),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&left_set, BytecodeTypeId::new(0), 1),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&left_map, BytecodeTypeId::new(4), 1),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&text, character, 1),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.iterator_item(&first_cycle, BytecodeTypeId::new(5), 0),
            Err(VmError::Invariant(_))
        ));
        assert!(matches!(
            engine.range_item(
                BytecodeRangeKind::Exclusive,
                &Value::Integer(1),
                &Value::Char('z'),
                0,
            ),
            Err(VmError::Invariant(_))
        ));
        assert_eq!(
            engine
                .range_item(
                    BytecodeRangeKind::Exclusive,
                    &Value::Char('a'),
                    &Value::Char('c'),
                    3,
                )
                .unwrap(),
            (None, usize::MAX)
        );
        assert_eq!(
            engine
                .range_item(
                    BytecodeRangeKind::Inclusive,
                    &Value::Integer(i128::MAX),
                    &Value::Integer(i128::MAX),
                    1,
                )
                .unwrap(),
            (None, usize::MAX)
        );

        assert_eq!(
            engine
                .checked_prefix(
                    BytecodePrefixOperator::Negate,
                    BytecodeTypeId::new(5),
                    Value::Integer(42),
                )
                .unwrap()
                .unwrap(),
            Value::Integer(-42)
        );
        assert_eq!(
            engine
                .checked_prefix(
                    BytecodePrefixOperator::Negate,
                    BytecodeTypeId::new(5),
                    Value::Integer(i64::MIN as i128),
                )
                .unwrap()
                .unwrap_err()
                .0,
            PanicCode::CheckedOverflow
        );
        assert!(matches!(
            engine.checked_prefix(
                BytecodePrefixOperator::LogicalNot,
                boolean,
                Value::Bool(true),
            ),
            Err(VmError::Invariant(_))
        ));
        assert_eq!(
            engine
                .checked_scalar_binary(
                    BytecodeBinaryOperator::Add,
                    byte,
                    BytecodeTypeId::new(5),
                    Value::Byte(40),
                    Value::Integer(2),
                )
                .unwrap()
                .unwrap(),
            Value::Byte(42)
        );
        assert_eq!(
            engine
                .checked_scalar_binary(
                    BytecodeBinaryOperator::Multiply,
                    byte,
                    byte,
                    Value::Byte(6),
                    Value::Byte(7),
                )
                .unwrap()
                .unwrap(),
            Value::Byte(42)
        );
        assert!(matches!(
            engine.checked_scalar_binary(
                BytecodeBinaryOperator::Add,
                float,
                float,
                Value::Unit,
                Value::Unit,
            ),
            Err(VmError::Invariant(_))
        ));

        for (shape, values) in [
            (
                BytecodeAggregateKind::Closure {
                    callable: BytecodeCallableId::new(0),
                    captures: vec![BytecodeTypeId::new(0)],
                },
                Vec::new(),
            ),
            (
                BytecodeAggregateKind::Newtype {
                    nominal: BytecodeNominalId::new(0),
                },
                Vec::new(),
            ),
            (BytecodeAggregateKind::Ref, Vec::new()),
            (
                BytecodeAggregateKind::Record {
                    nominal: BytecodeNominalId::new(1),
                    fields: vec![0, 1],
                },
                vec![Value::Unit],
            ),
            (
                BytecodeAggregateKind::Variant {
                    variant: 1,
                    fields: vec![None],
                },
                Vec::new(),
            ),
            (
                BytecodeAggregateKind::Variant {
                    variant: 2,
                    fields: vec![Some(0), None],
                },
                vec![Value::Unit, Value::Unit],
            ),
            (BytecodeAggregateKind::OptionNone, vec![Value::Unit]),
            (BytecodeAggregateKind::OptionSome, Vec::new()),
            (BytecodeAggregateKind::ResultOk, Vec::new()),
            (BytecodeAggregateKind::ResultErr, Vec::new()),
        ] {
            assert!(matches!(
                engine.construct_aggregate(BytecodeTypeId::new(9), &shape, values),
                Err(VmError::Invariant(_))
            ));
        }
    }

    #[test]
    fn eager_cursor_copy_owns_an_independent_source_and_position() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::Eager,
            trace,
        );
        let source = engine
            .materialize_constant(&BytecodeConstantValue {
                ty: BytecodeTypeId::new(6),
                kind: BytecodeConstantValueKind::Array(vec![
                    BytecodeConstantValue {
                        ty: BytecodeTypeId::new(5),
                        kind: BytecodeConstantValueKind::Integer(1),
                    },
                    BytecodeConstantValue {
                        ty: BytecodeTypeId::new(5),
                        kind: BytecodeConstantValueKind::Integer(2),
                    },
                ]),
            })
            .unwrap();
        let original = engine
            .allocate(
                BytecodeTypeId::new(7),
                super::HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(source.clone()),
                    next: 1,
                    adapter: None,
                },
                std::slice::from_ref(&source),
            )
            .unwrap();
        let allocations = engine.statistics.allocations;
        let copied = engine.copy_value(&original).unwrap();
        assert_eq!(engine.statistics.allocations, allocations + 2);

        let (Value::Heap(original), Value::Heap(copied)) = (original, copied) else {
            unreachable!("managed cursors use heap handles")
        };
        assert_ne!(original, copied);
        let super::HeapObject::Iterator {
            mode: original_mode,
            source: original_source,
            next: original_next,
            adapter: _,
        } = engine.heap.get(original).unwrap().clone()
        else {
            unreachable!("the original cursor retains its iterator shape")
        };
        let super::HeapObject::Iterator {
            mode: copied_mode,
            source: copied_source,
            next: copied_next,
            adapter: _,
        } = engine.heap.get(copied).unwrap().clone()
        else {
            unreachable!("the copied cursor retains its iterator shape")
        };
        assert_eq!(original_mode, BytecodeCursorMode::Own);
        assert_eq!(copied_mode, BytecodeCursorMode::Own);
        assert_eq!(original_next, 1);
        assert_eq!(copied_next, 1);
        assert_ne!(
            original_source.unwrap().heap_handle(),
            copied_source.unwrap().heap_handle(),
            "owning cursor copies must not share their destructively advanced collection"
        );

        engine
            .replace_object(
                original,
                super::HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(source),
                    next: 2,
                    adapter: None,
                },
                &[],
            )
            .unwrap();
        assert!(matches!(
            engine.heap.get(copied).unwrap(),
            super::HeapObject::Iterator { next: 1, .. }
        ));
    }

    #[test]
    fn host_materialization_roots_completed_children_under_gc_pressure() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let returned = RuntimeValue::Tuple(vec![
            RuntimeValue::Array(vec![
                RuntimeValue::String("left".into()),
                RuntimeValue::String("right".into()),
            ]),
            RuntimeValue::Array(vec![
                RuntimeValue::String("up".into()),
                RuntimeValue::String("down".into()),
            ]),
        ]);

        let value = engine
            .materialize_host_value(BytecodeTypeId::new(4), returned.clone())
            .unwrap();
        let snapshot = snapshot_value(
            &value,
            &engine.heap,
            &engine.callable_names,
            &engine.nominal_names,
        )
        .unwrap();
        assert_eq!(snapshot, returned);
        assert!(engine.statistics.collections > 0);
    }

    #[test]
    fn detached_host_snapshots_do_not_become_vm_roots() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let managed = engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("host-owned".into()),
                &[],
            )
            .unwrap();
        let retained_by_host = snapshot_value(
            &managed,
            &engine.heap,
            &engine.callable_names,
            &engine.nominal_names,
        )
        .unwrap();

        engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("pressure".into()),
                &[],
            )
            .unwrap();

        assert_eq!(retained_by_host, RuntimeValue::String("host-owned".into()));
        assert!(
            snapshot_value(
                &managed,
                &engine.heap,
                &engine.callable_names,
                &engine.nominal_names,
            )
            .is_err()
        );
        assert!(engine.statistics.reclaimed_objects > 0);
    }

    #[test]
    fn structured_pending_values_publish_and_withdraw_temporary_roots() {
        let program = root_pressure_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            pressure_limits(),
            ValueCopyStrategy::default(),
            trace,
        );
        let retained = engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("retained".into()),
                &[],
            )
            .unwrap();
        let marker = engine.temporary_roots.len();
        let mut pending = Vec::new();
        engine.queue_fallback_value(
            &mut pending,
            RuntimeType {
                ty: BytecodeTypeId::new(0),
                substitutions: Vec::new(),
            },
            retained.clone(),
        );

        engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("pressure".into()),
                &[],
            )
            .unwrap();
        assert_eq!(
            snapshot_value(
                &retained,
                &engine.heap,
                &engine.callable_names,
                &engine.nominal_names,
            )
            .unwrap(),
            RuntimeValue::String("retained".into())
        );

        pending.clear();
        engine.temporary_roots.truncate(marker);
        engine
            .allocate(
                BytecodeTypeId::new(0),
                super::HeapObject::String("after-withdrawal".into()),
                &[],
            )
            .unwrap();
        assert!(matches!(retained, Value::Heap(_)));
        assert!(
            snapshot_value(
                &retained,
                &engine.heap,
                &engine.callable_names,
                &engine.nominal_names,
            )
            .is_err()
        );
        assert!(engine.statistics.reclaimed_objects > 0);
    }

    #[test]
    fn iterator_contract_helpers_reject_malformed_descriptors_and_states() {
        let mut program = terminal_fallback_program();
        let trace = derive_trace_metadata(&program).unwrap();
        let add_type =
            |program: &mut BytecodeProgram, name: &str, kind: BytecodeTypeKind| -> BytecodeTypeId {
                let id = BytecodeTypeId::new(program.types.len() as u32);
                program.types.push(BytecodeType {
                    name: name.into(),
                    kind,
                });
                id
            };
        let string = BytecodeTypeId::new(0);
        let strings = BytecodeTypeId::new(1);
        let int = BytecodeTypeId::new(5);
        let ints = BytecodeTypeId::new(6);
        let non_array_cursor = add_type(
            &mut program,
            "cursor[own, String]",
            BytecodeTypeKind::Cursor {
                mode: BytecodeCursorMode::Own,
                collection: string,
            },
        );
        let empty_array = add_type(
            &mut program,
            "Array[]",
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Array,
                arguments: Vec::new(),
            },
        );
        let empty_cursor = add_type(
            &mut program,
            "cursor[own, Array[]]",
            BytecodeTypeKind::Cursor {
                mode: BytecodeCursorMode::Own,
                collection: empty_array,
            },
        );
        let empty_result = add_type(
            &mut program,
            "Array[] ! String",
            BytecodeTypeKind::Result {
                success: empty_array,
                error: string,
            },
        );
        let int_result = add_type(
            &mut program,
            "Array[Int] ! Array[String]",
            BytecodeTypeKind::Result {
                success: ints,
                error: strings,
            },
        );
        let bad_map = add_type(
            &mut program,
            "Map[String]",
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Map,
                arguments: vec![string],
            },
        );
        let bad_set = add_type(
            &mut program,
            "Set[]",
            BytecodeTypeKind::Intrinsic {
                constructor: BytecodeIntrinsicType::Set,
                arguments: Vec::new(),
            },
        );
        let mut host = RejectingHost;
        let mut engine = Engine::new(
            &program,
            &mut host,
            VmLimits::default(),
            ValueCopyStrategy::default(),
            trace,
        );

        assert!(matches!(
            engine.normalize_iterator_source(&Value::Integer(1), BytecodeTypeId::new(7)),
            Err(VmError::Invariant(message)) if message.contains("not a managed iterable")
        ));
        let option = engine
            .allocate(BytecodeTypeId::new(9), HeapObject::OptionNone, &[])
            .unwrap();
        assert!(matches!(
            engine.normalize_iterator_source(&option, BytecodeTypeId::new(7)),
            Err(VmError::Invariant(message)) if message.contains("not an owning iterable")
        ));

        let array = engine
            .allocate(
                ints,
                HeapObject::Array(vec![Some(Value::Integer(1)), Some(Value::Integer(2))].into()),
                &[],
            )
            .unwrap();
        let normalized = engine
            .normalize_iterator_source(&array, BytecodeTypeId::new(7))
            .unwrap();
        assert!(matches!(
            engine.heap.get(normalized.heap_handle().unwrap()).unwrap(),
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Own,
                adapter: None,
                ..
            }
        ));
        let own_cursor = engine
            .allocate(
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        assert_eq!(
            engine
                .normalize_iterator_source(&own_cursor, BytecodeTypeId::new(7))
                .unwrap(),
            own_cursor
        );
        for source in [
            engine
                .allocate(string, HeapObject::String("text".into()), &[])
                .unwrap(),
            engine
                .allocate(
                    BytecodeTypeId::new(2),
                    HeapObject::Map(vec![(Some(Value::Integer(1)), Some(array.clone()))].into()),
                    std::slice::from_ref(&array),
                )
                .unwrap(),
            engine
                .allocate(
                    BytecodeTypeId::new(8),
                    HeapObject::Set(vec![Some(Value::Integer(1))].into()),
                    &[],
                )
                .unwrap(),
            engine
                .allocate(
                    BytecodeTypeId::new(3),
                    HeapObject::Range {
                        kind: BytecodeRangeKind::Exclusive,
                        start: Some(Value::Integer(0)),
                        end: Some(Value::Integer(2)),
                    },
                    &[],
                )
                .unwrap(),
        ] {
            assert!(
                engine
                    .normalize_iterator_source(&source, BytecodeTypeId::new(7))
                    .is_ok()
            );
        }

        assert!(
            engine
                .cursor_array_element_type(BytecodeTypeId::new(999))
                .is_err()
        );
        assert!(engine.cursor_array_element_type(string).is_err());
        assert!(engine.cursor_array_element_type(non_array_cursor).is_err());
        assert!(engine.cursor_array_element_type(empty_cursor).is_err());
        assert_eq!(
            engine
                .cursor_array_element_type(BytecodeTypeId::new(7))
                .unwrap(),
            int
        );
        assert!(engine.result_array_type(BytecodeTypeId::new(999)).is_err());
        assert!(engine.result_array_type(string).is_err());
        assert!(engine.result_array_type(BytecodeTypeId::new(10)).is_err());
        assert!(engine.result_array_type(empty_result).is_err());
        assert_eq!(engine.result_array_type(int_result).unwrap(), (ints, int));
        assert!(matches!(
            engine.materialize_host_value(bad_map, RuntimeValue::Map(Vec::new())),
            Err(VmError::Invariant(message)) if message.contains("map type has the wrong arity")
        ));
        assert!(matches!(
            engine.materialize_host_value(bad_set, RuntimeValue::Set(Vec::new())),
            Err(VmError::Invariant(message)) if message.contains("set type has the wrong arity")
        ));
        let host_array = engine
            .materialize_host_value(
                strings,
                RuntimeValue::Array(vec![RuntimeValue::String("array".into())]),
            )
            .unwrap();
        assert!(matches!(
            engine.heap.get(host_array.heap_handle().unwrap()).unwrap(),
            HeapObject::Array(_)
        ));
        let host_map = engine
            .materialize_host_value(
                BytecodeTypeId::new(2),
                RuntimeValue::Map(vec![(
                    RuntimeValue::String("key".into()),
                    RuntimeValue::Array(vec![RuntimeValue::String("value".into())]),
                )]),
            )
            .unwrap();
        assert!(matches!(
            engine.heap.get(host_map.heap_handle().unwrap()).unwrap(),
            HeapObject::Map(_)
        ));
        let host_set = engine
            .materialize_host_value(
                BytecodeTypeId::new(8),
                RuntimeValue::Set(vec![RuntimeValue::String("set".into())]),
            )
            .unwrap();
        assert!(matches!(
            engine.heap.get(host_set.heap_handle().unwrap()).unwrap(),
            HeapObject::Set(_)
        ));

        assert!(matches!(
            engine.next_owned_iterator_value(&Value::Integer(1), int),
            Err(VmError::Invariant(message)) if message.contains("not a managed cursor")
        ));
        assert!(matches!(
            engine.next_owned_iterator_value(&array, int),
            Err(VmError::Invariant(message)) if message.contains("wrong heap shape")
        ));
        let borrowed_cursor = engine
            .allocate(
                BytecodeTypeId::new(17),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Ref,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        assert!(matches!(
            engine.next_owned_iterator_value(&borrowed_cursor, int),
            Err(VmError::Invariant(message)) if message.contains("not an owning cursor")
        ));
        let exhausted_cursor = engine
            .allocate(
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(array.clone()),
                    next: usize::MAX,
                    adapter: None,
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        assert_eq!(
            engine
                .next_owned_iterator_value(&exhausted_cursor, int)
                .unwrap()
                .unwrap(),
            None
        );
        let missing_source = engine
            .allocate(
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: None,
                    next: 0,
                    adapter: None,
                },
                &[],
            )
            .unwrap();
        assert!(matches!(
            engine.next_owned_iterator_value(&missing_source, int),
            Err(VmError::Invariant(message)) if message.contains("iterator source")
        ));
        let (step, adapter, next) = engine
            .iterator_adapter_next(
                &Value::Integer(1),
                IteratorAdapter::Take {
                    remaining: 0,
                    source_item: int,
                },
                int,
            )
            .unwrap();
        assert!(step.is_none());
        assert_eq!(next, usize::MAX);
        assert!(matches!(
            adapter,
            IteratorAdapter::Take { remaining: 0, .. }
        ));

        let state = BytecodePlace {
            slot: BytecodeSlotId::new(0),
            ty: BytecodeTypeId::new(7),
            projections: Vec::new(),
            source_loan: None,
        };
        let span = BytecodeSpan {
            file: 0,
            start: 0,
            end: 0,
        };
        let fallback = install_fallback_frame(&mut engine, int, Value::Integer(1));
        assert!(matches!(
            engine.iterator_next(0, &fallback.owner, None, int, span),
            Err(VmError::Invariant(message)) if message.contains("not managed")
        ));
        engine.frames[0].slots[0] = SlotState::Value(array.clone());
        assert!(matches!(
            engine.iterator_next(0, &state, None, int, span),
            Err(VmError::Invariant(message)) if message.contains("wrong heap shape")
        ));
        engine.frames[0].slots[0] = SlotState::Value(exhausted_cursor.clone());
        assert!(matches!(
            engine.iterator_next(0, &state, None, int, span),
            Ok(Ok(None))
        ));
        let adapter_cursor = engine
            .allocate(
                BytecodeTypeId::new(17),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Ref,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: Some(IteratorAdapter::Take {
                        remaining: 0,
                        source_item: int,
                    }),
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        engine.frames[0].slots[0] = SlotState::Value(adapter_cursor);
        assert!(matches!(
            engine.iterator_next(0, &state, None, int, span),
            Err(VmError::Invariant(message)) if message.contains("lazy iterator adapter")
        ));
        let owning_adapter = engine
            .allocate(
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: Some(IteratorAdapter::Take {
                        remaining: 0,
                        source_item: int,
                    }),
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        engine.frames[0].slots[0] = SlotState::Value(owning_adapter);
        assert!(matches!(
            engine.iterator_next(0, &state, Some(&state), int, span),
            Err(VmError::Invariant(message)) if message.contains("lazy iterator adapter")
        ));
        let owning_cursor = engine
            .allocate(
                BytecodeTypeId::new(7),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Own,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        engine.frames[0].slots[0] = SlotState::Value(owning_cursor);
        assert!(matches!(
            engine.iterator_next(0, &state, Some(&state), int, span),
            Err(VmError::Invariant(message)) if message.contains("owning iterator")
        ));
        let ref_state = engine
            .allocate(
                BytecodeTypeId::new(17),
                HeapObject::Iterator {
                    mode: BytecodeCursorMode::Ref,
                    source: Some(array.clone()),
                    next: 0,
                    adapter: None,
                },
                std::slice::from_ref(&array),
            )
            .unwrap();
        engine.frames[0].slots[0] = SlotState::Value(ref_state);
        assert!(matches!(
            engine.iterator_next(0, &state, None, int, span),
            Err(VmError::Invariant(message)) if message.contains("no source place")
        ));
        let other_array = engine
            .allocate(
                ints,
                HeapObject::Array(vec![Some(Value::Integer(9))].into()),
                &[],
            )
            .unwrap();
        engine.frames[0].slots.push(SlotState::Value(other_array));
        engine.frame_traces[0].slots.push(ints);
        let borrowed_place = BytecodePlace {
            slot: BytecodeSlotId::new(1),
            ty: ints,
            projections: Vec::new(),
            source_loan: None,
        };
        assert!(matches!(
            engine.iterator_next(0, &state, Some(&borrowed_place), int, span),
            Err(VmError::Invariant(message)) if message.contains("differs")
        ));
        engine.frames[0].slots[1] = SlotState::Value(array);
        assert!(matches!(
            engine.iterator_next(0, &state, Some(&borrowed_place), int, span),
            Ok(Ok(Some(super::IteratorStep::Position(0))))
        ));
    }
}
