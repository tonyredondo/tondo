use std::cmp::Ordering;

use crate::bytecode::{
    BytecodeAggregateKind, BytecodeBinaryOperator, BytecodeBlockId, BytecodeBootstrapHostFunction,
    BytecodeCallArgument, BytecodeCallArgumentTarget, BytecodeCoercion, BytecodeConstant,
    BytecodeConstantValue, BytecodeConstantValueKind, BytecodeConstantVariantValue,
    BytecodeContainmentKind, BytecodeFunctionId, BytecodeIndexAccess, BytecodeInstruction,
    BytecodeInstructionKind, BytecodeIntrinsicType, BytecodeNumericConversion, BytecodeOperand,
    BytecodeOperandKind, BytecodeOperation, BytecodeOperationKind, BytecodeParameterMode,
    BytecodePlace, BytecodePrefixOperator, BytecodeProgram, BytecodeProjection,
    BytecodeProjectionKind, BytecodeRangeKind, BytecodeRvalue, BytecodeRvalueKind,
    BytecodeScalarType, BytecodeSpan, BytecodeTag, BytecodeTerminator, BytecodeTerminatorKind,
    BytecodeTypeId, BytecodeTypeKind, BytecodeVerificationLimits, verify_bytecode_with_limits,
};

use super::heap::{Heap, HeapHandle, HeapObject};
use super::literal;
use super::value::{AggregatePayload, Value, snapshot_value};
use super::{PanicCode, RuntimeValue, VmError, VmLimits, VmPanic, VmStackFrame, VmStatistics};

/// Host boundary for callables that deliberately have no bytecode body.
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
    validate_limits(limits)?;
    verify_bytecode_with_limits(
        program,
        BytecodeVerificationLimits {
            max_dataflow_steps: limits.max_verification_steps,
        },
    )?;
    Engine::new(program, host, limits).run(entry)
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

#[derive(Debug)]
struct Frame {
    function: BytecodeFunctionId,
    block: BytecodeBlockId,
    instruction: usize,
    slots: Vec<SlotState>,
    continuation: Option<CallContinuation>,
}

impl Frame {
    fn roots(&self, output: &mut Vec<Value>) {
        output.extend(self.slots.iter().filter_map(|slot| match slot {
            SlotState::Value(value) => Some(value.clone()),
            SlotState::Dead | SlotState::Uninitialized => None,
        }));
    }
}

struct Engine<'program, 'host> {
    program: &'program BytecodeProgram,
    host: &'host mut dyn VmHost,
    limits: VmLimits,
    heap: Heap,
    frames: Vec<Frame>,
    pending_panic: Option<VmPanic>,
    statistics: VmStatistics,
    callable_names: Vec<String>,
    nominal_names: Vec<String>,
}

impl<'program, 'host> Engine<'program, 'host> {
    fn new(
        program: &'program BytecodeProgram,
        host: &'host mut dyn VmHost,
        limits: VmLimits,
    ) -> Self {
        Self {
            program,
            host,
            limits,
            heap: Heap::new(limits),
            frames: Vec::new(),
            pending_panic: None,
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
        self.push_frame(entry, Vec::new(), None)?;

        loop {
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
                continue;
            }
            let terminator = block.terminator.clone();
            if let Some(outcome) = self.execute_terminator(frame_index, &terminator)? {
                self.heap.collect(&[], &mut self.statistics)?;
                return Ok(VmExecution {
                    outcome,
                    statistics: self.statistics,
                });
            }
        }
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
                BytecodeInstructionKind::Store { .. } => None,
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
        self.frames.push(Frame {
            function: function_id,
            block: function.entry,
            instruction: 0,
            slots,
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
            BytecodeInstructionKind::Store { destination, value } => {
                let value = self.evaluate_rvalue(frame, value)?;
                self.write_place(frame, destination, value)?;
            }
        }
        Ok(())
    }

    fn execute_terminator(
        &mut self,
        frame: usize,
        terminator: &BytecodeTerminator,
    ) -> Result<Option<VmOutcome>, VmError> {
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
            BytecodeTerminatorKind::IteratorNext {
                state,
                destination,
                has_value,
                exhausted,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                match self.iterator_next(frame, state, span)? {
                    Ok(Some(value)) => {
                        self.write_place(frame, destination, value)?;
                        self.jump(frame, *has_value);
                    }
                    Ok(None) => self.jump(frame, *exhausted),
                    Err((code, message)) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                }
            }
            BytecodeTerminatorKind::ValidatePlaces {
                places,
                replacements,
                for_write,
                target,
                unwind,
            } => {
                let span = self.resolve_span(frame, terminator.span)?;
                let result = self.validate_places(frame, places, replacements, *for_write);
                match result {
                    Ok(()) => self.jump(frame, *target),
                    Err(PlaceFailure::Panic(code, message)) => {
                        self.begin_panic(frame, code, message, span, *unwind)?;
                    }
                    Err(PlaceFailure::Vm(error)) => return Err(error),
                }
            }
            BytecodeTerminatorKind::Return => {
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
                    let value = snapshot_value(
                        &value,
                        &self.heap,
                        &self.callable_names,
                        &self.nominal_names,
                    )?;
                    return Ok(Some(VmOutcome::Returned(value)));
                }
            }
            BytecodeTerminatorKind::ResumePanic => {
                if self.pending_panic.is_none() {
                    return Err(VmError::invariant(
                        "ResumePanic executed without an active panic",
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
                    self.jump(caller, continuation.unwind);
                } else {
                    let panic = self.pending_panic.take().ok_or_else(|| {
                        VmError::invariant("root panic disappeared during unwind")
                    })?;
                    return Ok(Some(VmOutcome::Panicked(panic)));
                }
            }
            BytecodeTerminatorKind::Unreachable => {
                return Err(VmError::invariant("executed unreachable bytecode"));
            }
        }
        Ok(None)
    }

    fn begin_panic(
        &mut self,
        frame: usize,
        code: PanicCode,
        message: String,
        span: BytecodeSpan,
        unwind: BytecodeBlockId,
    ) -> Result<(), VmError> {
        if self.pending_panic.is_some() {
            return Err(VmError::invariant(
                "a second panic began while cleanup was already unwinding",
            ));
        }
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
        self.pending_panic = Some(VmPanic {
            code,
            message,
            span,
            stack,
        });
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

    fn roots(&self, extra: &[Value]) -> Vec<Value> {
        let mut roots = extra.to_vec();
        for frame in &self.frames {
            frame.roots(&mut roots);
        }
        roots
    }

    fn allocate(&mut self, object: HeapObject, extra: &[Value]) -> Result<Value, VmError> {
        let roots = self.roots(extra);
        self.heap
            .allocate(object, &roots, &mut self.statistics)
            .map(Value::Heap)
    }

    fn replace_object(
        &mut self,
        handle: HeapHandle,
        object: HeapObject,
        extra: &[Value],
    ) -> Result<(), VmError> {
        let roots = self.roots(extra);
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
            BytecodeConstant::Integer(spelling) => literal::integer(spelling)
                .map(Value::Integer)
                .ok_or_else(|| VmError::invariant("verified integer literal is malformed")),
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
                self.allocate(HeapObject::String(text), &[])
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
            BytecodeConstantValueKind::Integer(value) => Ok(Value::Integer(*value)),
            BytecodeConstantValueKind::Float(bits) => {
                let value = if self.scalar(constant.ty)? == BytecodeScalarType::Float32 {
                    f64::from(f32::from_bits(*bits as u32))
                } else {
                    f64::from_bits(*bits)
                };
                Ok(Value::Float(value))
            }
            BytecodeConstantValueKind::Char(value) => Ok(Value::Char(*value)),
            BytecodeConstantValueKind::String(value) => {
                self.allocate(HeapObject::String(value.clone()), &[])
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
                    HeapObject::Tuple(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            BytecodeConstantValueKind::Array(values) => {
                let values = self.materialize_constants(values)?;
                self.allocate(
                    HeapObject::Array(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            BytecodeConstantValueKind::Map(entries) => {
                let mut output = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    output.push((
                        Some(self.materialize_constant(key)?),
                        Some(self.materialize_constant(value)?),
                    ));
                }
                self.allocate(HeapObject::Map(output), &[])
            }
            BytecodeConstantValueKind::Set(values) => {
                let values = self.materialize_constants(values)?;
                self.allocate(HeapObject::Set(values.into_iter().map(Some).collect()), &[])
            }
            BytecodeConstantValueKind::Newtype { nominal, value } => {
                let value = self.materialize_constant(value)?;
                self.allocate(
                    HeapObject::Newtype {
                        nominal: *nominal,
                        value: Some(value.clone()),
                    },
                    &[value],
                )
            }
            BytecodeConstantValueKind::Record { nominal, fields } => {
                let mut output = Vec::with_capacity(fields.len());
                for (field, value) in fields {
                    output.push((*field, Some(self.materialize_constant(value)?)));
                }
                let roots = output
                    .iter()
                    .filter_map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                self.allocate(
                    HeapObject::Record {
                        nominal: *nominal,
                        fields: output,
                    },
                    &roots,
                )
            }
            BytecodeConstantValueKind::Variant { variant, payload } => {
                let payload = self.materialize_constant_payload(payload)?;
                let mut roots = Vec::new();
                payload.trace_values(&mut roots);
                self.allocate(
                    HeapObject::Variant {
                        variant: *variant,
                        payload,
                    },
                    &roots,
                )
            }
            BytecodeConstantValueKind::OptionNone => self.allocate(HeapObject::OptionNone, &[]),
            BytecodeConstantValueKind::OptionSome(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(HeapObject::OptionSome(Some(value.clone())), &[value])
            }
            BytecodeConstantValueKind::ResultOk(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(HeapObject::ResultOk(Some(value.clone())), &[value])
            }
            BytecodeConstantValueKind::ResultErr(value) => {
                let value = self.materialize_constant(value)?;
                self.allocate(HeapObject::ResultErr(Some(value.clone())), &[value])
            }
            BytecodeConstantValueKind::Range { kind, start, end } => {
                let start = self.materialize_constant(start)?;
                let end = self.materialize_constant(end)?;
                self.allocate(
                    HeapObject::Range {
                        kind: *kind,
                        start: Some(start.clone()),
                        end: Some(end.clone()),
                    },
                    &[start, end],
                )
            }
        }
    }

    fn materialize_constants(
        &mut self,
        constants: &[BytecodeConstantValue],
    ) -> Result<Vec<Value>, VmError> {
        constants
            .iter()
            .map(|constant| self.materialize_constant(constant))
            .collect()
    }

    fn materialize_constant_payload(
        &mut self,
        payload: &BytecodeConstantVariantValue,
    ) -> Result<AggregatePayload, VmError> {
        Ok(match payload {
            BytecodeConstantVariantValue::Unit => AggregatePayload::Unit,
            BytecodeConstantVariantValue::Tuple(values) => AggregatePayload::Tuple(
                self.materialize_constants(values)?
                    .into_iter()
                    .map(Some)
                    .collect(),
            ),
            BytecodeConstantVariantValue::Record(fields) => {
                let mut output = Vec::with_capacity(fields.len());
                for (field, value) in fields {
                    output.push((*field, Some(self.materialize_constant(value)?)));
                }
                AggregatePayload::Record(output)
            }
        })
    }

    fn copy_value(&mut self, value: &Value) -> Result<Value, VmError> {
        let Value::Heap(handle) = value else {
            return Ok(value.clone());
        };
        let object = self.heap.get(*handle)?.clone();
        match object {
            HeapObject::String(_) | HeapObject::Ref(_) => Ok(value.clone()),
            HeapObject::Iterator { .. } => Err(VmError::invariant(
                "verified bytecode attempted to copy an affine iterator",
            )),
            HeapObject::Tuple(values) => {
                let values = self.copy_optional_values(&values)?;
                self.allocate(HeapObject::Tuple(values), &[])
            }
            HeapObject::Array(values) => {
                let values = self.copy_optional_values(&values)?;
                self.allocate(HeapObject::Array(values), &[])
            }
            HeapObject::Map(entries) => {
                let mut output = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    output.push((
                        self.copy_optional_value(&key)?,
                        self.copy_optional_value(&value)?,
                    ));
                }
                self.allocate(HeapObject::Map(output), &[])
            }
            HeapObject::Set(values) => {
                let values = self.copy_optional_values(&values)?;
                self.allocate(HeapObject::Set(values), &[])
            }
            HeapObject::Newtype { nominal, value } => {
                let value = self.copy_optional_value(&value)?;
                self.allocate(HeapObject::Newtype { nominal, value }, &[])
            }
            HeapObject::Record { nominal, fields } => {
                let mut output = Vec::with_capacity(fields.len());
                for (field, value) in fields {
                    output.push((field, self.copy_optional_value(&value)?));
                }
                self.allocate(
                    HeapObject::Record {
                        nominal,
                        fields: output,
                    },
                    &[],
                )
            }
            HeapObject::Variant { variant, payload } => {
                let payload = self.copy_payload(&payload)?;
                self.allocate(HeapObject::Variant { variant, payload }, &[])
            }
            HeapObject::OptionNone => self.allocate(HeapObject::OptionNone, &[]),
            HeapObject::OptionSome(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate(HeapObject::OptionSome(value), &[])
            }
            HeapObject::ResultOk(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate(HeapObject::ResultOk(value), &[])
            }
            HeapObject::ResultErr(value) => {
                let value = self.copy_optional_value(&value)?;
                self.allocate(HeapObject::ResultErr(value), &[])
            }
            HeapObject::Union { member, value } => {
                let value = self.copy_optional_value(&value)?;
                self.allocate(HeapObject::Union { member, value }, &[])
            }
            HeapObject::Range { kind, start, end } => {
                let start = self.copy_optional_value(&start)?;
                let end = self.copy_optional_value(&end)?;
                self.allocate(HeapObject::Range { kind, start, end }, &[])
            }
        }
    }

    fn copy_optional_values(
        &mut self,
        values: &[Option<Value>],
    ) -> Result<Vec<Option<Value>>, VmError> {
        values
            .iter()
            .map(|value| self.copy_optional_value(value))
            .collect()
    }

    fn copy_optional_value(&mut self, value: &Option<Value>) -> Result<Option<Value>, VmError> {
        value
            .as_ref()
            .map(|value| self.copy_value(value))
            .transpose()
    }

    fn copy_payload(&mut self, payload: &AggregatePayload) -> Result<AggregatePayload, VmError> {
        Ok(match payload {
            AggregatePayload::Unit => AggregatePayload::Unit,
            AggregatePayload::Tuple(values) => {
                AggregatePayload::Tuple(self.copy_optional_values(values)?)
            }
            AggregatePayload::Record(fields) => {
                let mut output = Vec::with_capacity(fields.len());
                for (field, value) in fields {
                    output.push((*field, self.copy_optional_value(value)?));
                }
                AggregatePayload::Record(output)
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
            } => {
                let left_value = self.evaluate_operand(frame, left)?;
                let right_value = self.evaluate_operand(frame, right)?;
                self.pure_binary(*operator, left.ty, right.ty, left_value, right_value)
            }
            BytecodeRvalueKind::Construct { shape, values } => {
                let values = self.evaluate_operands(frame, values)?;
                self.construct_aggregate(shape, values)
            }
            BytecodeRvalueKind::RecordUpdate { base, fields } => {
                let base = self.evaluate_operand(frame, base)?;
                let Value::Heap(handle) = base else {
                    return Err(VmError::invariant("record update base is not managed"));
                };
                let HeapObject::Record {
                    nominal,
                    fields: mut output,
                } = self.heap.get(handle)?.clone()
                else {
                    return Err(VmError::invariant("record update base is not a record"));
                };
                for (field, value) in fields {
                    let value = self.evaluate_operand(frame, value)?;
                    let destination = output
                        .iter_mut()
                        .find(|(candidate, _)| candidate == field)
                        .ok_or_else(|| VmError::invariant("record update field is missing"))?;
                    destination.1 = Some(value);
                }
                self.allocate(
                    HeapObject::Record {
                        nominal,
                        fields: output,
                    },
                    &[],
                )
            }
            BytecodeRvalueKind::Coerce { kind, value } => {
                let value_result = self.evaluate_operand(frame, value)?;
                match kind {
                    BytecodeCoercion::Exact | BytecodeCoercion::Opaque => Ok(value_result),
                    BytecodeCoercion::UnionInjection => self.allocate(
                        HeapObject::Union {
                            member: value.ty,
                            value: Some(value_result.clone()),
                        },
                        &[value_result],
                    ),
                    BytecodeCoercion::UnionWidening => Ok(value_result),
                    BytecodeCoercion::OptionLift => self.allocate(
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
                self.numeric_conversion(*target, *conversion, value)
            }
            BytecodeRvalueKind::Range { kind, start, end } => {
                let start = self.evaluate_operand(frame, start)?;
                let end = self.evaluate_operand(frame, end)?;
                self.allocate(
                    HeapObject::Range {
                        kind: *kind,
                        start: Some(start.clone()),
                        end: Some(end.clone()),
                    },
                    &[start, end],
                )
            }
            BytecodeRvalueKind::Contains {
                kind,
                item,
                container,
            } => {
                let item = self.evaluate_operand(frame, item)?;
                let container = self.evaluate_operand(frame, container)?;
                Ok(Value::Bool(self.contains(*kind, &item, &container)?))
            }
            BytecodeRvalueKind::Length(value) => {
                let value = self.evaluate_operand(frame, value)?;
                Ok(Value::Integer(self.length(&value)? as i128))
            }
            BytecodeRvalueKind::IteratorState(value) => {
                let value = self.evaluate_operand(frame, value)?;
                self.allocate(
                    HeapObject::Iterator {
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
        operands
            .iter()
            .map(|operand| self.evaluate_operand(frame, operand))
            .collect()
    }

    fn construct_aggregate(
        &mut self,
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
                HeapObject::Set(unique)
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
        self.allocate(object, &roots)
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
        _left_ty: BytecodeTypeId,
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
                    (Value::Float(left), Value::Float(right)) => Ok(Value::Float(match operator {
                        Op::Multiply => left * right,
                        Op::Divide => left / right,
                        Op::Remainder => left % right,
                        Op::Add => left + right,
                        Op::Subtract => left - right,
                        _ => unreachable!(),
                    })),
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
        target: BytecodeScalarType,
        conversion: BytecodeNumericConversion,
        value: Value,
    ) -> Result<Value, VmError> {
        let converted = convert_numeric(target, &value);
        if conversion == BytecodeNumericConversion::Checked {
            match converted {
                Ok(value) => self.allocate(HeapObject::ResultOk(Some(value.clone())), &[value]),
                Err(variant) => {
                    let error = self.allocate(
                        HeapObject::Variant {
                            variant,
                            payload: AggregatePayload::Unit,
                        },
                        &[],
                    )?;
                    self.allocate(HeapObject::ResultErr(Some(error.clone())), &[error])
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
    fn read_place(&mut self, frame: usize, place: &BytecodePlace) -> Result<Value, VmError> {
        let mut value = self.read_slot(frame, place.slot)?.clone();
        for projection in &place.projections {
            value = self.read_projection(frame, value, projection)?;
        }
        Ok(value)
    }

    fn take_place(&mut self, frame: usize, place: &BytecodePlace) -> Result<Value, VmError> {
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

    fn read_projection(
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
            (BytecodeProjectionKind::Field(member), HeapObject::Record { fields, .. }) => {
                clone_field(&fields, *member, "record field")
            }
            (BytecodeProjectionKind::TupleField(index), HeapObject::Tuple(values)) => {
                clone_index(&values, *index, "tuple field")
            }
            (BytecodeProjectionKind::NewtypeValue, HeapObject::Newtype { value, .. }) => {
                present(&value, "newtype value").cloned()
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
                    output.push(Some(self.copy_value(present(value, "array rest item")?)?));
                }
                self.allocate(HeapObject::Array(output), &[])
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = self.integer_slot(frame, *index)?;
                let index = normalize_index(index, values.len()).ok_or_else(|| {
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
                            self.allocate(HeapObject::OptionSome(Some(value.clone())), &[value])
                        } else {
                            self.allocate(HeapObject::OptionNone, &[])
                        }
                    }
                    BytecodeIndexAccess::MapEntry => {
                        let index = found
                            .ok_or_else(|| VmError::invariant("unvalidated map entry is absent"))?;
                        present(&entries[index].1, "map value").cloned()
                    }
                    BytecodeIndexAccess::Array => Err(VmError::invariant(
                        "array index access was applied to a map",
                    )),
                }
            }
            (BytecodeProjectionKind::Slice { start, end, step }, HeapObject::Array(values)) => {
                let indices = self
                    .slice_indices_from_slots(frame, *start, *end, *step, values.len())
                    .map_err(|_| VmError::invariant("unvalidated slice reached a projection"))?;
                let mut output = Vec::with_capacity(indices.len());
                for index in indices {
                    output.push(Some(
                        self.copy_value(present(&values[index], "slice item")?)?,
                    ));
                }
                self.allocate(HeapObject::Array(output), &[])
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
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = normalize_index(self.integer_slot(frame, *index)?, values.len())
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
        let Value::Heap(handle) = parent else {
            return Err(VmError::invariant("write projection base is not managed"));
        };
        let mut object = self.heap.get(handle)?.clone();
        match (&projection.kind, &mut object) {
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
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let index = normalize_index(self.integer_slot(frame, *index)?, values.len())
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
        for_write: bool,
    ) -> Result<(), PlaceFailure> {
        if places.len() != replacements.len() {
            return Err(PlaceFailure::Vm(VmError::invariant(
                "place validation inputs are not aligned",
            )));
        }
        let mut paths = Vec::with_capacity(places.len());
        for (place, replacement) in places.iter().zip(replacements) {
            let path = self.validate_place(frame, place)?;
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
            } else if replacement.is_some() {
                return Err(PlaceFailure::Vm(VmError::invariant(
                    "non-slice place validation has a replacement operand",
                )));
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

    fn validate_place(
        &mut self,
        frame: usize,
        place: &BytecodePlace,
    ) -> Result<ResolvedPlacePath, PlaceFailure> {
        let mut value = self.read_slot(frame, place.slot)?.clone();
        let mut path = ResolvedPlacePath {
            root: place.slot.index(),
            components: Vec::with_capacity(place.projections.len()),
        };
        for (index, projection) in place.projections.iter().enumerate() {
            let component = self.resolve_place_component(frame, &value, projection)?;
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
    ) -> Result<PlaceComponent, PlaceFailure> {
        let Value::Heap(handle) = parent else {
            return Err(PlaceFailure::Vm(VmError::invariant(
                "place projection base is not managed",
            )));
        };
        let object = self.heap.get(*handle)?;
        Ok(match (&projection.kind, object) {
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
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Array(values))
                if *access == BytecodeIndexAccess::Array =>
            {
                let raw = self.integer_slot(frame, *index)?;
                let index = normalize_index(raw, values.len()).ok_or_else(|| {
                    PlaceFailure::Panic(PanicCode::Bounds, "array index is out of bounds".into())
                })?;
                PlaceComponent::Index(index as i128)
            }
            (BytecodeProjectionKind::Index { index, access }, HeapObject::Map(entries))
                if *access == BytecodeIndexAccess::MapEntry =>
            {
                let key = self.read_slot(frame, *index)?.clone();
                self.find_map_entry(entries, &key)?;
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
            } => {
                let left_value = self.evaluate_operand(frame, left)?;
                let right_value = self.evaluate_operand(frame, right)?;
                Ok(
                    match self.checked_binary(
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
            }
            BytecodeOperationKind::BuildMap {
                entries,
                reject_dynamic_duplicates,
            } => {
                let mut evaluated = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    evaluated.push((
                        self.evaluate_operand(frame, key)?,
                        self.evaluate_operand(frame, value)?,
                    ));
                }
                let mut output: Vec<(Option<Value>, Option<Value>)> =
                    Vec::with_capacity(entries.len());
                for (key, value) in evaluated {
                    if let Some(index) = self.find_map_entry(&output, &key)? {
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
                Ok(OperationResult::Value(
                    self.allocate(HeapObject::Map(output), &[])?,
                ))
            }
            BytecodeOperationKind::Index {
                base,
                index,
                access,
            } => {
                let base = self.evaluate_operand(frame, base)?;
                let index = self.evaluate_operand(frame, index)?;
                Ok(match self.index_value(base, index, *access)? {
                    Ok(value) => OperationResult::Value(value),
                    Err((code, message)) => OperationResult::Panic(code, message),
                })
            }
            BytecodeOperationKind::Slice {
                base,
                start,
                end,
                step,
            } => {
                let base = self.evaluate_operand(frame, base)?;
                let start = start
                    .as_ref()
                    .map(|value| self.evaluate_operand(frame, value))
                    .transpose()?;
                let end = end
                    .as_ref()
                    .map(|value| self.evaluate_operand(frame, value))
                    .transpose()?;
                let step = step
                    .as_ref()
                    .map(|value| self.evaluate_operand(frame, value))
                    .transpose()?;
                Ok(match self.slice_value(base, start, end, step)? {
                    Ok(value) => OperationResult::Value(value),
                    Err((code, message)) => OperationResult::Panic(code, message),
                })
            }
            BytecodeOperationKind::Call { callee, arguments } => {
                let callee = self.evaluate_operand(frame, callee)?;
                self.prepare_call(frame, callee, arguments)
            }
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
            } => {
                let condition = self.evaluate_operand(frame, condition)?;
                let Value::Bool(condition) = condition else {
                    return Err(VmError::invariant("assert condition is not Bool"));
                };
                let mut values = Vec::with_capacity(message_parts.len());
                for part in message_parts {
                    values.push((self.evaluate_operand(frame, &part.value)?, part.spread));
                }
                if condition {
                    Ok(OperationResult::Value(Value::Unit))
                } else {
                    let mut message = String::new();
                    for (value, spread) in values {
                        if spread {
                            let Value::Heap(handle) = value else {
                                return Err(VmError::invariant(
                                    "spread assert message is not managed",
                                ));
                            };
                            let HeapObject::Array(parts) = self.heap.get(handle)?.clone() else {
                                return Err(VmError::invariant(
                                    "spread assert message is not an Array",
                                ));
                            };
                            for part in parts {
                                let part = present(&part, "assert message part")?;
                                message.push_str(self.string_value(part)?);
                            }
                        } else {
                            message.push_str(self.string_value(&value)?);
                        }
                    }
                    if message_parts.is_empty() {
                        message = format!("assertion failed: {condition_repr}");
                    }
                    Ok(OperationResult::Panic(PanicCode::AssertionFailed, message))
                }
            }
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
                let (_, bits) = if scalar == BytecodeScalarType::Byte {
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
                if operator == Op::ShiftLeft {
                    left.checked_shl(count)
                } else {
                    left.checked_shr(count)
                }
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
            let element = self.checked_binary(
                operator,
                left_element.unwrap_or(left_ty),
                right_element.unwrap_or(right_ty),
                result_element,
                left_value?,
                right_value?,
            )?;
            match element {
                Ok(value) => output.push(Some(value)),
                Err(panic) => return Ok(Err(panic)),
            }
        }
        Ok(Ok(self.allocate(HeapObject::Array(output), &[])?))
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
            HeapObject::Array(values) => Ok(values.clone()),
            _ => Err(VmError::invariant("Array value has the wrong heap shape")),
        }
    }

    fn index_value(
        &mut self,
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
                let Some(index) = normalize_index(index, values.len()) else {
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
            (BytecodeIndexAccess::MapLookup, HeapObject::Map(entries)) => {
                if let Some(position) = self.find_map_entry(&entries, &index)? {
                    let value = self.copy_value(present(&entries[position].1, "map value")?)?;
                    Ok(Ok(self.allocate(
                        HeapObject::OptionSome(Some(value.clone())),
                        &[value],
                    )?))
                } else {
                    Ok(Ok(self.allocate(HeapObject::OptionNone, &[])?))
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

    fn slice_value(
        &mut self,
        base: Value,
        start: Option<Value>,
        end: Option<Value>,
        step: Option<Value>,
    ) -> Result<Result<Value, (PanicCode, String)>, VmError> {
        let Value::Heap(handle) = base else {
            return Err(VmError::invariant("slice base is not managed"));
        };
        let HeapObject::Array(values) = self.heap.get(handle)?.clone() else {
            return Err(VmError::invariant("slice base is not Array"));
        };
        let integer = |value: Option<Value>, label: &str| -> Result<Option<i128>, VmError> {
            value
                .map(|value| match value {
                    Value::Integer(value) => Ok(value),
                    _ => Err(VmError::invariant(format!("slice {label} is not Int"))),
                })
                .transpose()
        };
        let indices = match slice_indices(
            integer(start, "start")?,
            integer(end, "end")?,
            integer(step, "step")?,
            values.len(),
        ) {
            Ok(indices) => indices,
            Err(panic) => return Ok(Err(panic)),
        };
        let mut output = Vec::with_capacity(indices.len());
        for index in indices {
            output.push(Some(
                self.copy_value(present(&values[index], "slice item")?)?,
            ));
        }
        Ok(Ok(self.allocate(HeapObject::Array(output), &[])?))
    }

    fn prepare_call(
        &mut self,
        frame: usize,
        callee: Value,
        arguments: &[BytecodeCallArgument],
    ) -> Result<OperationResult, VmError> {
        let Value::Function {
            callable,
            arguments: _type_arguments,
        } = callee
        else {
            return Err(VmError::invariant("call callee is not a function value"));
        };
        let metadata = self
            .program
            .callable(callable)
            .ok_or_else(|| VmError::invariant("callable metadata index is invalid"))?
            .clone();
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
        for argument in arguments {
            if argument.mode != BytecodeParameterMode::Value {
                return Err(VmError::invariant(
                    "borrowed calls await the M5 ownership runtime",
                ));
            }
            let value = self.evaluate_operand(frame, &argument.value)?;
            match argument.target {
                BytecodeCallArgumentTarget::Receiver => {
                    let index = receiver.ok_or_else(|| {
                        VmError::invariant("call provides a receiver to a free function")
                    })?;
                    values[index] = Some(value);
                }
                BytecodeCallArgumentTarget::Fixed(index) => {
                    let slot = values
                        .get_mut(index as usize)
                        .ok_or_else(|| VmError::invariant("fixed call target index is invalid"))?;
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
                        variadic_values.push(self.copy_value(present(&item, "variadic item")?)?);
                    }
                }
            }
        }
        if let Some(index) = variadic {
            values[index] = Some(self.allocate(
                HeapObject::Array(variadic_values.into_iter().map(Some).collect()),
                &[],
            )?);
        }
        let values = values
            .into_iter()
            .map(|value| value.ok_or_else(|| VmError::invariant("call parameter is uninitialized")))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(function) = metadata.implementation {
            Ok(OperationResult::Call {
                function,
                arguments: values,
            })
        } else {
            let snapshots = values
                .iter()
                .map(|value| {
                    snapshot_value(value, &self.heap, &self.callable_names, &self.nominal_names)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let returned = self.host.invoke(&metadata.name, &snapshots)?;
            Ok(OperationResult::Value(
                self.materialize_host_value(returned)?,
            ))
        }
    }

    fn materialize_host_value(&mut self, value: RuntimeValue) -> Result<Value, VmError> {
        match value {
            RuntimeValue::Unit => Ok(Value::Unit),
            RuntimeValue::Bool(value) => Ok(Value::Bool(value)),
            RuntimeValue::Integer(value) => Ok(Value::Integer(value)),
            RuntimeValue::Float(value) => Ok(Value::Float(value)),
            RuntimeValue::Byte(value) => Ok(Value::Byte(value)),
            RuntimeValue::Char(value) => Ok(Value::Char(value)),
            RuntimeValue::String(value) => self.allocate(HeapObject::String(value), &[]),
            RuntimeValue::Tuple(values) => {
                let values = self.materialize_host_values(values)?;
                self.allocate(
                    HeapObject::Tuple(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            RuntimeValue::Array(values) => {
                let values = self.materialize_host_values(values)?;
                self.allocate(
                    HeapObject::Array(values.into_iter().map(Some).collect()),
                    &[],
                )
            }
            RuntimeValue::OptionNone => self.allocate(HeapObject::OptionNone, &[]),
            RuntimeValue::OptionSome(value) => {
                let value = self.materialize_host_value(*value)?;
                self.allocate(HeapObject::OptionSome(Some(value.clone())), &[value])
            }
            RuntimeValue::ResultOk(value) => {
                let value = self.materialize_host_value(*value)?;
                self.allocate(HeapObject::ResultOk(Some(value.clone())), &[value])
            }
            RuntimeValue::ResultErr(value) => {
                let value = self.materialize_host_value(*value)?;
                self.allocate(HeapObject::ResultErr(Some(value.clone())), &[value])
            }
            RuntimeValue::Map(_)
            | RuntimeValue::Set(_)
            | RuntimeValue::Function { .. }
            | RuntimeValue::Newtype { .. }
            | RuntimeValue::Record { .. }
            | RuntimeValue::Variant { .. }
            | RuntimeValue::Union { .. }
            | RuntimeValue::Range { .. }
            | RuntimeValue::Ref(_)
            | RuntimeValue::Cycle(_) => Err(VmError::Host(
                "bootstrap host returned an unsupported managed value".into(),
            )),
        }
    }

    fn materialize_host_values(
        &mut self,
        values: Vec<RuntimeValue>,
    ) -> Result<Vec<Value>, VmError> {
        values
            .into_iter()
            .map(|value| self.materialize_host_value(value))
            .collect()
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
        _span: BytecodeSpan,
    ) -> Result<Result<Option<Value>, (PanicCode, String)>, VmError> {
        let iterator = self.read_place(frame, state)?;
        let Value::Heap(handle) = iterator else {
            return Err(VmError::invariant("iterator state is not managed"));
        };
        let HeapObject::Iterator { source, next } = self.heap.get(handle)?.clone() else {
            return Err(VmError::invariant(
                "iterator state has the wrong heap shape",
            ));
        };
        if next == usize::MAX {
            return Ok(Ok(None));
        }
        let source = present(&source, "iterator source")?.clone();
        let (item, next_index) = self.iterator_item(&source, next)?;
        self.replace_object(
            handle,
            HeapObject::Iterator {
                source: Some(source.clone()),
                next: next_index,
            },
            &[source],
        )?;
        Ok(Ok(item))
    }

    fn iterator_item(
        &mut self,
        source: &Value,
        next: usize,
    ) -> Result<(Option<Value>, usize), VmError> {
        let Value::Heap(handle) = source else {
            return Err(VmError::invariant("iterator source is not managed"));
        };
        match self.heap.get(*handle)?.clone() {
            HeapObject::Array(values) | HeapObject::Set(values) => {
                let Some(value) = values.get(next) else {
                    return Ok((None, usize::MAX));
                };
                Ok((
                    Some(self.copy_value(present(value, "iterator item")?)?),
                    next.saturating_add(1),
                ))
            }
            HeapObject::Map(entries) => {
                let Some((key, value)) = entries.get(next) else {
                    return Ok((None, usize::MAX));
                };
                let key = self.copy_value(present(key, "map iterator key")?)?;
                let value = self.copy_value(present(value, "map iterator value")?)?;
                let tuple = self.allocate(
                    HeapObject::Tuple(vec![Some(key.clone()), Some(value.clone())]),
                    &[key, value],
                )?;
                Ok((Some(tuple), next.saturating_add(1)))
            }
            HeapObject::String(text) => {
                let Some(value) = text.chars().nth(next) else {
                    return Ok((None, usize::MAX));
                };
                Ok((Some(Value::Char(value)), next.saturating_add(1)))
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
    root: u32,
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

fn normalize_index(index: i128, length: usize) -> Option<usize> {
    let length = i128::try_from(length).ok()?;
    let normalized = if index < 0 {
        length.checked_add(index)?
    } else {
        index
    };
    (0..length)
        .contains(&normalized)
        .then(|| usize::try_from(normalized).ok())
        .flatten()
}

fn slice_indices(
    start: Option<i128>,
    end: Option<i128>,
    step: Option<i128>,
    length: usize,
) -> Result<Vec<usize>, (PanicCode, String)> {
    let step = step.unwrap_or(1);
    if step == 0 {
        return Err((PanicCode::ZeroSliceStep, "slice step cannot be zero".into()));
    }
    let length = i128::try_from(length).map_err(|_| {
        (
            PanicCode::Bounds,
            "array length is not representable as Int".into(),
        )
    })?;
    let normalize_positive = |value: i128| {
        let value = if value < 0 {
            length.saturating_add(value)
        } else {
            value
        };
        value.clamp(0, length)
    };
    let normalize_negative = |value: i128| {
        let value = if value < 0 {
            length.saturating_add(value)
        } else {
            value
        };
        value.clamp(-1, length.saturating_sub(1))
    };
    let mut output = Vec::new();
    if step > 0 {
        let mut index = start.map_or(0, normalize_positive);
        let end = end.map_or(length, normalize_positive);
        while index < end {
            output.push(index as usize);
            let Some(next) = index.checked_add(step) else {
                break;
            };
            index = next;
        }
    } else {
        let mut index = start.map_or(length - 1, normalize_negative);
        let end = end.map_or(-1, normalize_negative);
        while index > end {
            output.push(index as usize);
            let Some(next) = index.checked_add(step) else {
                break;
            };
            index = next;
        }
    }
    Ok(output)
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
        (HeapObject::Tuple(left), HeapObject::Tuple(right))
        | (HeapObject::Array(left), HeapObject::Array(right)) => {
            queue_options(left, right, pending)?
        }
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

fn convert_numeric(target: BytecodeScalarType, value: &Value) -> Result<Value, u32> {
    let integer_target = integer_bounds(target);
    match value {
        Value::Integer(value) => {
            if let Some((minimum, maximum)) = integer_target {
                if (minimum..=maximum).contains(value) {
                    Ok(Value::Integer(*value))
                } else {
                    Err(0)
                }
            } else if target == BytecodeScalarType::Byte {
                u8::try_from(*value).map(Value::Byte).map_err(|_| 0)
            } else if target == BytecodeScalarType::Float32 {
                Ok(Value::Float(f64::from(*value as f32)))
            } else if target == BytecodeScalarType::Float {
                Ok(Value::Float(*value as f64))
            } else {
                Err(0)
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
                    Err(0)
                } else {
                    Ok(Value::Float(f64::from(converted)))
                }
            } else {
                if !value.is_finite() {
                    return Err(1);
                }
                if value.fract() != 0.0 {
                    return Err(2);
                }
                if target == BytecodeScalarType::Byte {
                    if (0.0..=255.0).contains(value) {
                        Ok(Value::Byte(*value as u8))
                    } else {
                        Err(0)
                    }
                } else if let Some((minimum, maximum)) = integer_target {
                    if *value >= minimum as f64 && *value <= maximum as f64 {
                        let converted = *value as i128;
                        if converted >= minimum && converted <= maximum {
                            Ok(Value::Integer(converted))
                        } else {
                            Err(0)
                        }
                    } else {
                        Err(0)
                    }
                } else {
                    Err(0)
                }
            }
        }
        Value::Unit | Value::Bool(_) | Value::Char(_) | Value::Function { .. } | Value::Heap(_) => {
            Err(0)
        }
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
