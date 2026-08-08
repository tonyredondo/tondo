use std::collections::BTreeMap;

use crate::bytecode::{
    BytecodeCallableId, BytecodeNominalId, BytecodeParameterMode, BytecodePlace, BytecodeRangeKind,
    BytecodeTypeId,
};

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
    use crate::runtime::{RuntimeHostValueKind, RuntimeValue, VmLimits, VmStatistics};

    use super::{AggregatePayload, RuntimeJoin, RuntimeLoan, Value, snapshot_value};

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
                    fields: vec![(4, RuntimeValue::Integer(7))],
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
                    variant: 0,
                    payload: Vec::new(),
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
                    variant: 1,
                    payload: vec![(None, RuntimeValue::Integer(8))],
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
                    variant: 2,
                    payload: vec![(Some(7), RuntimeValue::Integer(9))],
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
        }
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
