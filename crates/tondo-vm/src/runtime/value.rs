use std::collections::BTreeMap;

use crate::bytecode::{
    BytecodeCallableId, BytecodeNominalId, BytecodeParameterMode, BytecodePlace, BytecodeRangeKind,
    BytecodeTraceDescriptor, BytecodeTypeId,
};

use super::heap::{Heap, HeapHandle, HeapObject};
use super::{RuntimeValue, VmError, VmMemoryBudget, VmMemoryCharge};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    Unit,
    Bool(bool),
    Integer(i128),
    Float(f64),
    Byte(u8),
    Char(char),
    Function {
        callable: BytecodeCallableId,
        arguments: Vec<BytecodeTypeId>,
    },
    Loan(RuntimeLoan),
    Join(RuntimeJoin),
    Host(RuntimeValue),
    Heap(HeapHandle),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeLoan {
    pub(super) task: usize,
    pub(super) frame: usize,
    pub(super) place: BytecodePlace,
    pub(super) mode: BytecodeParameterMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RuntimeJoin {
    pub(super) task: usize,
    pub(super) scope: usize,
}

/// Scope marker used after an explicit `return` transfers a `Join` to the
/// caller.  The child remains affine and must still be consumed, but it is no
/// longer owned by a lexical task scope.
pub(super) const TRANSFERRED_JOIN_SCOPE: usize = usize::MAX;

impl Value {
    pub(super) fn heap_handle(&self) -> Option<HeapHandle> {
        match self {
            Self::Heap(handle) => Some(*handle),
            Self::Unit
            | Self::Bool(_)
            | Self::Integer(_)
            | Self::Float(_)
            | Self::Byte(_)
            | Self::Char(_)
            | Self::Function { .. }
            | Self::Loan(_)
            | Self::Join(_)
            | Self::Host(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum AggregatePayload {
    Unit,
    Tuple(Vec<Option<Value>>),
    Record(Vec<(u32, Option<Value>)>),
}

impl AggregatePayload {
    pub(super) fn trace_values(&self, output: &mut Vec<Value>) {
        match self {
            Self::Unit => {}
            Self::Tuple(values) => output.extend(values.iter().flatten().cloned()),
            Self::Record(fields) => {
                output.extend(fields.iter().filter_map(|(_, value)| value.clone()));
            }
        }
    }
}

/// The charge travels with every detached argument until its consuming call
/// or async admission has finished. It is not a managed-heap root.
#[derive(Debug)]
pub(super) struct SnapshotArguments {
    pub(super) values: Vec<RuntimeValue>,
    pub(super) memory: Option<VmMemoryCharge>,
}

impl SnapshotArguments {
    /// Move internal detached arguments into dispatch without a second copy.
    /// Their construction is separate from this transport admission.
    pub(super) fn admit_owned(
        values: Vec<RuntimeValue>,
        budget: Option<&VmMemoryBudget>,
    ) -> Result<Self, VmError> {
        let memory = budget
            .map(|budget| {
                let bytes = values.iter().try_fold(0_u64, |sum, value| {
                    sum.checked_add(value.measure_retained_bytes(Some(budget))?)
                        .ok_or(VmError::ResourceLimit {
                            resource: "memory",
                            limit: budget.limit(),
                        })
                })?;
                budget.reserve(bytes)
            })
            .transpose()?;
        Ok(Self { values, memory })
    }
}

impl std::ops::Deref for SnapshotArguments {
    type Target = [RuntimeValue];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

/// Admit the whole batch before copying any payload. Repeated references are
/// separate detached copies, while cycles retain the existing path markers.
pub(super) fn snapshot_arguments(
    values: &[Value],
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
    budget: Option<&VmMemoryBudget>,
) -> Result<SnapshotArguments, VmError> {
    let Some(budget) = budget else {
        return Ok(SnapshotArguments {
            values: values
                .iter()
                .map(|value| snapshot_value(value, heap, callable_names, nominal_names))
                .collect::<Result<_, _>>()?,
            memory: None,
        });
    };
    admit_snapshot_inputs(
        values.iter().map(SnapshotInput::Managed),
        Some(heap),
        callable_names,
        nominal_names,
        budget,
    )
}

/// A borrowed host adapter needs the same batch and workspace admission as a
/// managed caller before making its own detached copies.
pub(super) fn snapshot_detached_arguments(
    values: &[RuntimeValue],
    budget: Option<&VmMemoryBudget>,
) -> Result<SnapshotArguments, VmError> {
    let Some(budget) = budget else {
        return Ok(SnapshotArguments {
            values: values.to_vec(),
            memory: None,
        });
    };
    admit_snapshot_inputs(
        values.iter().map(SnapshotInput::Detached),
        None,
        &[],
        &[],
        budget,
    )
}

fn admit_snapshot_inputs<'a>(
    inputs: impl ExactSizeIterator<Item = SnapshotInput<'a>> + Clone,
    heap: Option<&'a Heap>,
    callable_names: &[String],
    nominal_names: &[String],
    budget: &VmMemoryBudget,
) -> Result<SnapshotArguments, VmError> {
    let mut admission = SnapshotAdmission {
        heap,
        callable_names,
        nominal_names,
        memory: budget.reserve(0)?,
        nodes: 0,
        depth: 0,
    };
    for input in inputs.clone() {
        match input {
            SnapshotInput::Managed(value) => admission.value(value, None, 1)?,
            SnapshotInput::Detached(value) => admission.detached(value, 1)?,
        }
    }
    // Copy jobs use one descriptor per node. Pending heap exits and the
    // visited-handle map share the per-depth allowance. No source payload is
    // cloned until the complete output and workspace have been admitted.
    let workspace = admission
        .nodes
        .checked_mul(super::TEST_DETACHED_VALUE_BYTES)
        .and_then(|bytes| {
            bytes.checked_add(admission.depth as u64 * super::TEST_SNAPSHOT_FRAME_BYTES)
        })
        .ok_or_else(|| admission.memory_error())?;
    let _workspace = budget.reserve(workspace)?;
    let values = copy_snapshot_inputs(inputs, heap, callable_names, nominal_names)?;
    Ok(SnapshotArguments {
        values,
        memory: Some(admission.memory),
    })
}

#[derive(Clone, Copy)]
enum SnapshotInput<'a> {
    Managed(&'a Value),
    Detached(&'a RuntimeValue),
}

enum SnapshotChildren<'a> {
    Empty,
    Pair(Option<SnapshotInput<'a>>, Option<SnapshotInput<'a>>),
    Managed(std::slice::Iter<'a, Option<Value>>),
    Fields(std::slice::Iter<'a, (u32, Option<Value>)>),
    ManagedMap {
        entries: std::slice::Iter<'a, (Option<Value>, Option<Value>)>,
        next_value: Option<&'a Option<Value>>,
    },
    Detached(std::slice::Iter<'a, RuntimeValue>),
    DetachedMap {
        entries: std::slice::Iter<'a, (RuntimeValue, RuntimeValue)>,
        next_value: Option<&'a RuntimeValue>,
    },
}

impl<'a> SnapshotChildren<'a> {
    fn next(&mut self) -> Result<Option<SnapshotInput<'a>>, VmError> {
        Ok(match self {
            Self::Empty => None,
            Self::Pair(first, second) => first.take().or_else(|| second.take()),
            Self::Managed(values) => values
                .next()
                .map(|value| present_value(value).map(SnapshotInput::Managed))
                .transpose()?,
            Self::Fields(fields) => fields
                .next()
                .map(|(_, value)| present_value(value).map(SnapshotInput::Managed))
                .transpose()?,
            Self::ManagedMap {
                entries,
                next_value,
            } => {
                if let Some(value) = next_value.take() {
                    Some(SnapshotInput::Managed(present_value(value)?))
                } else if let Some((key, value)) = entries.next() {
                    *next_value = Some(value);
                    Some(SnapshotInput::Managed(present_value(key)?))
                } else {
                    None
                }
            }
            Self::Detached(values) => values.next().map(SnapshotInput::Detached),
            Self::DetachedMap {
                entries,
                next_value,
            } => {
                if let Some(value) = next_value.take() {
                    Some(SnapshotInput::Detached(value))
                } else if let Some((key, value)) = entries.next() {
                    *next_value = Some(value);
                    Some(SnapshotInput::Detached(key))
                } else {
                    None
                }
            }
        })
    }
}

enum SnapshotJob<'source, 'output> {
    Copy(SnapshotInput<'source>, &'output mut RuntimeValue),
    Leave(HeapHandle),
}

/// Each output container receives its final slots before child jobs borrow
/// them. The jobs hold disjoint references, so no recursive construction or
/// temporary tree of copied payloads is needed.
fn copy_snapshot_inputs<'a>(
    inputs: impl ExactSizeIterator<Item = SnapshotInput<'a>>,
    heap: Option<&'a Heap>,
    callable_names: &[String],
    nominal_names: &[String],
) -> Result<Vec<RuntimeValue>, VmError> {
    let mut output = vec![RuntimeValue::Unit; inputs.len()];
    let mut jobs = inputs
        .zip(&mut output)
        .map(|(input, output)| SnapshotJob::Copy(input, output))
        .collect::<Vec<_>>();
    let mut visiting = BTreeMap::new();
    while let Some(job) = jobs.pop() {
        let SnapshotJob::Copy(input, output) = job else {
            let SnapshotJob::Leave(handle) = job else {
                unreachable!()
            };
            visiting.remove(&handle);
            continue;
        };
        let handle = match input {
            SnapshotInput::Managed(Value::Heap(handle)) => Some(*handle),
            _ => None,
        };
        if let Some(handle) = handle {
            if let Some(index) = visiting.get(&handle) {
                *output = RuntimeValue::Cycle(*index);
                continue;
            }
            visiting.insert(handle, visiting.len());
            jobs.push(SnapshotJob::Leave(handle));
        }
        let (header, mut children) = snapshot_header(input, heap, callable_names, nominal_names)?;
        *output = header;
        let mut child = |destination| -> Result<(), VmError> {
            let input = children
                .next()?
                .ok_or_else(|| VmError::invariant("snapshot header has excess child slots"))?;
            jobs.push(SnapshotJob::Copy(input, destination));
            Ok(())
        };
        match output {
            RuntimeValue::Tuple(values)
            | RuntimeValue::Array(values)
            | RuntimeValue::Set(values)
            | RuntimeValue::Record { values, .. }
            | RuntimeValue::Variant { values, .. }
            | RuntimeValue::Closure {
                captures: values, ..
            } => {
                for value in values {
                    child(value)?;
                }
            }
            RuntimeValue::Map(entries) => {
                for (key, value) in entries {
                    child(key)?;
                    child(value)?;
                }
            }
            RuntimeValue::Newtype { value, .. }
            | RuntimeValue::OptionSome(value)
            | RuntimeValue::ResultOk(value)
            | RuntimeValue::ResultErr(value)
            | RuntimeValue::Union { value, .. }
            | RuntimeValue::Ref(Some(value)) => child(value)?,
            RuntimeValue::Range { start, end, .. } => {
                child(start)?;
                child(end)?;
            }
            _ => {}
        }
        if children.next()?.is_some() {
            return Err(VmError::invariant("snapshot header is missing child slots"));
        }
    }
    Ok(output)
}

fn snapshot_header<'a>(
    input: SnapshotInput<'a>,
    heap: Option<&'a Heap>,
    callable_names: &[String],
    nominal_names: &[String],
) -> Result<(RuntimeValue, SnapshotChildren<'a>), VmError> {
    let value = match input {
        SnapshotInput::Detached(value) | SnapshotInput::Managed(Value::Host(value)) => {
            return Ok(detached_snapshot_header(value));
        }
        SnapshotInput::Managed(value) => value,
    };
    let empty = SnapshotChildren::Empty;
    let header = match value {
        Value::Unit => RuntimeValue::Unit,
        Value::Bool(value) => RuntimeValue::Bool(*value),
        Value::Integer(value) => RuntimeValue::Integer(*value),
        Value::Float(value) => RuntimeValue::Float(*value),
        Value::Byte(value) => RuntimeValue::Byte(*value),
        Value::Char(value) => RuntimeValue::Char(*value),
        Value::Function {
            callable,
            arguments,
        } => RuntimeValue::Function {
            name: callable_names
                .get(callable.index() as usize)
                .cloned()
                .unwrap_or_else(|| format!("callable#{}", callable.index())),
            type_arguments: arguments.iter().map(|argument| argument.index()).collect(),
        },
        Value::Heap(handle) => {
            let heap =
                heap.ok_or_else(|| VmError::invariant("managed snapshot has no source heap"))?;
            return heap_snapshot_header(
                heap.get(*handle)?,
                heap.descriptor(*handle)?,
                heap,
                nominal_names,
            );
        }
        Value::Loan(_) | Value::Join(_) => {
            return Err(VmError::invariant(
                "a call-local value escaped through the VM boundary",
            ));
        }
        Value::Host(_) => unreachable!("detached host header was selected"),
    };
    Ok((header, empty))
}

fn heap_snapshot_header<'a>(
    object: &'a HeapObject,
    descriptor: BytecodeTypeId,
    heap: &Heap,
    nominal_names: &[String],
) -> Result<(RuntimeValue, SnapshotChildren<'a>), VmError> {
    let slots = |length| vec![RuntimeValue::Unit; length];
    let one = |value| SnapshotChildren::Pair(Some(SnapshotInput::Managed(value)), None);
    Ok(match object {
        HeapObject::String(text) => (RuntimeValue::String(text.clone()), SnapshotChildren::Empty),
        HeapObject::Tuple(values) => (
            RuntimeValue::Tuple(slots(values.len())),
            SnapshotChildren::Managed(values.iter()),
        ),
        HeapObject::Array(values) => (
            RuntimeValue::Array(slots(values.len())),
            SnapshotChildren::Managed(values.iter()),
        ),
        HeapObject::Set(values) => (
            RuntimeValue::Set(slots(values.len())),
            SnapshotChildren::Managed(values.iter()),
        ),
        HeapObject::Map(entries) => (
            RuntimeValue::Map(vec![
                (RuntimeValue::Unit, RuntimeValue::Unit);
                entries.len()
            ]),
            SnapshotChildren::ManagedMap {
                entries: entries.iter(),
                next_value: None,
            },
        ),
        HeapObject::Closure { callable, captures } => (
            RuntimeValue::Closure {
                callable: callable.index(),
                captures: slots(captures.len()),
            },
            SnapshotChildren::Managed(captures.iter()),
        ),
        HeapObject::Newtype { nominal, value } => (
            RuntimeValue::Newtype {
                name: nominal_name(*nominal, nominal_names),
                value: Box::new(RuntimeValue::Unit),
            },
            one(present_value(value)?),
        ),
        HeapObject::Record { nominal, fields } => (
            RuntimeValue::Record {
                name: nominal_name(*nominal, nominal_names),
                values: slots(fields.len()),
            },
            SnapshotChildren::Fields(fields.iter()),
        ),
        HeapObject::Variant { variant, payload } => {
            let BytecodeTraceDescriptor::Variant {
                nominal: Some(nominal),
                variants,
                ..
            } = heap.type_descriptor(descriptor)?
            else {
                return Err(VmError::invariant(
                    "a nominal variant has no nominal trace descriptor",
                ));
            };
            let ordinal = variants
                .iter()
                .position(|candidate| candidate.member == *variant)
                .ok_or_else(|| {
                    VmError::invariant("variant member is absent from its descriptor")
                })?;
            let (count, children) = match payload {
                AggregatePayload::Unit => (0, SnapshotChildren::Empty),
                AggregatePayload::Tuple(values) => {
                    (values.len(), SnapshotChildren::Managed(values.iter()))
                }
                AggregatePayload::Record(fields) => {
                    (fields.len(), SnapshotChildren::Fields(fields.iter()))
                }
            };
            (
                RuntimeValue::Variant {
                    name: nominal_name(*nominal, nominal_names),
                    variant: u32::try_from(ordinal)
                        .map_err(|_| VmError::invariant("variant ordinal exceeds u32"))?,
                    values: slots(count),
                },
                children,
            )
        }
        HeapObject::OptionNone => (RuntimeValue::OptionNone, SnapshotChildren::Empty),
        HeapObject::OptionSome(value) => (
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Unit)),
            one(present_value(value)?),
        ),
        HeapObject::ResultOk(value) => (
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)),
            one(present_value(value)?),
        ),
        HeapObject::ResultErr(value) => (
            RuntimeValue::ResultErr(Box::new(RuntimeValue::Unit)),
            one(present_value(value)?),
        ),
        HeapObject::Union { member, value } => (
            RuntimeValue::Union {
                member: member.index(),
                value: Box::new(RuntimeValue::Unit),
            },
            one(present_value(value)?),
        ),
        HeapObject::Range { kind, start, end } => (
            RuntimeValue::Range {
                inclusive: *kind == BytecodeRangeKind::Inclusive,
                start: Box::new(RuntimeValue::Unit),
                end: Box::new(RuntimeValue::Unit),
            },
            SnapshotChildren::Pair(
                Some(SnapshotInput::Managed(present_value(start)?)),
                Some(SnapshotInput::Managed(present_value(end)?)),
            ),
        ),
        HeapObject::Ref(None) => (RuntimeValue::Ref(None), SnapshotChildren::Empty),
        HeapObject::Ref(Some(value)) => (
            RuntimeValue::Ref(Some(Box::new(RuntimeValue::Unit))),
            one(value),
        ),
        HeapObject::Iterator { .. } => {
            return Err(VmError::invariant(
                "an internal iterator state escaped through the VM boundary",
            ));
        }
    })
}

fn detached_snapshot_header(value: &RuntimeValue) -> (RuntimeValue, SnapshotChildren<'_>) {
    let slots = |length| vec![RuntimeValue::Unit; length];
    let one = |value| SnapshotChildren::Pair(Some(SnapshotInput::Detached(value)), None);
    let empty = SnapshotChildren::Empty;
    match value {
        RuntimeValue::Tuple(values) => (
            RuntimeValue::Tuple(slots(values.len())),
            SnapshotChildren::Detached(values.iter()),
        ),
        RuntimeValue::Array(values) => (
            RuntimeValue::Array(slots(values.len())),
            SnapshotChildren::Detached(values.iter()),
        ),
        RuntimeValue::Set(values) => (
            RuntimeValue::Set(slots(values.len())),
            SnapshotChildren::Detached(values.iter()),
        ),
        RuntimeValue::Record { name, values } => (
            RuntimeValue::Record {
                name: name.clone(),
                values: slots(values.len()),
            },
            SnapshotChildren::Detached(values.iter()),
        ),
        RuntimeValue::Variant {
            name,
            variant,
            values,
        } => (
            RuntimeValue::Variant {
                name: name.clone(),
                variant: *variant,
                values: slots(values.len()),
            },
            SnapshotChildren::Detached(values.iter()),
        ),
        RuntimeValue::Closure { callable, captures } => (
            RuntimeValue::Closure {
                callable: *callable,
                captures: slots(captures.len()),
            },
            SnapshotChildren::Detached(captures.iter()),
        ),
        RuntimeValue::Map(entries) => (
            RuntimeValue::Map(vec![
                (RuntimeValue::Unit, RuntimeValue::Unit);
                entries.len()
            ]),
            SnapshotChildren::DetachedMap {
                entries: entries.iter(),
                next_value: None,
            },
        ),
        RuntimeValue::Newtype { name, value } => (
            RuntimeValue::Newtype {
                name: name.clone(),
                value: Box::new(RuntimeValue::Unit),
            },
            one(value),
        ),
        RuntimeValue::OptionSome(value) => (
            RuntimeValue::OptionSome(Box::new(RuntimeValue::Unit)),
            one(value),
        ),
        RuntimeValue::ResultOk(value) => (
            RuntimeValue::ResultOk(Box::new(RuntimeValue::Unit)),
            one(value),
        ),
        RuntimeValue::ResultErr(value) => (
            RuntimeValue::ResultErr(Box::new(RuntimeValue::Unit)),
            one(value),
        ),
        RuntimeValue::Union { member, value } => (
            RuntimeValue::Union {
                member: *member,
                value: Box::new(RuntimeValue::Unit),
            },
            one(value),
        ),
        RuntimeValue::Ref(Some(value)) => (
            RuntimeValue::Ref(Some(Box::new(RuntimeValue::Unit))),
            one(value),
        ),
        RuntimeValue::Range {
            inclusive,
            start,
            end,
        } => (
            RuntimeValue::Range {
                inclusive: *inclusive,
                start: Box::new(RuntimeValue::Unit),
                end: Box::new(RuntimeValue::Unit),
            },
            SnapshotChildren::Pair(
                Some(SnapshotInput::Detached(start)),
                Some(SnapshotInput::Detached(end)),
            ),
        ),
        // These variants have no recursive payload; strings, function names
        // and type arguments were included in the complete batch admission.
        _ => (value.clone(), empty),
    }
}

/// A borrowed path avoids allocating a visited map during preflight. The
/// depth cap also bounds the subsequent recursive construction and cleanup.
struct SnapshotPath<'a> {
    handle: HeapHandle,
    parent: Option<&'a SnapshotPath<'a>>,
}

impl SnapshotPath<'_> {
    fn contains(mut path: Option<&Self>, handle: HeapHandle) -> bool {
        while let Some(current) = path {
            if current.handle == handle {
                return true;
            }
            path = current.parent;
        }
        false
    }
}

struct SnapshotAdmission<'a> {
    heap: Option<&'a Heap>,
    callable_names: &'a [String],
    nominal_names: &'a [String],
    memory: VmMemoryCharge,
    nodes: u64,
    depth: usize,
}

impl SnapshotAdmission<'_> {
    fn memory_error(&self) -> VmError {
        VmError::ResourceLimit {
            resource: "memory",
            limit: self.memory.budget().limit(),
        }
    }

    fn bytes(&mut self, bytes: u64) -> Result<(), VmError> {
        let total = self
            .memory
            .bytes()
            .checked_add(bytes)
            .ok_or_else(|| self.memory_error())?;
        self.memory.resize(total)
    }

    fn node(&mut self, depth: usize) -> Result<VmMemoryCharge, VmError> {
        if depth > super::TEST_SNAPSHOT_MAX_DEPTH {
            return Err(VmError::ResourceLimit {
                resource: "stack depth",
                limit: super::TEST_SNAPSHOT_MAX_DEPTH as u64,
            });
        }
        let frame = self
            .memory
            .budget()
            .reserve(super::TEST_SNAPSHOT_FRAME_BYTES)?;
        self.bytes(super::TEST_DETACHED_VALUE_BYTES)?;
        self.nodes += 1;
        self.depth = self.depth.max(depth);
        Ok(frame)
    }

    fn value(
        &mut self,
        value: &Value,
        path: Option<&SnapshotPath<'_>>,
        depth: usize,
    ) -> Result<(), VmError> {
        if let Value::Host(value) = value {
            return self.detached(value, depth);
        }
        let _frame = self.node(depth)?;
        match value {
            Value::Function {
                callable,
                arguments,
            } => {
                self.bytes(snapshot_name_bytes(
                    self.callable_names,
                    callable.index(),
                    "callable#",
                ))?;
                self.bytes(arguments.len() as u64 * 4)
            }
            Value::Heap(handle) if !SnapshotPath::contains(path, *handle) => {
                let heap = self
                    .heap
                    .ok_or_else(|| VmError::invariant("managed snapshot has no source heap"))?;
                let path = SnapshotPath {
                    handle: *handle,
                    parent: path,
                };
                self.object(heap.get(*handle)?, heap.descriptor(*handle)?, &path, depth)
            }
            Value::Loan(_) => Err(VmError::invariant(
                "a call-local loan escaped through the VM boundary",
            )),
            Value::Join(_) => Err(VmError::invariant(
                "an affine Join escaped the structured runtime",
            )),
            Value::Unit
            | Value::Bool(_)
            | Value::Integer(_)
            | Value::Float(_)
            | Value::Byte(_)
            | Value::Char(_)
            | Value::Heap(_) => Ok(()),
            Value::Host(_) => unreachable!("host values use detached admission"),
        }
    }

    fn object(
        &mut self,
        object: &HeapObject,
        descriptor: BytecodeTypeId,
        path: &SnapshotPath<'_>,
        depth: usize,
    ) -> Result<(), VmError> {
        match object {
            HeapObject::String(text) => self.bytes(text.len() as u64),
            HeapObject::Tuple(values) => self.optional_values(values, path, depth),
            HeapObject::Array(values) | HeapObject::Set(values) => {
                self.optional_values(values, path, depth)
            }
            HeapObject::Map(entries) => {
                for (key, value) in entries {
                    self.value(present_value(key)?, Some(path), depth + 1)?;
                    self.value(present_value(value)?, Some(path), depth + 1)?;
                }
                Ok(())
            }
            HeapObject::Closure { captures, .. } => self.optional_values(captures, path, depth),
            HeapObject::Newtype { nominal, value } => {
                self.bytes(snapshot_name_bytes(
                    self.nominal_names,
                    nominal.index(),
                    "nominal#",
                ))?;
                self.value(present_value(value)?, Some(path), depth + 1)
            }
            HeapObject::Record { nominal, fields } => {
                self.bytes(snapshot_name_bytes(
                    self.nominal_names,
                    nominal.index(),
                    "nominal#",
                ))?;
                for (_, value) in fields {
                    self.value(present_value(value)?, Some(path), depth + 1)?;
                }
                Ok(())
            }
            HeapObject::Variant { variant, payload } => {
                let BytecodeTraceDescriptor::Variant {
                    nominal: Some(nominal),
                    variants,
                    ..
                } = self
                    .heap
                    .ok_or_else(|| VmError::invariant("managed snapshot has no source heap"))?
                    .type_descriptor(descriptor)?
                else {
                    return Err(VmError::invariant(
                        "a nominal variant has no nominal trace descriptor",
                    ));
                };
                if !variants
                    .iter()
                    .any(|candidate| candidate.member == *variant)
                {
                    return Err(VmError::invariant(
                        "variant member is absent from its descriptor",
                    ));
                }
                self.bytes(snapshot_name_bytes(
                    self.nominal_names,
                    nominal.index(),
                    "nominal#",
                ))?;
                match payload {
                    AggregatePayload::Unit => Ok(()),
                    AggregatePayload::Tuple(values) => self.optional_values(values, path, depth),
                    AggregatePayload::Record(fields) => {
                        for (_, value) in fields {
                            self.value(present_value(value)?, Some(path), depth + 1)?;
                        }
                        Ok(())
                    }
                }
            }
            HeapObject::OptionSome(value)
            | HeapObject::ResultOk(value)
            | HeapObject::ResultErr(value)
            | HeapObject::Union { value, .. } => {
                self.value(present_value(value)?, Some(path), depth + 1)
            }
            HeapObject::OptionNone | HeapObject::Ref(None) => Ok(()),
            HeapObject::Ref(Some(value)) => self.value(value, Some(path), depth + 1),
            HeapObject::Range { start, end, .. } => {
                self.value(present_value(start)?, Some(path), depth + 1)?;
                self.value(present_value(end)?, Some(path), depth + 1)
            }
            HeapObject::Iterator { .. } => Err(VmError::invariant(
                "an internal iterator state escaped through the VM boundary",
            )),
        }
    }

    fn optional_values(
        &mut self,
        values: &[Option<Value>],
        path: &SnapshotPath<'_>,
        depth: usize,
    ) -> Result<(), VmError> {
        for value in values {
            self.value(present_value(value)?, Some(path), depth + 1)?;
        }
        Ok(())
    }

    fn detached(&mut self, value: &RuntimeValue, depth: usize) -> Result<(), VmError> {
        let _frame = self.node(depth)?;
        match value {
            RuntimeValue::String(text) => self.bytes(text.len() as u64),
            RuntimeValue::Function {
                name,
                type_arguments,
            } => {
                self.bytes(name.len() as u64)?;
                self.bytes(type_arguments.len() as u64 * 4)
            }
            RuntimeValue::Record { name, values } | RuntimeValue::Variant { name, values, .. } => {
                self.bytes(name.len() as u64)?;
                self.detached_values(values, depth)
            }
            RuntimeValue::Newtype { name, value } => {
                self.bytes(name.len() as u64)?;
                self.detached(value, depth + 1)
            }
            RuntimeValue::Tuple(values)
            | RuntimeValue::Array(values)
            | RuntimeValue::Set(values)
            | RuntimeValue::Closure {
                captures: values, ..
            } => self.detached_values(values, depth),
            RuntimeValue::Map(entries) => {
                for (key, value) in entries {
                    self.detached(key, depth + 1)?;
                    self.detached(value, depth + 1)?;
                }
                Ok(())
            }
            RuntimeValue::OptionSome(value)
            | RuntimeValue::ResultOk(value)
            | RuntimeValue::ResultErr(value)
            | RuntimeValue::Union { value, .. }
            | RuntimeValue::Ref(Some(value)) => self.detached(value, depth + 1),
            RuntimeValue::Range { start, end, .. } => {
                self.detached(start, depth + 1)?;
                self.detached(end, depth + 1)
            }
            RuntimeValue::Unit
            | RuntimeValue::Bool(_)
            | RuntimeValue::Integer(_)
            | RuntimeValue::Float(_)
            | RuntimeValue::Byte(_)
            | RuntimeValue::Char(_)
            | RuntimeValue::OptionNone
            | RuntimeValue::Ref(None)
            | RuntimeValue::Host { .. }
            | RuntimeValue::Cycle(_) => Ok(()),
        }
    }

    fn detached_values(&mut self, values: &[RuntimeValue], depth: usize) -> Result<(), VmError> {
        for value in values {
            self.detached(value, depth + 1)?;
        }
        Ok(())
    }
}

fn snapshot_name_bytes(names: &[String], index: u32, prefix: &str) -> u64 {
    names.get(index as usize).map_or_else(
        || prefix.len() as u64 + u64::from(index.checked_ilog10().map_or(1, |digits| digits + 1)),
        |name| name.len() as u64,
    )
}

pub(super) fn snapshot_value(
    value: &Value,
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
) -> Result<RuntimeValue, VmError> {
    let mut visiting = BTreeMap::new();
    snapshot_value_inner(value, heap, callable_names, nominal_names, &mut visiting)
}

fn snapshot_value_inner(
    value: &Value,
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
    visiting: &mut BTreeMap<HeapHandle, usize>,
) -> Result<RuntimeValue, VmError> {
    Ok(match value {
        Value::Unit => RuntimeValue::Unit,
        Value::Bool(value) => RuntimeValue::Bool(*value),
        Value::Integer(value) => RuntimeValue::Integer(*value),
        Value::Float(value) => RuntimeValue::Float(*value),
        Value::Byte(value) => RuntimeValue::Byte(*value),
        Value::Char(value) => RuntimeValue::Char(*value),
        Value::Function {
            callable,
            arguments,
        } => RuntimeValue::Function {
            name: callable_names
                .get(callable.index() as usize)
                .cloned()
                .unwrap_or_else(|| format!("callable#{}", callable.index())),
            type_arguments: arguments.iter().map(|argument| argument.index()).collect(),
        },
        Value::Loan(_) => {
            return Err(VmError::invariant(
                "a call-local loan escaped through the VM boundary",
            ));
        }
        Value::Join(_) => {
            return Err(VmError::invariant(
                "an affine Join escaped the structured runtime",
            ));
        }
        Value::Host(value) => value.clone(),
        Value::Heap(handle) => {
            if let Some(id) = visiting.get(handle) {
                return Ok(RuntimeValue::Cycle(*id));
            }
            let id = visiting.len();
            visiting.insert(*handle, id);
            let result = snapshot_object(
                heap.get(*handle)?,
                heap.descriptor(*handle)?,
                heap,
                callable_names,
                nominal_names,
                visiting,
            )?;
            visiting.remove(handle);
            result
        }
    })
}

fn snapshot_object(
    object: &HeapObject,
    descriptor: BytecodeTypeId,
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
    visiting: &mut BTreeMap<HeapHandle, usize>,
) -> Result<RuntimeValue, VmError> {
    let snapshot = |value: &Value, visiting: &mut BTreeMap<HeapHandle, usize>| {
        snapshot_value_inner(value, heap, callable_names, nominal_names, visiting)
    };
    Ok(match object {
        HeapObject::String(value) => RuntimeValue::String(value.clone()),
        HeapObject::Tuple(values) => RuntimeValue::Tuple(
            values
                .iter()
                .map(|value| snapshot(present_value(value)?, visiting))
                .collect::<Result<_, _>>()?,
        ),
        HeapObject::Array(values) => RuntimeValue::Array(
            values
                .iter()
                .map(|value| snapshot(present_value(value)?, visiting))
                .collect::<Result<_, _>>()?,
        ),
        HeapObject::Map(entries) => RuntimeValue::Map(
            entries
                .iter()
                .map(|(key, value)| {
                    Ok((
                        snapshot(present_value(key)?, visiting)?,
                        snapshot(present_value(value)?, visiting)?,
                    ))
                })
                .collect::<Result<_, VmError>>()?,
        ),
        HeapObject::Set(values) => RuntimeValue::Set(
            values
                .iter()
                .map(|value| snapshot(present_value(value)?, visiting))
                .collect::<Result<_, _>>()?,
        ),
        HeapObject::Closure { callable, captures } => RuntimeValue::Closure {
            callable: callable.index(),
            captures: captures
                .iter()
                .map(|value| snapshot(present_value(value)?, visiting))
                .collect::<Result<_, _>>()?,
        },
        HeapObject::Newtype { nominal, value } => RuntimeValue::Newtype {
            name: nominal_name(*nominal, nominal_names),
            value: Box::new(snapshot(present_value(value)?, visiting)?),
        },
        HeapObject::Record { nominal, fields } => RuntimeValue::Record {
            name: nominal_name(*nominal, nominal_names),
            values: fields
                .iter()
                .map(|(_, value)| snapshot(present_value(value)?, visiting))
                .collect::<Result<_, VmError>>()?,
        },
        HeapObject::Variant { variant, payload } => {
            let BytecodeTraceDescriptor::Variant {
                nominal: Some(nominal),
                variants,
                ..
            } = heap.type_descriptor(descriptor)?
            else {
                return Err(VmError::invariant(
                    "a nominal variant has no nominal trace descriptor",
                ));
            };
            let ordinal = variants
                .iter()
                .position(|candidate| candidate.member == *variant)
                .ok_or_else(|| {
                    VmError::invariant("variant member is absent from its descriptor")
                })?;
            RuntimeValue::Variant {
                name: nominal_name(*nominal, nominal_names),
                variant: u32::try_from(ordinal)
                    .map_err(|_| VmError::invariant("variant ordinal exceeds u32"))?,
                values: snapshot_payload_values(
                    payload,
                    heap,
                    callable_names,
                    nominal_names,
                    visiting,
                )?,
            }
        }
        HeapObject::OptionNone => RuntimeValue::OptionNone,
        HeapObject::OptionSome(value) => {
            RuntimeValue::OptionSome(Box::new(snapshot(present_value(value)?, visiting)?))
        }
        HeapObject::ResultOk(value) => {
            RuntimeValue::ResultOk(Box::new(snapshot(present_value(value)?, visiting)?))
        }
        HeapObject::ResultErr(value) => {
            RuntimeValue::ResultErr(Box::new(snapshot(present_value(value)?, visiting)?))
        }
        HeapObject::Union { member, value } => RuntimeValue::Union {
            member: member.index(),
            value: Box::new(snapshot(present_value(value)?, visiting)?),
        },
        HeapObject::Range { kind, start, end } => RuntimeValue::Range {
            inclusive: *kind == BytecodeRangeKind::Inclusive,
            start: Box::new(snapshot(present_value(start)?, visiting)?),
            end: Box::new(snapshot(present_value(end)?, visiting)?),
        },
        HeapObject::Iterator { .. } => {
            return Err(VmError::invariant(
                "an internal iterator state escaped through the VM boundary",
            ));
        }
        HeapObject::Ref(value) => RuntimeValue::Ref(
            value
                .as_ref()
                .map(|value| snapshot(value, visiting).map(Box::new))
                .transpose()?,
        ),
    })
}

fn present_value(value: &Option<Value>) -> Result<&Value, VmError> {
    value
        .as_ref()
        .ok_or_else(|| VmError::invariant("a moved value escaped through the VM boundary"))
}

fn snapshot_payload(
    payload: &AggregatePayload,
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
    visiting: &mut BTreeMap<HeapHandle, usize>,
) -> Result<Vec<(Option<u32>, RuntimeValue)>, VmError> {
    let snapshot = |value: &Value, visiting: &mut BTreeMap<HeapHandle, usize>| {
        snapshot_value_inner(value, heap, callable_names, nominal_names, visiting)
    };
    match payload {
        AggregatePayload::Unit => Ok(Vec::new()),
        AggregatePayload::Tuple(values) => values
            .iter()
            .map(|value| {
                let value = value
                    .as_ref()
                    .ok_or_else(|| VmError::invariant("a moved variant payload escaped the VM"))?;
                Ok((None, snapshot(value, visiting)?))
            })
            .collect(),
        AggregatePayload::Record(fields) => fields
            .iter()
            .map(|(field, value)| {
                let value = value
                    .as_ref()
                    .ok_or_else(|| VmError::invariant("a moved variant field escaped the VM"))?;
                Ok((Some(*field), snapshot(value, visiting)?))
            })
            .collect(),
    }
}

fn snapshot_payload_values(
    payload: &AggregatePayload,
    heap: &Heap,
    callable_names: &[String],
    nominal_names: &[String],
    visiting: &mut BTreeMap<HeapHandle, usize>,
) -> Result<Vec<RuntimeValue>, VmError> {
    Ok(
        snapshot_payload(payload, heap, callable_names, nominal_names, visiting)?
            .into_iter()
            .map(|(_, value)| value)
            .collect(),
    )
}

fn nominal_name(id: BytecodeNominalId, names: &[String]) -> String {
    names
        .get(id.index() as usize)
        .cloned()
        .unwrap_or_else(|| format!("nominal#{}", id.index()))
}

#[cfg(test)]
mod tests {
    use crate::bytecode::{
        BytecodeCallableId, BytecodeCursorMode, BytecodeField, BytecodeNominalId,
        BytecodeParameterMode, BytecodePlace, BytecodeRangeKind, BytecodeSlotId,
        BytecodeTraceDescriptor, BytecodeTypeId, BytecodeVariant, BytecodeVariantPayload,
    };
    use crate::runtime::heap::{Heap, HeapObject};
    use crate::runtime::{
        RuntimeHostValueKind, RuntimeValue, VmLimits, VmMemoryBudget, VmStatistics,
    };

    use super::{
        AggregatePayload, RuntimeJoin, RuntimeLoan, Value, snapshot_arguments, snapshot_value,
    };

    fn assert_admitted_snapshot(
        value: &Value,
        heap: &Heap,
        callables: &[String],
        nominals: &[String],
        expected: &RuntimeValue,
    ) {
        let budget = VmMemoryBudget::new(1_048_576);
        for source in [value.clone(), Value::Host(expected.clone())] {
            let arguments =
                snapshot_arguments(&[source], heap, callables, nominals, Some(&budget)).unwrap();
            assert_eq!(arguments.values.as_slice(), std::slice::from_ref(expected));
            assert_eq!(Some(budget.live_bytes()), expected.retained_bytes());
            assert_eq!(
                arguments.memory.as_ref().unwrap().bytes(),
                budget.live_bytes()
            );
            drop(arguments);
            assert_eq!(budget.live_bytes(), 0);
        }
        let untracked =
            snapshot_arguments(std::slice::from_ref(value), heap, callables, nominals, None)
                .unwrap();
        assert_eq!(untracked.values.as_slice(), std::slice::from_ref(expected));
        assert!(untracked.memory.is_none());
        for account in [None, Some(&budget)] {
            let arguments =
                super::snapshot_detached_arguments(std::slice::from_ref(expected), account)
                    .unwrap();
            assert_eq!(arguments.values.as_slice(), std::slice::from_ref(expected));
            assert_eq!(
                budget.live_bytes(),
                if account.is_some() {
                    expected.retained_bytes().unwrap()
                } else {
                    0
                }
            );
            drop(arguments);
            assert_eq!(budget.live_bytes(), 0);
        }
    }

    fn snapshot_heap() -> Heap {
        Heap::new(
            VmLimits {
                max_heap_objects: 64,
                max_heap_bytes: 1024 * 1024,
                initial_gc_threshold: 64,
                ..VmLimits::default()
            },
            vec![
                BytecodeTraceDescriptor::String,
                BytecodeTraceDescriptor::Tuple {
                    fields: vec![BytecodeTypeId::new(0)],
                },
                BytecodeTraceDescriptor::Array {
                    element: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Map {
                    key: BytecodeTypeId::new(0),
                    value: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Set {
                    element: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Closure {
                    callable: BytecodeCallableId::new(7),
                    captures: vec![BytecodeTypeId::new(0)],
                },
                BytecodeTraceDescriptor::Newtype {
                    nominal: BytecodeNominalId::new(0),
                    arguments: Vec::new(),
                    value: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Record {
                    nominal: BytecodeNominalId::new(1),
                    arguments: Vec::new(),
                    fields: vec![BytecodeField {
                        member: 4,
                        ty: BytecodeTypeId::new(0),
                    }],
                },
                BytecodeTraceDescriptor::Variant {
                    nominal: Some(BytecodeNominalId::new(2)),
                    arguments: Vec::new(),
                    variants: vec![
                        BytecodeVariant {
                            member: 0,
                            payload: BytecodeVariantPayload::Unit,
                        },
                        BytecodeVariant {
                            member: 1,
                            payload: BytecodeVariantPayload::Tuple(vec![BytecodeTypeId::new(0)]),
                        },
                        BytecodeVariant {
                            member: 2,
                            payload: BytecodeVariantPayload::Record(vec![BytecodeField {
                                member: 7,
                                ty: BytecodeTypeId::new(0),
                            }]),
                        },
                    ],
                },
                BytecodeTraceDescriptor::Option {
                    value: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Result {
                    success: BytecodeTypeId::new(0),
                    error: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Union {
                    members: vec![BytecodeTypeId::new(0)],
                },
                BytecodeTraceDescriptor::Range {
                    element: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Ref {
                    value: BytecodeTypeId::new(0),
                },
                BytecodeTraceDescriptor::Cursor {
                    mode: BytecodeCursorMode::Own,
                    collection: BytecodeTypeId::new(2),
                },
            ],
        )
    }

    fn allocate(heap: &mut Heap, descriptor: u32, object: HeapObject) -> Value {
        let handle = heap
            .allocate(
                BytecodeTypeId::new(descriptor),
                object,
                &[],
                &mut VmStatistics::default(),
            )
            .unwrap();
        Value::Heap(handle)
    }

    #[test]
    fn scalar_snapshots_and_internal_values_have_closed_boundaries() {
        let heap = snapshot_heap();
        let host = RuntimeValue::Host {
            kind: RuntimeHostValueKind::Bytes,
            id: 17,
        };
        let cases = [
            (Value::Unit, RuntimeValue::Unit),
            (Value::Bool(true), RuntimeValue::Bool(true)),
            (Value::Integer(-7), RuntimeValue::Integer(-7)),
            (Value::Float(3.5), RuntimeValue::Float(3.5)),
            (Value::Byte(255), RuntimeValue::Byte(255)),
            (Value::Char('λ'), RuntimeValue::Char('λ')),
            (
                Value::Function {
                    callable: BytecodeCallableId::new(1),
                    arguments: vec![BytecodeTypeId::new(2), BytecodeTypeId::new(3)],
                },
                RuntimeValue::Function {
                    name: "chosen".into(),
                    type_arguments: vec![2, 3],
                },
            ),
            (Value::Host(host.clone()), host),
        ];
        for (value, expected) in cases {
            assert_eq!(
                snapshot_value(&value, &heap, &["unused".into(), "chosen".into()], &[]).unwrap(),
                expected
            );
            assert_eq!(value.heap_handle(), None);
            assert_admitted_snapshot(
                &value,
                &heap,
                &["unused".into(), "chosen".into()],
                &[],
                &expected,
            );
        }
        assert_eq!(
            snapshot_value(
                &Value::Function {
                    callable: BytecodeCallableId::new(9),
                    arguments: Vec::new(),
                },
                &heap,
                &[],
                &[],
            )
            .unwrap(),
            RuntimeValue::Function {
                name: "callable#9".into(),
                type_arguments: Vec::new(),
            }
        );

        let loan = Value::Loan(RuntimeLoan {
            task: 0,
            frame: 0,
            place: BytecodePlace {
                slot: BytecodeSlotId::new(0),
                ty: BytecodeTypeId::new(0),
                projections: Vec::new(),
                source_loan: None,
            },
            mode: BytecodeParameterMode::Ref,
        });
        assert!(snapshot_value(&loan, &heap, &[], &[]).is_err());
        assert!(
            snapshot_value(
                &Value::Join(RuntimeJoin { task: 0, scope: 0 }),
                &heap,
                &[],
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn every_managed_value_shape_has_a_deterministic_snapshot() {
        let mut heap = snapshot_heap();
        let cases = [
            (
                allocate(&mut heap, 0, HeapObject::String("text".into())),
                RuntimeValue::String("text".into()),
            ),
            (
                allocate(
                    &mut heap,
                    1,
                    HeapObject::Tuple(vec![Some(Value::Integer(1))]),
                ),
                RuntimeValue::Tuple(vec![RuntimeValue::Integer(1)]),
            ),
            (
                allocate(
                    &mut heap,
                    2,
                    HeapObject::Array(vec![Some(Value::Integer(2))].into()),
                ),
                RuntimeValue::Array(vec![RuntimeValue::Integer(2)]),
            ),
            (
                allocate(
                    &mut heap,
                    3,
                    HeapObject::Map(
                        vec![(Some(Value::Integer(3)), Some(Value::Bool(true)))].into(),
                    ),
                ),
                RuntimeValue::Map(vec![(RuntimeValue::Integer(3), RuntimeValue::Bool(true))]),
            ),
            (
                allocate(
                    &mut heap,
                    4,
                    HeapObject::Set(vec![Some(Value::Integer(4))].into()),
                ),
                RuntimeValue::Set(vec![RuntimeValue::Integer(4)]),
            ),
            (
                allocate(
                    &mut heap,
                    5,
                    HeapObject::Closure {
                        callable: BytecodeCallableId::new(7),
                        captures: vec![Some(Value::Integer(5))],
                    },
                ),
                RuntimeValue::Closure {
                    callable: 7,
                    captures: vec![RuntimeValue::Integer(5)],
                },
            ),
            (
                allocate(
                    &mut heap,
                    6,
                    HeapObject::Newtype {
                        nominal: BytecodeNominalId::new(0),
                        value: Some(Value::Integer(6)),
                    },
                ),
                RuntimeValue::Newtype {
                    name: "Meters".into(),
                    value: Box::new(RuntimeValue::Integer(6)),
                },
            ),
            (
                allocate(
                    &mut heap,
                    7,
                    HeapObject::Record {
                        nominal: BytecodeNominalId::new(1),
                        fields: vec![(4, Some(Value::Integer(7)))],
                    },
                ),
                RuntimeValue::Record {
                    name: "nominal#1".into(),
                    values: vec![RuntimeValue::Integer(7)],
                },
            ),
            (
                allocate(
                    &mut heap,
                    8,
                    HeapObject::Variant {
                        variant: 0,
                        payload: AggregatePayload::Unit,
                    },
                ),
                RuntimeValue::Variant {
                    name: "nominal#2".into(),
                    variant: 0,
                    values: Vec::new(),
                },
            ),
            (
                allocate(
                    &mut heap,
                    8,
                    HeapObject::Variant {
                        variant: 1,
                        payload: AggregatePayload::Tuple(vec![Some(Value::Integer(8))]),
                    },
                ),
                RuntimeValue::Variant {
                    name: "nominal#2".into(),
                    variant: 1,
                    values: vec![RuntimeValue::Integer(8)],
                },
            ),
            (
                allocate(
                    &mut heap,
                    8,
                    HeapObject::Variant {
                        variant: 2,
                        payload: AggregatePayload::Record(vec![(7, Some(Value::Integer(9)))]),
                    },
                ),
                RuntimeValue::Variant {
                    name: "nominal#2".into(),
                    variant: 2,
                    values: vec![RuntimeValue::Integer(9)],
                },
            ),
            (
                allocate(&mut heap, 9, HeapObject::OptionNone),
                RuntimeValue::OptionNone,
            ),
            (
                allocate(
                    &mut heap,
                    9,
                    HeapObject::OptionSome(Some(Value::Integer(10))),
                ),
                RuntimeValue::OptionSome(Box::new(RuntimeValue::Integer(10))),
            ),
            (
                allocate(
                    &mut heap,
                    10,
                    HeapObject::ResultOk(Some(Value::Integer(11))),
                ),
                RuntimeValue::ResultOk(Box::new(RuntimeValue::Integer(11))),
            ),
            (
                allocate(
                    &mut heap,
                    10,
                    HeapObject::ResultErr(Some(Value::Integer(12))),
                ),
                RuntimeValue::ResultErr(Box::new(RuntimeValue::Integer(12))),
            ),
            (
                allocate(
                    &mut heap,
                    11,
                    HeapObject::Union {
                        member: BytecodeTypeId::new(0),
                        value: Some(Value::Integer(13)),
                    },
                ),
                RuntimeValue::Union {
                    member: 0,
                    value: Box::new(RuntimeValue::Integer(13)),
                },
            ),
            (
                allocate(
                    &mut heap,
                    12,
                    HeapObject::Range {
                        kind: BytecodeRangeKind::Inclusive,
                        start: Some(Value::Integer(1)),
                        end: Some(Value::Integer(3)),
                    },
                ),
                RuntimeValue::Range {
                    inclusive: true,
                    start: Box::new(RuntimeValue::Integer(1)),
                    end: Box::new(RuntimeValue::Integer(3)),
                },
            ),
            (
                allocate(&mut heap, 13, HeapObject::Ref(Some(Value::Integer(14)))),
                RuntimeValue::Ref(Some(Box::new(RuntimeValue::Integer(14)))),
            ),
        ];

        for (value, expected) in cases {
            assert!(value.heap_handle().is_some());
            assert_eq!(
                snapshot_value(&value, &heap, &[], &["Meters".into()]).unwrap(),
                expected
            );
            assert_admitted_snapshot(&value, &heap, &[], &["Meters".into()], &expected);
        }
    }

    #[test]
    fn detached_snapshot_budget_sweep_never_publishes_a_partial_graph() {
        let text = || RuntimeValue::String("é🦀".into());
        let input = RuntimeValue::Tuple(vec![
            RuntimeValue::Function {
                name: "callback".into(),
                type_arguments: vec![1, 2],
            },
            RuntimeValue::Map(vec![(
                text(),
                RuntimeValue::Record {
                    name: "Row".into(),
                    values: vec![text()],
                },
            )]),
            RuntimeValue::Newtype {
                name: "Label".into(),
                value: Box::new(text()),
            },
            RuntimeValue::Variant {
                name: "Choice".into(),
                variant: 3,
                values: vec![text()],
            },
            RuntimeValue::Array(vec![text(), text()]),
            RuntimeValue::Set(vec![text()]),
            RuntimeValue::Closure {
                callable: 7,
                captures: vec![text()],
            },
            RuntimeValue::OptionSome(Box::new(text())),
            RuntimeValue::ResultOk(Box::new(text())),
            RuntimeValue::ResultErr(Box::new(text())),
            RuntimeValue::Union {
                member: 1,
                value: Box::new(text()),
            },
            RuntimeValue::Range {
                inclusive: true,
                start: Box::new(RuntimeValue::Integer(1)),
                end: Box::new(RuntimeValue::Integer(9)),
            },
            RuntimeValue::Ref(Some(Box::new(text()))),
        ]);
        let expected = input.clone();
        let retained = input.retained_bytes().unwrap();
        let mut accepted = None;
        for limit in 0..4096 {
            let budget = VmMemoryBudget::new(limit);
            match super::snapshot_detached_arguments(std::slice::from_ref(&input), Some(&budget)) {
                Ok(snapshot) => {
                    assert_eq!(snapshot.values.as_slice(), std::slice::from_ref(&expected));
                    assert_eq!(budget.live_bytes(), retained);
                    drop(snapshot);
                    accepted = Some(limit);
                }
                Err(super::VmError::ResourceLimit {
                    resource: "memory",
                    limit: actual,
                }) => assert_eq!(actual, limit),
                other => panic!("unexpected snapshot outcome: {other:?}"),
            }
            assert_eq!(budget.live_bytes(), 0, "budget {limit}");
            assert_eq!(input, expected);
            if accepted.is_some() {
                break;
            }
        }
        assert!(accepted.is_some_and(|limit| limit > retained));
    }

    #[test]
    fn snapshot_admission_rejects_missing_context_and_storage_overflow() {
        let mut heap = snapshot_heap();
        let value = allocate(&mut heap, 0, HeapObject::String("value".into()));
        let budget = VmMemoryBudget::new(u64::MAX);
        let mut admission = super::SnapshotAdmission {
            heap: None,
            callable_names: &[],
            nominal_names: &[],
            memory: budget.reserve(0).unwrap(),
            nodes: 0,
            depth: 0,
        };
        assert!(matches!(
            admission.value(&value, None, 1),
            Err(super::VmError::Invariant(message)) if message.contains("no source heap")
        ));
        assert!(
            super::snapshot_header(super::SnapshotInput::Managed(&value), None, &[], &[]).is_err()
        );
        admission.memory.resize(0).unwrap();
        admission.bytes(u64::MAX).unwrap();
        assert!(matches!(
            admission.bytes(1),
            Err(super::VmError::ResourceLimit {
                resource: "memory",
                limit: u64::MAX
            })
        ));
        assert_eq!(budget.live_bytes(), u64::MAX);
        drop(admission);
        assert_eq!(budget.live_bytes(), 0);
        assert_admitted_snapshot(
            &Value::Function {
                callable: BytecodeCallableId::new(91),
                arguments: vec![BytecodeTypeId::new(3)],
            },
            &heap,
            &[],
            &[],
            &RuntimeValue::Function {
                name: "callable#91".into(),
                type_arguments: vec![3],
            },
        );
    }

    #[test]
    fn snapshot_variant_routes_reject_inconsistent_metadata_before_copying() {
        let mut heap = snapshot_heap();
        let root = allocate(&mut heap, 0, HeapObject::String("root".into()));
        let path = super::SnapshotPath {
            handle: root.heap_handle().unwrap(),
            parent: None,
        };
        let object = HeapObject::Variant {
            variant: 99,
            payload: AggregatePayload::Unit,
        };
        let budget = VmMemoryBudget::new(1024);
        for descriptor in [BytecodeTypeId::new(0), BytecodeTypeId::new(8)] {
            assert!(super::heap_snapshot_header(&object, descriptor, &heap, &[]).is_err());
            assert!(
                super::snapshot_object(
                    &object,
                    descriptor,
                    &heap,
                    &[],
                    &[],
                    &mut Default::default()
                )
                .is_err()
            );
            let mut admission = super::SnapshotAdmission {
                heap: Some(&heap),
                callable_names: &[],
                nominal_names: &[],
                memory: budget.reserve(0).unwrap(),
                nodes: 0,
                depth: 0,
            };
            assert!(admission.object(&object, descriptor, &path, 1).is_err());
            assert_eq!(budget.live_bytes(), 0);
        }
        let mut missing = super::SnapshotAdmission {
            heap: None,
            callable_names: &[],
            nominal_names: &[],
            memory: budget.reserve(0).unwrap(),
            nodes: 0,
            depth: 0,
        };
        assert!(
            missing
                .object(&object, BytecodeTypeId::new(8), &path, 1)
                .is_err()
        );
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn snapshot_batches_admit_exact_payload_and_workspace_before_copying_aliases() {
        let mut heap = snapshot_heap();
        let source = allocate(&mut heap, 0, HeapObject::String("é🦀".into()));
        for copies in [0, 1, 2] {
            let values = vec![source.clone(); copies];
            let retained = copies as u64 * 38;
            let peak = retained + copies as u64 * 32 + if copies == 0 { 0 } else { 64 };
            for limit in [peak.saturating_sub(1), peak] {
                let budget = VmMemoryBudget::new(limit);
                let result = snapshot_arguments(&values, &heap, &[], &[], Some(&budget));
                if limit < peak {
                    assert!(
                        matches!(result, Err(super::VmError::ResourceLimit { resource: "memory", limit: actual }) if actual == limit)
                    );
                } else {
                    let snapshots = result.unwrap();
                    assert_eq!(
                        snapshots.values,
                        vec![RuntimeValue::String("é🦀".into()); copies]
                    );
                    assert_eq!(budget.live_bytes(), retained);
                    let HeapObject::String(original) =
                        heap.get(source.heap_handle().unwrap()).unwrap()
                    else {
                        unreachable!()
                    };
                    for snapshot in &snapshots.values {
                        let RuntimeValue::String(text) = snapshot else {
                            unreachable!()
                        };
                        assert_ne!(text.as_ptr(), original.as_ptr());
                    }
                    drop(snapshots);
                }
                assert_eq!(budget.live_bytes(), 0);
                let originals = vec![RuntimeValue::String("é🦀".into()); copies];
                let result = super::snapshot_detached_arguments(&originals, Some(&budget));
                if limit < peak {
                    assert!(matches!(
                        result,
                        Err(super::VmError::ResourceLimit {
                            resource: "memory",
                            ..
                        })
                    ));
                } else {
                    let snapshots = result.unwrap();
                    assert_eq!(snapshots.values, originals);
                    assert_eq!(budget.live_bytes(), retained);
                    drop(snapshots);
                }
                assert_eq!(budget.live_bytes(), 0);
            }
        }
        let budget = VmMemoryBudget::new(128);
        for value in [
            Value::Loan(RuntimeLoan {
                task: 0,
                frame: 0,
                place: BytecodePlace {
                    slot: BytecodeSlotId::new(0),
                    ty: BytecodeTypeId::new(0),
                    projections: Vec::new(),
                    source_loan: None,
                },
                mode: BytecodeParameterMode::Ref,
            }),
            Value::Join(RuntimeJoin { task: 0, scope: 0 }),
        ] {
            assert!(matches!(
                snapshot_arguments(&[value], &heap, &[], &[], Some(&budget)),
                Err(super::VmError::Invariant(_))
            ));
            assert_eq!(budget.live_bytes(), 0);
        }
    }

    #[test]
    fn snapshot_preflight_bounds_data_depth_and_preserves_cycle_markers() {
        let mut heap = Heap::new(
            VmLimits::default(),
            vec![BytecodeTraceDescriptor::Ref {
                value: BytecodeTypeId::new(0),
            }],
        );
        let cycle = allocate(&mut heap, 0, HeapObject::Ref(None));
        heap.replace(
            cycle.heap_handle().unwrap(),
            HeapObject::Ref(Some(cycle.clone())),
            std::slice::from_ref(&cycle),
            &mut VmStatistics::default(),
        )
        .unwrap();
        assert_admitted_snapshot(
            &cycle,
            &heap,
            &[],
            &[],
            &RuntimeValue::Ref(Some(Box::new(RuntimeValue::Cycle(0)))),
        );

        let mut value = allocate(&mut heap, 0, HeapObject::Ref(None));
        for _ in 1..super::super::TEST_SNAPSHOT_MAX_DEPTH {
            let handle = heap
                .allocate(
                    BytecodeTypeId::new(0),
                    HeapObject::Ref(Some(value.clone())),
                    std::slice::from_ref(&value),
                    &mut VmStatistics::default(),
                )
                .unwrap();
            value = Value::Heap(handle);
        }
        let budget = VmMemoryBudget::new(1_048_576);
        let snapshots =
            snapshot_arguments(std::slice::from_ref(&value), &heap, &[], &[], Some(&budget))
                .unwrap();
        assert_eq!(budget.live_bytes(), 256 * 32);
        let copied = super::snapshot_detached_arguments(&snapshots, Some(&budget)).unwrap();
        assert_eq!(budget.live_bytes(), 2 * 256 * 32);
        let mut depth = 1;
        let mut tail = &copied[0];
        while let RuntimeValue::Ref(Some(value)) = tail {
            depth += 1;
            tail = value;
        }
        assert_eq!(depth, 256);
        assert_eq!(tail, &RuntimeValue::Ref(None));
        drop(copied);
        drop(snapshots);
        assert_eq!(budget.live_bytes(), 0);
        let handle = heap
            .allocate(
                BytecodeTypeId::new(0),
                HeapObject::Ref(Some(value.clone())),
                std::slice::from_ref(&value),
                &mut VmStatistics::default(),
            )
            .unwrap();
        assert!(matches!(
            snapshot_arguments(&[Value::Heap(handle)], &heap, &[], &[], Some(&budget)),
            Err(super::VmError::ResourceLimit {
                resource: "stack depth",
                limit: 256
            })
        ));
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn moved_and_iterator_states_never_escape_the_vm_boundary() {
        let mut heap = snapshot_heap();
        let moved = allocate(&mut heap, 1, HeapObject::Tuple(vec![None]));
        assert!(snapshot_value(&moved, &heap, &[], &[]).is_err());
        let moved_variant = allocate(
            &mut heap,
            8,
            HeapObject::Variant {
                variant: 1,
                payload: AggregatePayload::Tuple(vec![None]),
            },
        );
        assert!(snapshot_value(&moved_variant, &heap, &[], &[]).is_err());
        let moved_field = allocate(
            &mut heap,
            8,
            HeapObject::Variant {
                variant: 2,
                payload: AggregatePayload::Record(vec![(7, None)]),
            },
        );
        assert!(snapshot_value(&moved_field, &heap, &[], &[]).is_err());
        let iterator = allocate(
            &mut heap,
            14,
            HeapObject::Iterator {
                mode: BytecodeCursorMode::Own,
                source: None,
                next: 0,
                adapter: None,
            },
        );
        assert!(snapshot_value(&iterator, &heap, &[], &[]).is_err());
        let budget = VmMemoryBudget::new(1024);
        for value in [moved, moved_variant, moved_field, iterator] {
            assert!(matches!(
                snapshot_arguments(&[value], &heap, &[], &[], Some(&budget)),
                Err(super::VmError::Invariant(_))
            ));
            assert_eq!(budget.live_bytes(), 0);
        }

        let mut structural_variant_heap = Heap::new(
            VmLimits::default(),
            vec![BytecodeTraceDescriptor::Variant {
                nominal: None,
                arguments: Vec::new(),
                variants: vec![BytecodeVariant {
                    member: 0,
                    payload: BytecodeVariantPayload::Unit,
                }],
            }],
        );
        assert!(matches!(
            structural_variant_heap.type_descriptor(BytecodeTypeId::new(99)),
            Err(super::VmError::Invariant(message))
                if message == "heap type descriptor is missing"
        ));
        let structural_variant = allocate(
            &mut structural_variant_heap,
            0,
            HeapObject::Variant {
                variant: 0,
                payload: AggregatePayload::Unit,
            },
        );
        assert!(matches!(
            snapshot_value(&structural_variant, &structural_variant_heap, &[], &[]),
            Err(super::VmError::Invariant(message))
                if message == "a nominal variant has no nominal trace descriptor"
        ));
    }

    #[test]
    fn aggregate_payload_tracing_ignores_moved_values_and_preserves_order() {
        let mut traced = Vec::new();
        AggregatePayload::Unit.trace_values(&mut traced);
        AggregatePayload::Tuple(vec![Some(Value::Integer(1)), None, Some(Value::Integer(2))])
            .trace_values(&mut traced);
        AggregatePayload::Record(vec![
            (7, Some(Value::Integer(3))),
            (8, None),
            (9, Some(Value::Integer(4))),
        ])
        .trace_values(&mut traced);
        assert_eq!(
            traced,
            [
                Value::Integer(1),
                Value::Integer(2),
                Value::Integer(3),
                Value::Integer(4),
            ]
        );
    }
}
