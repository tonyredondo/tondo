//! Execute immutable artifact queries without entering an external host.

use super::*;
use crate::reflection::{
    ReflectCapability, ReflectTypeKind, ReflectionDescriptorKind as Descriptor,
    ReflectionOperation as Query, ReflectionTable,
};

fn handle(kind: Descriptor, index: u32, artifact_tag: [u8; 32]) -> RuntimeValue {
    RuntimeValue::Host {
        kind: RuntimeHostValueKind::Reflection(kind, artifact_tag),
        id: u64::from(index),
    }
}

fn variant(name: &str, index: u32, values: Vec<RuntimeValue>) -> RuntimeValue {
    RuntimeValue::Variant {
        name: name.to_owned(),
        variant: index,
        values,
    }
}

fn type_kind(kind: ReflectTypeKind) -> RuntimeValue {
    let (ordinal, values) = match kind {
        ReflectTypeKind::Primitive(kind) => {
            (0, vec![variant("PrimitiveKind", kind as u32, vec![])])
        }
        ReflectTypeKind::Record => (1, vec![]),
        ReflectTypeKind::Enum => (2, vec![]),
        ReflectTypeKind::Newtype => (3, vec![]),
        ReflectTypeKind::Tuple => (4, vec![]),
        ReflectTypeKind::Union => (5, vec![]),
        ReflectTypeKind::Function => (6, vec![]),
        ReflectTypeKind::Applied(kind) => (7, vec![variant("AppliedKind", kind as u32, vec![])]),
        ReflectTypeKind::Reference(kind) => {
            (8, vec![variant("ReferenceKind", kind as u32, vec![])])
        }
        ReflectTypeKind::Opaque => (9, vec![]),
    };
    variant("TypeKind", ordinal, values)
}

fn receiver_index(
    values: &[Value],
    expected: Descriptor,
    artifact_tag: &[u8; 32],
) -> Result<u32, VmError> {
    let [
        Value::Host(RuntimeValue::Host {
            kind: RuntimeHostValueKind::Reflection(kind, tag),
            id,
        }),
    ] = values
    else {
        return Err(VmError::invariant(
            "reflection query has an invalid receiver",
        ));
    };
    if *kind != expected {
        return Err(VmError::invariant(
            "reflection query has the wrong descriptor kind",
        ));
    }
    if tag != artifact_tag {
        return Err(VmError::invariant(
            "reflection value belongs to a different artifact",
        ));
    }
    u32::try_from(*id).map_err(|_| VmError::invariant("reflection handle is outside this artifact"))
}

// A borrowed plan lets admission precede every allocation in the returned value.
enum QueryResult<'a> {
    Handle(Descriptor, u32),
    OptionalHandle(Descriptor, Option<u32>),
    Handles(Descriptor, &'a [u32]),
    Text(&'a str),
    OptionalText(Option<&'a str>),
    Integer(u32),
    Bool(bool),
    Kind(ReflectTypeKind),
    Enum(&'static str, u32),
    Capabilities(&'a [ReflectCapability]),
}

impl QueryResult<'_> {
    fn bytes(&self) -> u64 {
        let node = super::super::TEST_DETACHED_VALUE_BYTES;
        match self {
            Self::Handle(..) | Self::Integer(_) | Self::Bool(_) => node,
            Self::OptionalHandle(_, value) => node * (1 + u64::from(value.is_some())),
            Self::Handles(_, values) => node.saturating_mul(values.len() as u64 + 1),
            Self::Text(value) => node.saturating_add(value.len() as u64),
            Self::OptionalText(value) => {
                node.saturating_add(value.map_or(0, |text| node.saturating_add(text.len() as u64)))
            }
            Self::Kind(_) => 256,
            Self::Enum(name, _) => node + name.len() as u64,
            Self::Capabilities(values) => node.saturating_add(
                (values.len() as u64).saturating_mul(node + "TypeCapability".len() as u64),
            ),
        }
    }

    fn materialize(self, tag: [u8; 32]) -> RuntimeValue {
        match self {
            Self::Handle(kind, index) => handle(kind, index, tag),
            Self::OptionalHandle(kind, index) => index.map_or(RuntimeValue::OptionNone, |index| {
                RuntimeValue::OptionSome(Box::new(handle(kind, index, tag)))
            }),
            Self::Handles(kind, values) => RuntimeValue::Array(
                values
                    .iter()
                    .map(|index| handle(kind, *index, tag))
                    .collect(),
            ),
            Self::Text(value) => RuntimeValue::String(value.to_owned()),
            Self::OptionalText(value) => value.map_or(RuntimeValue::OptionNone, |text| {
                RuntimeValue::OptionSome(Box::new(RuntimeValue::String(text.to_owned())))
            }),
            Self::Integer(value) => RuntimeValue::Integer(i128::from(value)),
            Self::Bool(value) => RuntimeValue::Bool(value),
            Self::Kind(kind) => type_kind(kind),
            Self::Enum(name, index) => variant(name, index, vec![]),
            Self::Capabilities(values) => RuntimeValue::Set(
                values
                    .iter()
                    .map(|kind| variant("TypeCapability", *kind as u32, vec![]))
                    .collect(),
            ),
        }
    }
}

fn query_result(
    table: &ReflectionTable,
    query: Query,
    index: u32,
) -> Result<QueryResult<'_>, VmError> {
    use QueryResult as R;
    let missing = || VmError::invariant("reflection handle is not retained");
    Ok(match query.receiver().unwrap_or(Descriptor::TypeInfo) {
        Descriptor::TypeInfo => {
            let record = table.types.get(index as usize).ok_or_else(missing)?;
            match query {
                Query::TypeInfo => R::Handle(Descriptor::TypeInfo, index),
                Query::Id => R::Handle(Descriptor::TypeId, index),
                Query::QualifiedName => R::Text(&record.qualified_name),
                Query::Kind => R::Kind(record.kind),
                Query::GenericArguments => {
                    R::Handles(Descriptor::TypeInfo, &record.generic_arguments)
                }
                Query::Capabilities => R::Capabilities(&record.capabilities),
                Query::Fields => R::Handles(Descriptor::FieldInfo, &record.fields),
                Query::Variants => R::Handles(Descriptor::VariantInfo, &record.variants),
                Query::TupleElements => R::Handles(Descriptor::TypeInfo, &record.tuple_elements),
                Query::Function => R::OptionalHandle(Descriptor::FunctionInfo, record.function),
                _ => return Err(VmError::invariant("invalid TypeInfo query")),
            }
        }
        Descriptor::FieldInfo => {
            let record = table.fields.get(index as usize).ok_or_else(missing)?;
            match query {
                Query::FieldName => R::Text(&record.name),
                Query::FieldType => R::Handle(Descriptor::TypeInfo, record.ty),
                Query::FieldOrdinal => R::Integer(record.ordinal),
                Query::FieldDocs => R::OptionalText(record.docs.as_deref()),
                _ => return Err(VmError::invariant("invalid FieldInfo query")),
            }
        }
        Descriptor::VariantInfo => {
            let record = table.variants.get(index as usize).ok_or_else(missing)?;
            match query {
                Query::VariantName => R::Text(&record.name),
                Query::VariantOrdinal => R::Integer(record.ordinal),
                Query::VariantPayloadKind => {
                    R::Enum("VariantPayloadKind", record.payload_kind as u32)
                }
                Query::VariantTupleElements => {
                    R::Handles(Descriptor::TypeInfo, &record.tuple_types)
                }
                Query::VariantFields => R::Handles(Descriptor::FieldInfo, &record.record_fields),
                _ => return Err(VmError::invariant("invalid VariantInfo query")),
            }
        }
        Descriptor::ParameterInfo => {
            let record = table.parameters.get(index as usize).ok_or_else(missing)?;
            match query {
                Query::ParameterPosition => R::Integer(record.position),
                Query::ParameterType => R::Handle(Descriptor::TypeInfo, record.ty),
                Query::ParameterMode => R::Enum("ParameterMode", record.mode as u32),
                _ => return Err(VmError::invariant("invalid ParameterInfo query")),
            }
        }
        Descriptor::FunctionInfo => {
            let record = table.functions.get(index as usize).ok_or_else(missing)?;
            match query {
                Query::FunctionParameters => {
                    R::Handles(Descriptor::ParameterInfo, &record.parameters)
                }
                Query::FunctionOutcome => R::Handle(Descriptor::TypeInfo, record.outcome),
                Query::FunctionVariadic => R::Bool(record.variadic),
                Query::FunctionSuspends => R::Bool(record.suspends),
                Query::FunctionUnsafe => R::Bool(record.unsafe_),
                _ => return Err(VmError::invariant("invalid FunctionInfo query")),
            }
        }
        Descriptor::TypeId => return Err(VmError::invariant("TypeId has no queries")),
    })
}

impl Engine<'_, '_> {
    pub(super) fn prepare_reflection_call(
        &mut self,
        callable: BytecodeCallableId,
        outcome: BytecodeTypeId,
        values: &[Value],
    ) -> Result<Option<OperationResult>, VmError> {
        let table = &self.program.reflection;
        let Ok(position) = table
            .calls
            .binary_search_by_key(&callable, |call| call.callable)
        else {
            return Ok(None);
        };
        let call = &table.calls[position];
        let index = if call.operation == Query::TypeInfo {
            if !values.is_empty() {
                return Err(VmError::invariant("typeInfo received value arguments"));
            }
            call.root
                .ok_or_else(|| VmError::invariant("typeInfo has no retained root"))?
        } else {
            receiver_index(
                values,
                call.operation.receiver().expect("query has a receiver"),
                &table.artifact_tag,
            )?
        };
        let plan = query_result(table, call.operation, index)?;
        let bytes = plan.bytes();
        if bytes > self.limits.max_heap_bytes {
            return Err(VmError::ResourceLimit {
                resource: "reflection result bytes",
                limit: self.limits.max_heap_bytes,
            });
        }
        let owner = self.current_test_memory();
        let mut response = super::super::VmHostReturnBudget::new(owner.as_ref());
        response.reserve(bytes, [])?;
        let value = plan.materialize(table.artifact_tag);
        response.shrink(
            value
                .retained_bytes()
                .ok_or_else(|| VmError::invariant("reflection result size overflow"))?,
        )?;
        let returned = response.finish(value)?;
        let result =
            self.materialize_host_value_with_charge(outcome, returned.value, returned.memory)?;
        Ok(Some(OperationResult::Value(result)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflection_receiver_rejects_foreign_identity_and_malformed_handles() {
        let own = Value::Host(handle(Descriptor::TypeInfo, 7, [1; 32]));
        assert_eq!(
            receiver_index(std::slice::from_ref(&own), Descriptor::TypeInfo, &[1; 32]).unwrap(),
            7
        );
        assert!(
            matches!(receiver_index(&[own], Descriptor::TypeInfo, &[2; 32]),
            Err(VmError::Invariant(message)) if message.contains("different artifact"))
        );
        for values in [
            vec![],
            vec![Value::Integer(7)],
            vec![Value::Host(handle(Descriptor::FieldInfo, 7, [1; 32]))],
            vec![Value::Host(RuntimeValue::Host {
                kind: RuntimeHostValueKind::Reflection(Descriptor::TypeInfo, [1; 32]),
                id: u64::MAX,
            })],
        ] {
            assert!(receiver_index(&values, Descriptor::TypeInfo, &[1; 32]).is_err());
        }
    }
}
