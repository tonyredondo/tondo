//! Admission of compiler-owned descriptive metadata and its exact query calls.

use super::*;
use crate::reflection::{
    ReflectAppliedKind as A, ReflectCapability as C, ReflectPrimitiveKind as P,
    ReflectReferenceKind as R, ReflectTypeKind as K, ReflectionDescriptorKind as D,
    ReflectionOperation as Q, ReflectionTypeRecord,
};

fn invalid(message: impl Into<String>) -> BytecodeVerificationError {
    BytecodeVerificationError::new("reflection metadata", message)
}

impl Verifier<'_> {
    pub(super) fn verify_reflection(&self) -> Result<(), BytecodeVerificationError> {
        let table = &self.program.reflection;
        let mut previous = None;
        let mut pending = VecDeque::new();
        for call in &table.calls {
            self.consume_dataflow_step("reflection calls")?;
            if previous.is_some_and(|id| id >= call.callable) {
                return Err(invalid("calls are not uniquely ordered"));
            }
            previous = Some(call.callable);
            let callable = self
                .program
                .callable(call.callable)
                .ok_or_else(|| invalid("callable is missing"))?;
            if callable.implementation.is_some()
                || callable.closure.is_some()
                || callable.name.split('[').next() != Some(call.operation.name())
            {
                return Err(invalid("query identity differs from its callable"));
            }
            if call.operation == Q::TypeInfo {
                if !callable.parameters.is_empty()
                    || !self.reflection_descriptor(callable.outcome, D::TypeInfo)
                {
                    return Err(invalid("typeInfo has an invalid signature"));
                }
                pending.push_back(
                    call.root
                        .ok_or_else(|| invalid("typeInfo has no concrete root"))?,
                );
            } else {
                let [receiver] = callable.parameters.as_slice() else {
                    return Err(invalid("query must have exactly one receiver"));
                };
                if call.root.is_some()
                    || !receiver.receiver
                    || receiver.mode != BytecodeParameterMode::Value
                    || !call
                        .operation
                        .receiver()
                        .is_some_and(|kind| self.reflection_descriptor(receiver.ty, kind))
                {
                    return Err(invalid("query receiver or root is invalid"));
                }
                let outcome = callable.outcome;
                let valid = match call.operation {
                    Q::Id => self.reflection_descriptor(outcome, D::TypeId),
                    Q::QualifiedName | Q::FieldName | Q::VariantName => matches!(
                        self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Scalar(BytecodeScalarType::String))
                    ),
                    Q::FieldOrdinal | Q::VariantOrdinal | Q::ParameterPosition => matches!(
                        self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Scalar(BytecodeScalarType::Int))
                    ),
                    Q::FunctionVariadic | Q::FunctionSuspends | Q::FunctionUnsafe => matches!(
                        self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Scalar(BytecodeScalarType::Bool))
                    ),
                    Q::FieldType | Q::ParameterType | Q::FunctionOutcome => {
                        self.reflection_descriptor(outcome, D::TypeInfo)
                    }
                    Q::FieldDocs => matches!(self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Option(ty)) if matches!(self.program.ty(*ty).map(|t| &t.kind),
                            Some(BytecodeTypeKind::Scalar(BytecodeScalarType::String)))),
                    Q::ParameterMode => self.reflection_nominal(outcome, "ParameterMode"),
                    Q::VariantPayloadKind => self.reflection_nominal(outcome, "VariantPayloadKind"),
                    Q::Kind => self.reflection_nominal(outcome, "TypeKind"),
                    Q::GenericArguments | Q::TupleElements | Q::VariantTupleElements => {
                        self.reflection_array(outcome, D::TypeInfo)
                    }
                    Q::Fields | Q::VariantFields => self.reflection_array(outcome, D::FieldInfo),
                    Q::FunctionParameters => self.reflection_array(outcome, D::ParameterInfo),
                    Q::Variants => self.reflection_array(outcome, D::VariantInfo),
                    Q::Capabilities => matches!(self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Intrinsic { constructor: BytecodeIntrinsicType::Set, arguments })
                            if arguments.len() == 1 && self.reflection_nominal(arguments[0], "TypeCapability")),
                    Q::Function => matches!(self.program.ty(outcome).map(|t| &t.kind),
                        Some(BytecodeTypeKind::Option(ty)) if self.reflection_descriptor(*ty, D::FunctionInfo)),
                    Q::TypeInfo => false,
                };
                if !valid {
                    return Err(invalid("query outcome differs from its public contract"));
                }
            }
        }
        for (index, callable) in self.program.callables.iter().enumerate() {
            let name = callable.name.split('[').next().unwrap_or(&callable.name);
            if (name == Q::TypeInfo.name() || Q::QUERIES.iter().any(|query| query.name() == name))
                && table
                    .calls
                    .binary_search_by_key(&BytecodeCallableId::new(index as u32), |call| {
                        call.callable
                    })
                    .is_err()
            {
                return Err(invalid("reflection callable has no admitted query"));
            }
        }

        let mut types = BTreeSet::new();
        let mut fields = BTreeSet::new();
        let mut variants = BTreeSet::new();
        let mut parameters = BTreeSet::new();
        let mut functions = BTreeSet::new();
        let mut identities = BTreeSet::new();
        while let Some(index) = pending.pop_front() {
            self.consume_dataflow_step("reflection closure")?;
            if !types.insert(index) {
                continue;
            }
            let record = table
                .types
                .get(index as usize)
                .ok_or_else(|| invalid("type reference is outside the table"))?;
            if !identities.insert(record.source_type)
                || self.program.ty(record.source_type).is_none()
            {
                return Err(invalid("source type is missing or described twice"));
            }
            self.reflection_text(&record.qualified_name)?;
            if record.qualified_name.is_empty() {
                return Err(invalid("qualified type name is empty"));
            }
            self.verify_reflection_shape(record)?;
            let mut expected = Vec::new();
            for (capability, reflected) in [
                (ClosedCapability::Copy, C::Copy),
                (ClosedCapability::Discard, C::Discard),
                (ClosedCapability::Equatable, C::Equatable),
                (ClosedCapability::Key, C::Key),
                (ClosedCapability::Send, C::Send),
                (ClosedCapability::Share, C::Share),
            ] {
                if self.capability(record.source_type, capability, "reflection capabilities")? {
                    expected.push(reflected);
                }
            }
            if record.capabilities != expected {
                return Err(invalid(
                    "reflected capabilities differ from the verified type",
                ));
            }
            if (!record.fields.is_empty() && record.kind != K::Record)
                || (!record.variants.is_empty() && record.kind != K::Enum)
                || (!record.tuple_elements.is_empty()
                    && !matches!(record.kind, K::Tuple | K::Union))
                || (record.function.is_some() != (record.kind == K::Function))
            {
                return Err(invalid(
                    "descriptor views are inapplicable to their type kind",
                ));
            }
            self.queue_reflection_types(&record.generic_arguments, &mut pending)?;
            self.queue_reflection_types(&record.tuple_elements, &mut pending)?;
            self.verify_reflection_fields(&record.fields, &mut fields, &mut pending)?;
            let mut previous = None;
            for index in &record.variants {
                self.consume_dataflow_step("reflection variants")?;
                let variant = table
                    .variants
                    .get(*index as usize)
                    .ok_or_else(|| invalid("variant is missing"))?;
                if !variants.insert(*index)
                    || previous.is_some_and(|ordinal| ordinal >= variant.ordinal)
                {
                    return Err(invalid("variant ownership or ordinal is duplicated"));
                }
                previous = Some(variant.ordinal);
                self.reflection_text(&variant.name)?;
                if variant.name.is_empty()
                    || (!variant.tuple_types.is_empty() && !variant.record_fields.is_empty())
                {
                    return Err(invalid("variant name or payload is invalid"));
                }
                self.queue_reflection_types(&variant.tuple_types, &mut pending)?;
                self.verify_reflection_fields(&variant.record_fields, &mut fields, &mut pending)?;
            }
            if let Some(index) = record.function {
                let function = table
                    .functions
                    .get(index as usize)
                    .ok_or_else(|| invalid("function descriptor is missing"))?;
                if !functions.insert(index) {
                    return Err(invalid("function descriptor has multiple owners"));
                }
                pending.push_back(function.outcome);
                for (position, index) in function.parameters.iter().enumerate() {
                    self.consume_dataflow_step("reflection parameters")?;
                    let parameter = table
                        .parameters
                        .get(*index as usize)
                        .ok_or_else(|| invalid("parameter descriptor is missing"))?;
                    if !parameters.insert(*index) || parameter.position as usize != position {
                        return Err(invalid("parameter position or ownership is invalid"));
                    }
                    pending.push_back(parameter.ty);
                }
            }
        }
        if types.len() != table.types.len()
            || fields.len() != table.fields.len()
            || variants.len() != table.variants.len()
            || parameters.len() != table.parameters.len()
            || functions.len() != table.functions.len()
        {
            return Err(invalid("unreachable descriptors were retained"));
        }
        Ok(())
    }

    fn queue_reflection_types(
        &self,
        indices: &[u32],
        pending: &mut VecDeque<u32>,
    ) -> Result<(), BytecodeVerificationError> {
        for index in indices {
            self.consume_dataflow_step("reflection references")?;
            pending.push_back(*index);
        }
        Ok(())
    }

    fn reflected_source(&self, index: u32) -> Result<BytecodeTypeId, BytecodeVerificationError> {
        self.program
            .reflection
            .types
            .get(index as usize)
            .map(|record| record.source_type)
            .ok_or_else(|| invalid("type reference is outside the table"))
    }

    fn reflection_types_match(
        &self,
        indices: &[u32],
        expected: &[BytecodeTypeId],
        arguments: &[BytecodeTypeId],
    ) -> Result<bool, BytecodeVerificationError> {
        if indices.len() != expected.len() {
            return Ok(false);
        }
        for (index, expected) in indices.iter().zip(expected) {
            self.consume_dataflow_step("reflection type shape")?;
            if !self.type_matches_substitution(
                *expected,
                self.reflected_source(*index)?,
                arguments,
                "reflection type shape",
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn reflection_field_types_match(
        &self,
        indices: &[u32],
        expected: &[BytecodeField],
        arguments: &[BytecodeTypeId],
    ) -> Result<bool, BytecodeVerificationError> {
        for index in indices {
            self.consume_dataflow_step("reflection field shape")?;
            let field = self
                .program
                .reflection
                .fields
                .get(*index as usize)
                .ok_or_else(|| invalid("field is missing"))?;
            let Some(expected) = expected.get(field.ordinal as usize) else {
                return Ok(false);
            };
            if !self.type_matches_substitution(
                expected.ty,
                self.reflected_source(field.ty)?,
                arguments,
                "reflection field shape",
            )? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn verify_reflection_shape(
        &self,
        record: &ReflectionTypeRecord,
    ) -> Result<(), BytecodeVerificationError> {
        let source = self
            .program
            .ty(record.source_type)
            .ok_or_else(|| invalid("source type is missing"))?;
        let mut generic_arguments: &[BytecodeTypeId] = &[];
        let mut tuple_elements: &[BytecodeTypeId] = &[];
        let pair;
        let kind = match &source.kind {
            BytecodeTypeKind::Scalar(scalar) => K::Primitive(match scalar {
                BytecodeScalarType::Bool => P::Bool,
                BytecodeScalarType::Int => P::Int,
                BytecodeScalarType::Int8 => P::Int8,
                BytecodeScalarType::Int16 => P::Int16,
                BytecodeScalarType::Int32 => P::Int32,
                BytecodeScalarType::UInt8 => P::UInt8,
                BytecodeScalarType::UInt16 => P::UInt16,
                BytecodeScalarType::UInt32 => P::UInt32,
                BytecodeScalarType::UInt64 => P::UInt64,
                BytecodeScalarType::Float => P::Float,
                BytecodeScalarType::Float32 => P::Float32,
                BytecodeScalarType::Byte => P::Byte,
                BytecodeScalarType::Char => P::Char,
                BytecodeScalarType::String => P::String,
                BytecodeScalarType::Unit => P::Unit,
                BytecodeScalarType::Never => P::Never,
            }),
            BytecodeTypeKind::Nominal {
                nominal, arguments, ..
            } => {
                generic_arguments = arguments;
                match nominal
                    .and_then(|id| self.program.nominals.get(id.index() as usize))
                    .map(|n| &n.shape)
                {
                    None => K::Opaque,
                    Some(BytecodeNominalShape::Newtype { .. }) => K::Newtype,
                    Some(BytecodeNominalShape::Record { fields }) => {
                        if !self.reflection_field_types_match(&record.fields, fields, arguments)? {
                            return Err(invalid(
                                "field types differ from their nominal declaration",
                            ));
                        }
                        K::Record
                    }
                    Some(BytecodeNominalShape::Enum { variants }) => {
                        for index in &record.variants {
                            self.consume_dataflow_step("reflection variant shape")?;
                            let variant = self
                                .program
                                .reflection
                                .variants
                                .get(*index as usize)
                                .ok_or_else(|| invalid("variant is missing"))?;
                            let expected =
                                variants.get(variant.ordinal as usize).ok_or_else(|| {
                                    invalid("variant ordinal is outside its nominal declaration")
                                })?;
                            use crate::reflection::ReflectVariantPayloadKind as V;
                            let valid = match &expected.payload {
                                BytecodeVariantPayload::Unit => {
                                    variant.payload_kind == V::Unit
                                        && variant.tuple_types.is_empty()
                                        && variant.record_fields.is_empty()
                                }
                                BytecodeVariantPayload::Tuple(types) => {
                                    variant.payload_kind == V::Tuple
                                        && variant.record_fields.is_empty()
                                        && self.reflection_types_match(
                                            &variant.tuple_types,
                                            types,
                                            arguments,
                                        )?
                                }
                                BytecodeVariantPayload::Record(fields) => {
                                    variant.payload_kind == V::Record
                                        && variant.tuple_types.is_empty()
                                        && self.reflection_field_types_match(
                                            &variant.record_fields,
                                            fields,
                                            arguments,
                                        )?
                                }
                            };
                            if !valid {
                                return Err(invalid(
                                    "variant payload differs from its nominal declaration",
                                ));
                            }
                        }
                        K::Enum
                    }
                }
            }
            BytecodeTypeKind::Tuple(types) => {
                tuple_elements = types;
                K::Tuple
            }
            BytecodeTypeKind::Union(types) => {
                tuple_elements = types;
                K::Union
            }
            BytecodeTypeKind::Function(signature) => {
                let function = record
                    .function
                    .and_then(|index| self.program.reflection.functions.get(index as usize))
                    .ok_or_else(|| invalid("function descriptor is missing"))?;
                if function.variadic != signature.variadic.is_some()
                    || function.suspends != signature.is_async
                    || function.unsafe_ != signature.is_unsafe
                    || function.parameters.len()
                        != signature.parameters.len() + usize::from(signature.variadic.is_some())
                    || self.reflected_source(function.outcome)? != signature.outcome
                {
                    return Err(invalid("function descriptor differs from its signature"));
                }
                for (position, index) in function.parameters.iter().enumerate() {
                    self.consume_dataflow_step("reflection parameter shape")?;
                    let parameter = self
                        .program
                        .reflection
                        .parameters
                        .get(*index as usize)
                        .ok_or_else(|| invalid("parameter descriptor is missing"))?;
                    let (ty, mode) = if let Some(expected) = signature.parameters.get(position) {
                        (expected.ty, expected.mode)
                    } else {
                        (
                            signature
                                .variadic
                                .ok_or_else(|| invalid("unexpected variadic parameter"))?,
                            BytecodeParameterMode::Value,
                        )
                    };
                    use crate::reflection::ReflectParameterMode as M;
                    let mode = match mode {
                        BytecodeParameterMode::Value => M::Value,
                        BytecodeParameterMode::Ref => M::Ref,
                        BytecodeParameterMode::Mut => M::Mut,
                        BytecodeParameterMode::Var => M::Var,
                    };
                    if self.reflected_source(parameter.ty)? != ty || parameter.mode != mode {
                        return Err(invalid("parameter descriptor differs from its signature"));
                    }
                }
                K::Function
            }
            BytecodeTypeKind::Option(item) => {
                generic_arguments = std::slice::from_ref(item);
                K::Applied(A::Option)
            }
            BytecodeTypeKind::Result { success, error } => {
                pair = [*success, *error];
                generic_arguments = &pair;
                K::Applied(A::Result)
            }
            BytecodeTypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                generic_arguments = arguments;
                match constructor {
                    BytecodeIntrinsicType::Array => K::Applied(A::Array),
                    BytecodeIntrinsicType::Map => K::Applied(A::Map),
                    BytecodeIntrinsicType::Set => K::Applied(A::Set),
                    BytecodeIntrinsicType::Range => K::Applied(A::Range),
                    BytecodeIntrinsicType::Ref => K::Reference(R::Ref),
                    BytecodeIntrinsicType::Pointer => K::Reference(R::Pointer),
                    _ if !arguments.is_empty() => K::Applied(A::Other),
                    _ => K::Opaque,
                }
            }
            BytecodeTypeKind::OpaqueResult { arguments, .. } => {
                generic_arguments = arguments;
                K::Opaque
            }
            BytecodeTypeKind::Generated { .. } => K::Opaque,
            BytecodeTypeKind::GenericParameter(_) | BytecodeTypeKind::Cursor { .. } => {
                return Err(invalid("source type is not describable"));
            }
        };
        if record.kind != kind
            || !self.reflection_types_match(&record.generic_arguments, generic_arguments, &[])?
            || !self.reflection_types_match(&record.tuple_elements, tuple_elements, &[])?
        {
            return Err(invalid(
                "descriptor kind or arguments differ from the verified type",
            ));
        }
        Ok(())
    }

    fn verify_reflection_fields(
        &self,
        indices: &[u32],
        seen: &mut BTreeSet<u32>,
        pending: &mut VecDeque<u32>,
    ) -> Result<(), BytecodeVerificationError> {
        let mut previous = None;
        for index in indices {
            self.consume_dataflow_step("reflection fields")?;
            let field = self
                .program
                .reflection
                .fields
                .get(*index as usize)
                .ok_or_else(|| invalid("field is missing"))?;
            if !seen.insert(*index) || previous.is_some_and(|ordinal| ordinal >= field.ordinal) {
                return Err(invalid("field ownership or ordinal is duplicated"));
            }
            previous = Some(field.ordinal);
            if field.name.is_empty() {
                return Err(invalid("field name is empty"));
            }
            self.reflection_text(&field.name)?;
            if let Some(docs) = &field.docs {
                self.reflection_text(docs)?;
            }
            pending.push_back(field.ty);
        }
        Ok(())
    }

    fn reflection_text(&self, text: &str) -> Result<(), BytecodeVerificationError> {
        let next = self.dataflow_steps.get().saturating_add(text.len() as u64);
        if next > self.limits.max_dataflow_steps {
            return Err(BytecodeVerificationError::resource_limit(
                "reflection text",
                "descriptor text exceeds verification work budget",
            ));
        }
        self.dataflow_steps.set(next);
        Ok(())
    }

    fn reflection_descriptor(&self, ty: BytecodeTypeId, kind: D) -> bool {
        matches!(self.program.ty(ty).map(|t| &t.kind), Some(BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Reflection(actual), arguments }) if *actual == kind && arguments.is_empty())
    }

    fn reflection_array(&self, ty: BytecodeTypeId, kind: D) -> bool {
        matches!(self.program.ty(ty).map(|t| &t.kind), Some(BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Array, arguments }) if arguments.len()==1 && self.reflection_descriptor(arguments[0],kind))
    }

    fn reflection_nominal(&self, ty: BytecodeTypeId, name: &str) -> bool {
        matches!(self.program.ty(ty).map(|t| &t.kind), Some(BytecodeTypeKind::Nominal { identity, arguments, .. })
            if identity == &format!("@27:toolchain:std:0.1-bootstrap::reflect::type::{name}") && arguments.is_empty())
    }
}
