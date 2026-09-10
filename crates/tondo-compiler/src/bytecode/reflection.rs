//! Lower explicit reflection roots from resolved, concrete semantic types.

use super::*;
use crate::hir::{HirBootstrapHostFunction, HirField, HirTypeDeclarationKind, HirVariantPayload};
use crate::resolve::Visibility;
use tondo_vm::reflection::*;

pub(super) fn lower(
    resolved: &ResolvedProgram,
    hir: &HirProgram,
    monomorphization: &mut Monomorphization,
    catalog: &TypeCatalog,
    callables: &BTreeMap<ExecutableInstance, bc::BytecodeCallableId>,
    program: &bc::BytecodeProgram,
    limit: u32,
) -> Result<ReflectionTable, BytecodeError> {
    let mut builder = Builder {
        resolved,
        hir,
        interner: &mut monomorphization.interner,
        ids: BTreeMap::new(),
        pending: VecDeque::new(),
        types: Vec::new(),
        table: ReflectionTable::default(),
        limit,
    };
    for (instance, callable) in callables {
        let ExecutableInstance::Named(instance) = instance else {
            continue;
        };
        let HirCallableId::Host(HirBootstrapHostFunction::Reflection(operation)) =
            instance.callable
        else {
            continue;
        };
        let root = if operation == ReflectionOperation::TypeInfo {
            let [ty] = instance.arguments.as_slice() else {
                return Err(invalid("typeInfo requires one concrete type argument"));
            };
            Some(builder.intern(*ty)?)
        } else {
            None
        };
        builder.table.calls.push(ReflectionCall {
            callable: *callable,
            operation,
            root,
        });
    }
    while let Some(ty) = builder.pending.pop_front() {
        let record = builder.describe(ty, catalog.id(ty)?)?;
        builder.table.types.push(record);
    }
    let concrete = builder
        .types
        .iter()
        .map(|ty| catalog.id(*ty))
        .collect::<Result<Vec<_>, _>>()?;
    for (record, capabilities) in builder.table.types.iter_mut().zip(
        bc::derive_reflection_capabilities(program, &concrete).map_err(BytecodeError::Invariant)?,
    ) {
        record.capabilities = capabilities;
    }
    builder.table.calls.sort_by_key(|call| call.callable);
    if !builder.table.calls.is_empty() {
        // Bind opaque values to the exact deterministic code and descriptor
        // content. Neither paths, process IDs nor wall-clock time participate.
        let bytes =
            serde_json::to_vec(&(program, &builder.table)).map_err(|e| invalid(e.to_string()))?;
        let digest = crate::artifact::sha256(&bytes);
        builder.table.artifact_tag = crate::reflect::artifact_tag(&digest);
    }
    Ok(builder.table)
}

use std::collections::VecDeque;

struct Builder<'a> {
    resolved: &'a ResolvedProgram,
    hir: &'a HirProgram,
    interner: &'a mut TypeInterner,
    ids: BTreeMap<TypeId, u32>,
    pending: VecDeque<TypeId>,
    types: Vec<TypeId>,
    table: ReflectionTable,
    limit: u32,
}

fn invalid(message: impl Into<String>) -> BytecodeError {
    BytecodeError::construction("reflection metadata", message)
}

impl Builder<'_> {
    fn admit_descriptor(&self) -> Result<(), BytecodeError> {
        let count = self.types.len()
            + self.table.fields.len()
            + self.table.variants.len()
            + self.table.parameters.len()
            + self.table.functions.len();
        ensure_count(
            count.saturating_add(1),
            self.limit,
            None,
            "reflection descriptors",
        )
    }

    fn intern(&mut self, ty: TypeId) -> Result<u32, BytecodeError> {
        if let Some(id) = self.ids.get(&ty) {
            return Ok(*id);
        }
        self.admit_descriptor()?;
        let id = checked_index(self.types.len(), "reflection type")?;
        self.ids.insert(ty, id);
        self.types.push(ty);
        self.pending.push_back(ty);
        Ok(id)
    }

    fn children(&mut self, types: &[TypeId]) -> Result<Vec<u32>, BytecodeError> {
        types.iter().map(|ty| self.intern(*ty)).collect()
    }

    fn describe(
        &mut self,
        ty: TypeId,
        source_type: bc::BytecodeTypeId,
    ) -> Result<ReflectionTypeRecord, BytecodeError> {
        let kind = self
            .interner
            .kind(ty)
            .map_err(|e| invalid(e.to_string()))?
            .clone();
        let mut record = ReflectionTypeRecord {
            source_type,
            qualified_name: self
                .interner
                .canonical_interface(ty)
                .map_err(|e| invalid(e.to_string()))?,
            kind: ReflectTypeKind::Opaque,
            generic_arguments: vec![],
            capabilities: vec![],
            fields: vec![],
            variants: vec![],
            tuple_elements: vec![],
            function: None,
        };
        match kind {
            TypeKind::Scalar(scalar) => {
                use ReflectPrimitiveKind as P;
                record.kind = ReflectTypeKind::Primitive(match scalar {
                    ScalarType::Bool => P::Bool,
                    ScalarType::Int => P::Int,
                    ScalarType::Int8 => P::Int8,
                    ScalarType::Int16 => P::Int16,
                    ScalarType::Int32 => P::Int32,
                    ScalarType::UInt8 => P::UInt8,
                    ScalarType::UInt16 => P::UInt16,
                    ScalarType::UInt32 => P::UInt32,
                    ScalarType::UInt64 => P::UInt64,
                    ScalarType::Float => P::Float,
                    ScalarType::Float32 => P::Float32,
                    ScalarType::Byte => P::Byte,
                    ScalarType::Char => P::Char,
                    ScalarType::String => P::String,
                    ScalarType::Unit => P::Unit,
                    ScalarType::Never => P::Never,
                });
            }
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                record.generic_arguments = self.children(&arguments)?;
                let substitution = TypeSubstitution::new(arguments.clone());
                let declaration = self.hir.declarations().find_map(|(symbol, declaration)| {
                    (self.resolved.symbol(*symbol)?.identity() == &identity).then_some(declaration)
                });
                if let Some(declaration) = declaration {
                    let HirTypeDeclarationKind::Nominal(nominal) = declaration.kind() else {
                        return Err(invalid("a nominal descriptor resolved to an alias"));
                    };
                    let shape = nominal.shape().clone();
                    match shape {
                        HirNominalShape::Newtype { .. } => record.kind = ReflectTypeKind::Newtype,
                        HirNominalShape::Record { fields } => {
                            record.kind = ReflectTypeKind::Record;
                            record.fields = self.fields(&fields, &arguments)?;
                        }
                        HirNominalShape::Enum { variants } => {
                            record.kind = ReflectTypeKind::Enum;
                            for (ordinal, variant) in variants.iter().enumerate() {
                                let member = self
                                    .resolved
                                    .member(variant.member())
                                    .ok_or_else(|| invalid("variant member is missing"))?;
                                if member.visibility() != Visibility::Public {
                                    continue;
                                }
                                let mut descriptor = ReflectionVariantRecord {
                                    name: member.name().as_str().to_owned(),
                                    ordinal: checked_index(ordinal, "variant ordinal")?,
                                    payload_kind: match variant.payload() {
                                        HirVariantPayload::Unit => ReflectVariantPayloadKind::Unit,
                                        HirVariantPayload::Tuple(_) => {
                                            ReflectVariantPayloadKind::Tuple
                                        }
                                        HirVariantPayload::Record(_) => {
                                            ReflectVariantPayloadKind::Record
                                        }
                                    },
                                    tuple_types: vec![],
                                    record_fields: vec![],
                                };
                                match variant.payload() {
                                    HirVariantPayload::Unit => {}
                                    HirVariantPayload::Tuple(types) => {
                                        for template in types {
                                            let concrete = substitution
                                                .apply(self.interner, *template)
                                                .map_err(|e| invalid(e.to_string()))?;
                                            descriptor.tuple_types.push(self.intern(concrete)?);
                                        }
                                    }
                                    HirVariantPayload::Record(fields) => {
                                        descriptor.record_fields =
                                            self.fields(fields, &arguments)?
                                    }
                                }
                                self.admit_descriptor()?;
                                record.variants.push(checked_index(
                                    self.table.variants.len(),
                                    "reflection variant",
                                )?);
                                self.table.variants.push(descriptor);
                            }
                        }
                    }
                }
            }
            TypeKind::Tuple(types) | TypeKind::Union(types) => {
                record.kind = if matches!(self.interner.kind(ty), Ok(TypeKind::Tuple(_))) {
                    ReflectTypeKind::Tuple
                } else {
                    ReflectTypeKind::Union
                };
                record.tuple_elements = self.children(&types)?;
            }
            TypeKind::Function(function) => {
                record.kind = ReflectTypeKind::Function;
                let mut parameters = Vec::new();
                for (position, parameter) in function.parameters().iter().enumerate() {
                    let ty = self.intern(parameter.ty())?;
                    self.admit_descriptor()?;
                    parameters.push(checked_index(
                        self.table.parameters.len(),
                        "reflection parameter",
                    )?);
                    self.table.parameters.push(ReflectionParameterRecord {
                        position: checked_index(position, "parameter position")?,
                        ty,
                        mode: match parameter.mode() {
                            ParameterMode::Value => ReflectParameterMode::Value,
                            ParameterMode::Ref => ReflectParameterMode::Ref,
                            ParameterMode::Mut => ReflectParameterMode::Mut,
                            ParameterMode::Var => ReflectParameterMode::Var,
                        },
                    });
                }
                if let Some(element) = function.variadic() {
                    let ty = self.intern(element)?;
                    self.admit_descriptor()?;
                    parameters.push(checked_index(
                        self.table.parameters.len(),
                        "reflection parameter",
                    )?);
                    self.table.parameters.push(ReflectionParameterRecord {
                        position: checked_index(function.parameters().len(), "parameter position")?,
                        ty,
                        mode: ReflectParameterMode::Value,
                    });
                }
                let outcome = self.intern(function.outcome())?;
                self.admit_descriptor()?;
                record.function = Some(checked_index(
                    self.table.functions.len(),
                    "reflection function",
                )?);
                self.table.functions.push(ReflectionFunctionRecord {
                    parameters,
                    outcome,
                    variadic: function.variadic().is_some(),
                    suspends: function.suspends(),
                    unsafe_: function.is_unsafe(),
                });
            }
            TypeKind::Option(item) => {
                record.kind = ReflectTypeKind::Applied(ReflectAppliedKind::Option);
                record.generic_arguments = vec![self.intern(item)?];
            }
            TypeKind::Result { success, error } => {
                record.kind = ReflectTypeKind::Applied(ReflectAppliedKind::Result);
                record.generic_arguments = self.children(&[success, error])?;
            }
            TypeKind::Intrinsic {
                constructor,
                arguments,
            } => {
                record.generic_arguments = self.children(&arguments)?;
                record.kind = match constructor {
                    IntrinsicType::Array => ReflectTypeKind::Applied(ReflectAppliedKind::Array),
                    IntrinsicType::Map => ReflectTypeKind::Applied(ReflectAppliedKind::Map),
                    IntrinsicType::Set => ReflectTypeKind::Applied(ReflectAppliedKind::Set),
                    IntrinsicType::Range => ReflectTypeKind::Applied(ReflectAppliedKind::Range),
                    IntrinsicType::Ref => ReflectTypeKind::Reference(ReflectReferenceKind::Ref),
                    IntrinsicType::Pointer => {
                        ReflectTypeKind::Reference(ReflectReferenceKind::Pointer)
                    }
                    _ if !arguments.is_empty() => {
                        ReflectTypeKind::Applied(ReflectAppliedKind::Other)
                    }
                    _ => ReflectTypeKind::Opaque,
                };
            }
            TypeKind::OpaqueResult { arguments, .. } => {
                record.generic_arguments = self.children(&arguments)?
            }
            TypeKind::Generated { .. } => {}
            TypeKind::Error
            | TypeKind::GenericParameter(_)
            | TypeKind::Inference(_)
            | TypeKind::Cursor { .. } => {
                return Err(invalid("typeInfo requires a describable concrete type"));
            }
        }
        Ok(record)
    }

    fn fields(
        &mut self,
        fields: &[HirField],
        arguments: &[TypeId],
    ) -> Result<Vec<u32>, BytecodeError> {
        let mut result = Vec::new();
        let substitution = TypeSubstitution::new(arguments.to_vec());
        for (ordinal, field) in fields.iter().enumerate() {
            let member = self
                .resolved
                .member(field.member())
                .ok_or_else(|| invalid("field member is missing"))?;
            if member.visibility() != Visibility::Public {
                continue;
            }
            let name = member.name().as_str().to_owned();
            let docs = member.docs().map(str::to_owned);
            let concrete = substitution
                .apply(self.interner, field.ty())
                .map_err(|e| invalid(e.to_string()))?;
            let ty = self.intern(concrete)?;
            self.admit_descriptor()?;
            result.push(checked_index(self.table.fields.len(), "reflection field")?);
            self.table.fields.push(ReflectionFieldRecord {
                name,
                ty,
                docs,
                ordinal: checked_index(ordinal, "field ordinal")?,
            });
        }
        Ok(result)
    }
}
