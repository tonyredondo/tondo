use std::collections::BTreeMap;

use crate::bytecode::{BytecodeCallableId, BytecodeNominalId, BytecodeRangeKind, BytecodeTypeId};

use super::heap::{Heap, HeapHandle, HeapObject};
use super::{RuntimeValue, VmError};

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
    Heap(HeapHandle),
}

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
            | Self::Function { .. } => None,
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
        Value::Heap(handle) => {
            if let Some(id) = visiting.get(handle) {
                return Ok(RuntimeValue::Cycle(*id));
            }
            let id = visiting.len();
            visiting.insert(*handle, id);
            let result = snapshot_object(
                heap.get(*handle)?,
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
            closure: callable.index(),
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
            fields: fields
                .iter()
                .map(|(field, value)| Ok((*field, snapshot(present_value(value)?, visiting)?)))
                .collect::<Result<_, VmError>>()?,
        },
        HeapObject::Variant { variant, payload } => RuntimeValue::Variant {
            variant: *variant,
            payload: snapshot_payload(payload, heap, callable_names, nominal_names, visiting)?,
        },
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
                "an affine iterator state escaped through the VM boundary",
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

fn nominal_name(id: BytecodeNominalId, names: &[String]) -> String {
    names
        .get(id.index() as usize)
        .cloned()
        .unwrap_or_else(|| format!("nominal#{}", id.index()))
}
