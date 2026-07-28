use std::cmp::Ordering;
use std::collections::VecDeque;

use crate::bytecode::{
    ArraySliceError, BytecodeAggregateKind, BytecodeArraySequenceKind, BytecodeAwaitable,
    BytecodeBinaryOperator, BytecodeBlockId, BytecodeBootstrapHostFunction, BytecodeCallArgument,
    BytecodeCallArgumentTarget, BytecodeCoercion, BytecodeConstant, BytecodeConstantValue,
    BytecodeConstantValueKind, BytecodeConstantVariantValue, BytecodeContainmentKind,
    BytecodeCursorMode, BytecodeFunctionId, BytecodeIndexAccess, BytecodeInstruction,
    BytecodeInstructionKind, BytecodeIntrinsicType, BytecodeLoanId, BytecodeLoanKind,
    BytecodeNominalShape, BytecodeNumericConversion, BytecodeNumericConversionError,
    BytecodeOperand, BytecodeOperandKind, BytecodeOperation, BytecodeOperationKind,
    BytecodeParameterMode, BytecodePlace, BytecodePrefixOperator, BytecodeProgram,
    BytecodeProjection, BytecodeProjectionKind, BytecodeRangeKind, BytecodeRvalue,
    BytecodeRvalueKind, BytecodeScalarType, BytecodeScopeId, BytecodeSlotId, BytecodeSpan,
    BytecodeTag, BytecodeTerminator, BytecodeTerminatorKind, BytecodeTraceMetadata, BytecodeTypeId,
    BytecodeTypeKind, BytecodeVariantPayload, BytecodeVerificationLimits, normalize_array_index,
    normalize_array_slice_indices, verify_bytecode_with_trace_metadata,
};
use crate::literal;

use super::heap::{Heap, HeapHandle, HeapObject};
use super::value::{AggregatePayload, RuntimeJoin, RuntimeLoan, Value, snapshot_value};
use super::{
    PanicCode, RuntimeValue, ValueCopyStrategy, VmError, VmLimits, VmPanic, VmStackFrame,
    VmStatistics,
};

type HeapMapEntry = (Option<Value>, Option<Value>);

/// Host boundary for callables that deliberately have no bytecode body.
///
/// Arguments and results are detached snapshots. A host may retain or mutate
/// its own values, but it never receives a VM heap handle and therefore cannot
/// keep a managed object alive accidentally.
pub trait VmHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError>;
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
    Scope,
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

        while let Some(next) = self.runnable.pop_front() {
            let task = self
                .tasks
                .get_mut(next)
                .ok_or_else(|| VmError::invariant("the runnable queue contains an invalid task"))?;
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
        Err(VmError::invariant(
            "the cooperative executor has no runnable task before root completion",
        ))
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
        let panicked = matches!(completion, TaskCompletion::Panicked(_));
        let parent_scope = self.tasks[self.current_task].parent_scope;
        self.tasks[self.current_task].status = TaskStatus::Complete(Some(completion));
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
                    if sibling != self.current_task {
                        self.request_cancel(sibling)?;
                    }
                }
                self.wake_task(owner)?;
            }
        }
        let waiters = std::mem::take(&mut self.tasks[self.current_task].waiters);
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
        if !self.frames[frame].task_scopes.contains(&join.scope) {
            return Err(VmError::invariant(
                "Join escaped its owning active task scope",
            ));
        }
        let task = self
            .tasks
            .get_mut(join.task)
            .ok_or_else(|| VmError::invariant("Join references an invalid task"))?;
        if task.parent_scope != Some(join.scope) || task.join_consumed {
            return Err(VmError::invariant(
                "Join was consumed twice or by the wrong task scope",
            ));
        }
        task.join_consumed = true;
        Ok(join)
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
            | HeapObject::Ref(value)
            | HeapObject::Iterator { source: value, .. } => match value {
                Some(value) => self.value_contains_scope_join(value, scope, visited)?,
                None => false,
            },
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
                    | BytecodeIntrinsicType::Command
                    | BytecodeIntrinsicType::Pipeline
                    | BytecodeIntrinsicType::NumericConversionError => {}
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
                        };
                        self.push_frame(function, arguments, Some(continuation))?;
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
                                    }),
                                )?;
                            }
                            OperationResult::Panic(code, message) => {
                                self.begin_panic(frame, code, message, span, *unwind)?;
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
                        if task.parent_scope != Some(join.scope) || task.join_consumed {
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
                    OperationResult::Panic(code, message) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                    OperationResult::Value(_) => {
                        return Err(VmError::invariant(
                            "spawn operation did not produce an async child call",
                        ));
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
                                    }),
                                )?;
                            }
                            OperationResult::Panic(code, message) => {
                                self.begin_panic(frame, code, message, span, continuation)?;
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
                        }),
                    )?;
                }
                OperationResult::Panic(code, message) => {
                    self.begin_panic(frame, code, message, span, continuation)?;
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

enum OperationResult {
    Value(Value),
    Call {
        function: BytecodeFunctionId,
        arguments: Vec<Value>,
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
            HeapObject::Iterator { mode, source, next } => {
                let source = match mode {
                    BytecodeCursorMode::Own => self.copy_optional_value(&source)?,
                    BytecodeCursorMode::Ref => source,
                    BytecodeCursorMode::Mut => {
                        return Err(VmError::invariant(
                            "an exclusive iterator was copied as a first-class value",
                        ));
                    }
                };
                let roots = source.iter().cloned().collect::<Vec<_>>();
                self.allocate_like(*handle, HeapObject::Iterator { mode, source, next }, &roots)
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
                    | BytecodeCoercion::CallableErasure => Ok(value_result),
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

    fn represented_scalar(&self, mut ty: BytecodeTypeId) -> Result<BytecodeScalarType, VmError> {
        let mut remaining = self.program.types.len();
        loop {
            if remaining == 0 {
                return Err(VmError::invariant(
                    "verified scalar representation contains an opaque cycle",
                ));
            }
            remaining -= 1;
            match self.program.ty(ty).map(|ty| &ty.kind) {
                Some(BytecodeTypeKind::Scalar(scalar)) => return Ok(*scalar),
                Some(BytecodeTypeKind::OpaqueResult { witness, .. }) => ty = *witness,
                _ => return Err(VmError::invariant("verified scalar type is not scalar")),
            }
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
                match (function, returned) {
                    (BytecodeBootstrapHostFunction::ConsolePrint, RuntimeValue::Unit) => {
                        Ok(OperationResult::Value(Value::Unit))
                    }
                    (BytecodeBootstrapHostFunction::ConsolePrint, _) => Err(VmError::Host(
                        "std.console.print returned a non-Unit value".into(),
                    )),
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
                let snapshots = values
                    .iter()
                    .map(|value| {
                        snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let returned = self.host.invoke(function.name(), &snapshots)?;
                match (function, returned) {
                    (BytecodeBootstrapHostFunction::ConsolePrint, RuntimeValue::Unit) => {
                        Ok(OperationResult::Value(Value::Unit))
                    }
                    (BytecodeBootstrapHostFunction::ConsolePrint, _) => Err(VmError::Host(
                        "std.console.print returned a non-Unit value".into(),
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
        self.display_scalar(result_ty, argument.value.ty, value)
    }

    fn display_scalar(
        &mut self,
        result_ty: BytecodeTypeId,
        input_ty: BytecodeTypeId,
        value: Value,
    ) -> Result<Value, VmError> {
        let scalar = self.represented_scalar(input_ty)?;
        if scalar == BytecodeScalarType::String {
            self.string_value(&value)?;
            return self.copy_value(&value);
        }
        let text = match (scalar, value) {
            (BytecodeScalarType::Unit, Value::Unit) => "()".to_owned(),
            (BytecodeScalarType::Bool, Value::Bool(value)) => value.to_string(),
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
            ) => value.to_string(),
            (BytecodeScalarType::Float, Value::Float(value)) => value.to_string(),
            (BytecodeScalarType::Float32, Value::Float(value)) => (value as f32).to_string(),
            (BytecodeScalarType::Byte, Value::Byte(value)) => value.to_string(),
            (BytecodeScalarType::Char, Value::Char(value)) => value.to_string(),
            _ => {
                return Err(VmError::invariant(
                    "intrinsic Display value does not match its scalar type",
                ));
            }
        };
        self.allocate(result_ty, HeapObject::String(text), &[])
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
                if values.iter().any(|value| matches!(value, Value::Loan(_))) {
                    return Err(VmError::invariant(
                        "host callables cannot receive borrowed parameters",
                    ));
                }
                let snapshots = values
                    .iter()
                    .map(|value| {
                        snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let returned = self.host.invoke(&metadata.name, &snapshots)?;
                Ok(OperationResult::Value(
                    self.materialize_host_value(metadata.outcome, returned)?,
                ))
            }
        })();
        self.temporary_roots.truncate(marker);
        result
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
        let HeapObject::Iterator { mode, source, next } = self.heap.get(handle)?.clone() else {
            return Err(VmError::invariant(
                "iterator state has the wrong heap shape",
            ));
        };
        if next == usize::MAX {
            return Ok(Ok(None));
        }
        let source = present(&source, "iterator source")?.clone();
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
            },
            &roots,
        )?;
        Ok(Ok(item))
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
    let Some((_, maximum)) = integer_bounds(BytecodeScalarType::Int) else {
        return false;
    };
    i128::try_from(length).is_ok_and(|length| length <= maximum)
}

#[cfg(test)]
mod tests {
    use crate::bytecode::{
        BytecodeBinaryOperator, BytecodeConstantValue, BytecodeConstantValueKind,
        BytecodeCursorMode, BytecodeIntrinsicType, BytecodeProgram, BytecodeRangeKind,
        BytecodeScalarType, BytecodeType, BytecodeTypeId, BytecodeTypeKind, derive_trace_metadata,
    };

    use super::{
        Engine, PanicCode, RejectingHost, RuntimeType, RuntimeValue, TaskRecord, TaskStatus,
        TaskWait, Value, ValueCopyStrategy, VmLimits, snapshot_value,
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
        } = engine.heap.get(original).unwrap().clone()
        else {
            unreachable!("the original cursor retains its iterator shape")
        };
        let super::HeapObject::Iterator {
            mode: copied_mode,
            source: copied_source,
            next: copied_next,
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
}
