//! Source-visible reflection signatures and closed nominal descriptors.

use super::*;
use tondo_vm::reflection::{ReflectionDescriptorKind as Descriptor, ReflectionOperation as Query};

impl TypeLowerer<'_> {
    pub(super) fn lower_reflection_nominals(&mut self) -> Result<(), HirError> {
        let path = ModulePath::new("reflect")?;
        let Some(module) = self.packages.module(self.packages.standard(), &path) else {
            return Ok(());
        };
        for (name, variants) in tondo_vm::reflection::REFLECTION_ENUMS {
            let name = Name::new(*name).expect("reflection nominal name is valid");
            let Some(symbol) = self.resolved.bootstrap_nominal(&module, &name) else {
                continue;
            };
            let declaration = self
                .resolved
                .symbol(symbol)
                .expect("reflection nominal is indexed");
            let self_type = self
                .interner
                .nominal(declaration.identity().clone(), Vec::new())?;
            let mut members = Vec::new();
            for variant in *variants {
                let payload = if name.as_str() == "TypeKind" {
                    match *variant {
                        "Primitive" => vec![self.bootstrap_nominal_type(&module, "PrimitiveKind")?],
                        "Applied" => vec![self.bootstrap_nominal_type(&module, "AppliedKind")?],
                        "Reference" => vec![self.bootstrap_nominal_type(&module, "ReferenceKind")?],
                        _ => Vec::new(),
                    }
                } else {
                    Vec::new()
                };
                members.push(self.bootstrap_variant(symbol, variant, payload));
            }
            self.declarations.insert(
                symbol,
                HirTypeDeclaration {
                    symbol,
                    span: declaration.span(),
                    parameters: Vec::new(),
                    kind: HirTypeDeclarationKind::Nominal(HirNominalDefinition {
                        self_type,
                        shape: HirNominalShape::Enum { variants: members },
                    }),
                },
            );
        }
        Ok(())
    }

    pub(super) fn push_reflection_contracts(&mut self, span: Span) -> Result<(), HirError> {
        let path = ModulePath::new("reflect")?;
        let Some(module) = self.packages.module(self.packages.standard(), &path) else {
            return Ok(());
        };
        if self
            .resolved
            .bootstrap_nominal(&module, &Name::new("TypeKind").unwrap())
            .is_none()
        {
            return Ok(());
        }
        let info = self
            .interner
            .intrinsic(IntrinsicType::Reflection(Descriptor::TypeInfo), vec![])?;
        self.push_bootstrap_generic_host_callable(
            span,
            HirBootstrapHostFunction::Reflection(Query::TypeInfo),
            vec![],
            info,
            1,
            vec![],
        )?;
        for operation in Query::QUERIES {
            let outcome = match operation {
                Query::Id => self
                    .interner
                    .intrinsic(IntrinsicType::Reflection(Descriptor::TypeId), vec![])?,
                Query::QualifiedName | Query::FieldName | Query::VariantName => {
                    self.interner.scalar(ScalarType::String)
                }
                Query::FieldOrdinal | Query::VariantOrdinal | Query::ParameterPosition => {
                    self.interner.scalar(ScalarType::Int)
                }
                Query::FieldType | Query::ParameterType | Query::FunctionOutcome => info,
                Query::FieldDocs => {
                    let string = self.interner.scalar(ScalarType::String);
                    self.interner.option(string)?
                }
                Query::ParameterMode => self.bootstrap_nominal_type(&module, "ParameterMode")?,
                Query::VariantPayloadKind => {
                    self.bootstrap_nominal_type(&module, "VariantPayloadKind")?
                }
                Query::FunctionVariadic | Query::FunctionSuspends | Query::FunctionUnsafe => {
                    self.interner.scalar(ScalarType::Bool)
                }
                Query::Kind => self.bootstrap_nominal_type(&module, "TypeKind")?,
                Query::GenericArguments | Query::TupleElements | Query::VariantTupleElements => {
                    self.interner.intrinsic(IntrinsicType::Array, vec![info])?
                }
                Query::Capabilities => {
                    let capability = self.bootstrap_nominal_type(&module, "TypeCapability")?;
                    self.interner
                        .intrinsic(IntrinsicType::Set, vec![capability])?
                }
                Query::Fields
                | Query::Variants
                | Query::VariantFields
                | Query::FunctionParameters => {
                    let kind = match operation {
                        Query::Fields | Query::VariantFields => Descriptor::FieldInfo,
                        Query::FunctionParameters => Descriptor::ParameterInfo,
                        _ => Descriptor::VariantInfo,
                    };
                    let element = self
                        .interner
                        .intrinsic(IntrinsicType::Reflection(kind), vec![])?;
                    self.interner
                        .intrinsic(IntrinsicType::Array, vec![element])?
                }
                Query::Function => {
                    let function = self
                        .interner
                        .intrinsic(IntrinsicType::Reflection(Descriptor::FunctionInfo), vec![])?;
                    self.interner.option(function)?
                }
                Query::TypeInfo => unreachable!("only receiver queries are listed"),
            };
            let receiver = self.interner.intrinsic(
                IntrinsicType::Reflection(operation.receiver().expect("query has a receiver")),
                vec![],
            )?;
            self.push_bootstrap_host_callable(
                span,
                HirBootstrapHostFunction::Reflection(operation),
                vec![(receiver, true)],
                None,
                outcome,
            )?;
        }
        Ok(())
    }
}
