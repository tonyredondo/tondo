//! Semantic closure construction shared by derive and manifest generators.

use std::collections::{BTreeMap, BTreeSet};

use crate::hir::{
    HirCallableId, HirField, HirNominalShape, HirProgram, HirTraitConstructor,
    HirTypeDeclarationKind, HirVariantPayload,
};
use crate::meta::{
    MetaEnvironment, MetaGenericParameter, MetaImplementation, MetaModule, MetaRoot, MetaSnapshot,
};
use crate::meta_frontend::{DeriveFrontendError, build_meta_declaration, declaration_syntax};
use crate::resolve::{ResolvedProgram, SymbolId, Visibility};
use crate::source::{FileId, SourceDatabase};
use crate::syntax::Parsed;
use crate::types::{TypeId, TypeKind};

#[path = "meta_builtins.rs"]
mod builtins;

pub(crate) struct SnapshotInput<'a> {
    pub packages: &'a crate::package::PackageGraph,
    pub owner: &'a crate::package::PackageId,
    pub sources: &'a SourceDatabase,
    pub parsed: &'a [(FileId, Parsed)],
    pub resolved: &'a ResolvedProgram,
    pub hir: &'a HirProgram,
    pub environment: &'a MetaEnvironment,
}

struct ValueDeclaration<'a> {
    model: crate::meta::MetaDeclaration,
    ty: TypeId,
    parameters: &'a [crate::hir::HirGenericParameter],
}

pub(crate) fn environment(request: &crate::driver::CompilationRequest) -> MetaEnvironment {
    MetaEnvironment {
        edition: request.edition().as_str().into(),
        target: request.target().name().into(),
        profile: request.profile().as_str().into(),
        capabilities: request
            .capabilities()
            .iter()
            .map(|value| value.as_str().into())
            .collect(),
        features: request
            .build_inputs()
            .features()
            .iter()
            .map(|value| value.as_str().into())
            .collect(),
        packages: Vec::new(),
    }
}

impl SnapshotInput<'_> {
    /// Only explicit roots and public referenced declarations enter the model.
    /// One derive target may expose private members; that grant never follows
    /// a private member's type to another private declaration.
    pub fn build(
        &self,
        roots: Vec<MetaRoot>,
        seeds: &[SymbolId],
        builtin_roots: &[String],
        bound_roots: &[crate::hir::HirTraitReference],
        private_target: Option<SymbolId>,
    ) -> Result<MetaSnapshot, DeriveFrontendError> {
        let explicit = seeds.iter().copied().collect::<BTreeSet<_>>();
        let local_package = private_target
            .and_then(|id| self.resolved.symbol(id))
            .map(|symbol| symbol.identity().package());
        let identities = self
            .resolved
            .symbols()
            .map(|symbol| (symbol.identity(), symbol.id()))
            .collect::<BTreeMap<_, _>>();
        let mut pending = seeds.to_vec();
        let mut pending_builtins = builtin_roots.iter().cloned().collect::<BTreeSet<_>>();
        for bound in bound_roots {
            self.enqueue_trait(bound, &identities, &mut pending, &mut pending_builtins);
            self.enqueue_types(
                self.hir.interner(),
                bound.arguments().to_vec(),
                &identities,
                &mut pending,
                &mut pending_builtins,
            )?;
        }
        let mut seen_builtins = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut declarations = Vec::new();
        let mut modules = BTreeSet::new();
        let mut admitted = BTreeSet::new();
        let mut environment = self.environment.clone();
        loop {
            let Some(id) = pending.pop() else {
                let Some(name) = pending_builtins.pop_first() else {
                    break;
                };
                if !seen_builtins.insert(name.clone()) {
                    continue;
                }
                let model = self.prelude_declaration(
                    &name,
                    &identities,
                    &mut pending,
                    &mut pending_builtins,
                )?;
                modules.insert(model.module().to_owned());
                environment
                    .packages
                    .push(self.packages.standard().as_str().to_owned());
                declarations.push(model);
                continue;
            };
            if !seen.insert(id) {
                continue;
            }
            let symbol = self
                .resolved
                .symbol(id)
                .ok_or_else(|| invalid("snapshot symbol is absent"))?;
            if !explicit.contains(&id) && symbol.visibility() != Visibility::Public {
                continue;
            }
            let module = if local_package == Some(symbol.identity().package()) {
                symbol.identity().module().as_str().to_owned()
            } else {
                format!(
                    "{}::{}",
                    symbol.identity().package(),
                    symbol.identity().module()
                )
            };
            let Some(declaration) = self.hir.declaration(id) else {
                if let Some(ValueDeclaration {
                    model,
                    ty,
                    parameters,
                }) = self.value_declaration(symbol, &module)?
                {
                    let mut types = vec![ty];
                    for parameter in parameters {
                        for bound in parameter.bounds() {
                            self.enqueue_trait(
                                bound,
                                &identities,
                                &mut pending,
                                &mut pending_builtins,
                            );
                            types.extend_from_slice(bound.arguments());
                        }
                    }
                    self.enqueue_types(
                        self.hir.interner(),
                        types,
                        &identities,
                        &mut pending,
                        &mut pending_builtins,
                    )?;
                    modules.insert(module);
                    admitted.insert(id);
                    environment
                        .packages
                        .push(symbol.identity().package().as_str().into());
                    declarations.push(model);
                }
                continue;
            };
            let private = private_target == Some(id);
            let mut types = Vec::new();
            for member in self.resolved.members().filter(|member| {
                member.owner() == crate::resolve::MemberOwner::Type(id)
                    && matches!(
                        member.kind(),
                        crate::resolve::MemberKind::InherentMethod
                            | crate::resolve::MemberKind::AssociatedFunction
                    )
                    && (private || member.visibility() == Visibility::Public)
                    && !member.is_synthetic()
            }) {
                let callable = self
                    .hir
                    .callable(HirCallableId::Member(member.id()))
                    .ok_or_else(|| invalid("snapshot member signature is absent"))?;
                let value = self.value_signature(
                    &format!("{}.{}", symbol.name(), member.name()),
                    &module,
                    callable.function_type(),
                    callable.generics(),
                    member.span(),
                    true,
                    member.visibility(),
                )?;
                types.push(value.ty);
                for parameter in value.parameters {
                    for bound in parameter.bounds() {
                        self.enqueue_trait(bound, &identities, &mut pending, &mut pending_builtins);
                        types.extend_from_slice(bound.arguments());
                    }
                }
                declarations.push(value.model);
            }
            for implementation in self.hir.implementations() {
                if !matches!(self.hir.interner().kind(implementation.target()),Ok(TypeKind::Nominal {identity,..}) if identity == symbol.identity())
                {
                    continue;
                }
                if let HirTraitConstructor::Symbol(trait_id) =
                    implementation.trait_reference().constructor()
                    && !self.resolved.symbol(*trait_id).is_some_and(|item| {
                        item.visibility() == Visibility::Public || explicit.contains(trait_id)
                    })
                {
                    continue;
                }
                self.enqueue_trait(
                    implementation.trait_reference(),
                    &identities,
                    &mut pending,
                    &mut pending_builtins,
                );
                types.extend_from_slice(implementation.trait_reference().arguments());
                for parameter in implementation.parameters() {
                    for bound in parameter.bounds() {
                        self.enqueue_trait(bound, &identities, &mut pending, &mut pending_builtins);
                        types.extend_from_slice(bound.arguments());
                    }
                }
            }
            for parameter in declaration.parameters() {
                for bound in parameter.bounds() {
                    self.enqueue_trait(bound, &identities, &mut pending, &mut pending_builtins);
                    types.extend_from_slice(bound.arguments());
                }
            }
            match declaration.kind() {
                HirTypeDeclarationKind::Alias { target } => types.push(*target),
                HirTypeDeclarationKind::Trait(definition) => {
                    for method in definition.methods() {
                        let signature =
                            self.hir
                                .callable(HirCallableId::Member(method.member()))
                                .ok_or_else(|| invalid("snapshot trait signature is absent"))?;
                        types.push(signature.function_type());
                        for parameter in signature.generics() {
                            for bound in parameter.bounds() {
                                self.enqueue_trait(
                                    bound,
                                    &identities,
                                    &mut pending,
                                    &mut pending_builtins,
                                );
                                types.extend_from_slice(bound.arguments());
                            }
                        }
                    }
                }
                HirTypeDeclarationKind::Nominal(definition) => match definition.shape() {
                    HirNominalShape::Newtype { underlying } => types.push(*underlying),
                    HirNominalShape::Record { fields } => {
                        self.field_types(fields, private, &mut types)
                    }
                    HirNominalShape::Enum { variants } => {
                        for variant in variants {
                            if !private
                                && self
                                    .resolved
                                    .member(variant.member())
                                    .is_some_and(|member| member.visibility() != Visibility::Public)
                            {
                                continue;
                            }
                            match variant.payload() {
                                HirVariantPayload::Unit => {}
                                HirVariantPayload::Tuple(items) => types.extend_from_slice(items),
                                HirVariantPayload::Record(fields) => {
                                    self.field_types(fields, private, &mut types)
                                }
                            }
                        }
                    }
                },
            }
            self.enqueue_types(
                self.hir.interner(),
                types,
                &identities,
                &mut pending,
                &mut pending_builtins,
            )?;
            let mut model = if symbol.is_synthetic() {
                self.builtin_declaration(symbol, declaration, &module)?
            } else {
                let syntax = declaration_syntax(self.parsed, declaration.span())
                    .ok_or_else(|| invalid("snapshot declaration syntax is absent"))?;
                let model = build_meta_declaration(
                    self.packages,
                    self.owner,
                    self.sources,
                    self.resolved,
                    self.hir,
                    symbol,
                    declaration,
                    syntax,
                    &module,
                )?;
                let docs = self
                    .parsed
                    .iter()
                    .find(|(file, _)| *file == declaration.span().file())
                    .and_then(|(_, parsed)| {
                        crate::resolve::leading_docs(self.sources, parsed, declaration.span())
                    });
                model
                    .with_docs(docs)
                    .map_err(|error| invalid(error.to_string()))?
            };
            if !private && !matches!(declaration.kind(), HirTypeDeclarationKind::Trait(_)) {
                model = model.public_view();
            }
            modules.insert(module);
            admitted.insert(id);
            environment
                .packages
                .push(symbol.identity().package().as_str().into());
            declarations.push(model);
        }
        let mut implementations = Vec::new();
        for implementation in self.hir.implementations() {
            let Ok(TypeKind::Nominal { identity, .. }) =
                self.hir.interner().kind(implementation.target())
            else {
                continue;
            };
            if !identities
                .get(identity)
                .is_some_and(|id| admitted.contains(id))
            {
                continue;
            }
            let trait_reference = implementation.trait_reference();
            if let HirTraitConstructor::Symbol(id) = trait_reference.constructor()
                && !admitted.contains(id)
            {
                continue;
            }
            let signature = |ty| {
                self.hir
                    .interner()
                    .canonical_interface(ty)
                    .map_err(|error| invalid(error.to_string()))
            };
            let parameters = implementation
                .parameters()
                .iter()
                .map(|parameter| {
                    let name = self
                        .resolved
                        .local(parameter.local())
                        .ok_or_else(|| invalid("implementation binder is absent"))?
                        .name()
                        .as_str();
                    MetaGenericParameter::new(
                        name,
                        parameter
                            .bounds()
                            .iter()
                            .map(|bound| self.trait_reference(bound))
                            .collect::<Result<Vec<_>, _>>()?,
                    )
                    .map_err(|error| invalid(error.to_string()))
                })
                .collect::<Result<Vec<_>, DeriveFrontendError>>()?;
            implementations.push(MetaImplementation {
                module: format!(
                    "{}::{}",
                    implementation.module().package(),
                    implementation.module().path()
                ),
                target: signature(implementation.target())?,
                trait_identity: self.trait_constructor(trait_reference.constructor())?,
                arguments: trait_reference
                    .arguments()
                    .iter()
                    .map(|ty| signature(*ty))
                    .collect::<Result<Vec<_>, _>>()?,
                generic_parameters: parameters,
                origin: implementation.span().into(),
                docs: self
                    .parsed
                    .iter()
                    .find(|(file, _)| *file == implementation.span().file())
                    .and_then(|(_, parsed)| {
                        crate::resolve::leading_docs(self.sources, parsed, implementation.span())
                    }),
            });
        }
        MetaSnapshot::new(
            environment,
            roots,
            modules
                .into_iter()
                .map(|module| MetaModule::new(module, None::<String>))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| invalid(error.to_string()))?,
            declarations,
        )
        .and_then(|snapshot| snapshot.with_implementations(implementations))
        .map_err(|error| invalid(error.to_string()))
    }

    fn builtin_declaration(
        &self,
        symbol: &crate::resolve::Symbol,
        declaration: &crate::hir::HirTypeDeclaration,
        module: &str,
    ) -> Result<crate::meta::MetaDeclaration, DeriveFrontendError> {
        use crate::meta::{
            MetaDeclaration, MetaDeclarationKind, MetaField, MetaOrigin, MetaVariant,
            MetaVariantPayload, MetaVisibility,
        };
        let names =
            crate::meta_binders::names(self.hir, self.resolved, declaration.parameters(), None)
                .map_err(invalid)?;
        let generics =
            crate::meta_binders::parameters(self.hir, self.resolved, declaration.parameters())
                .map_err(invalid)?;
        let bounds = crate::meta_binders::bounds(&generics).map_err(invalid)?;
        let type_ref = |ty| {
            crate::meta_type::MetaTypeRef::resolved(
                self.hir,
                self.resolved,
                self.packages,
                self.owner,
                ty,
                self.hir
                    .interner()
                    .canonical_interface(ty)
                    .map_err(|error| invalid(error.to_string()))?,
                &names,
            )
            .map_err(|error| invalid(error.to_string()))
        };
        let member = |id| {
            self.resolved
                .member(id)
                .ok_or_else(|| invalid("builtin structural member is absent"))
        };
        let origin = |name: &str| {
            MetaOrigin::Builtin(format!("{}.{name}", symbol.identity().canonical_name()))
        };
        let fields = |fields: &[HirField]| {
            fields
                .iter()
                .enumerate()
                .map(|(ordinal, field)| {
                    let field_member = member(field.member())?;
                    MetaField::new(
                        field_member.name().as_str(),
                        type_ref(field.ty())?,
                        if field_member.visibility() == Visibility::Public {
                            MetaVisibility::Public
                        } else {
                            MetaVisibility::Private
                        },
                        ordinal as u32,
                        origin(field_member.name().as_str()),
                        field_member.docs(),
                    )
                    .map_err(|error| invalid(error.to_string()))
                })
                .collect::<Result<Vec<_>, DeriveFrontendError>>()
        };
        let kind = match declaration.kind() {
            HirTypeDeclarationKind::Alias { target } => {
                MetaDeclarationKind::Alias(type_ref(*target)?)
            }
            HirTypeDeclarationKind::Nominal(definition) => match definition.shape() {
                HirNominalShape::Newtype { underlying } => {
                    MetaDeclarationKind::Newtype(type_ref(*underlying)?)
                }
                HirNominalShape::Record { fields: items } => {
                    MetaDeclarationKind::Record(fields(items)?)
                }
                HirNominalShape::Enum { variants } => MetaDeclarationKind::Enum(
                    variants
                        .iter()
                        .enumerate()
                        .map(|(ordinal, variant)| {
                            let variant_member = member(variant.member())?;
                            let payload = match variant.payload() {
                                HirVariantPayload::Unit => MetaVariantPayload::Unit,
                                HirVariantPayload::Tuple(types) => MetaVariantPayload::Tuple(
                                    types
                                        .iter()
                                        .map(|ty| type_ref(*ty))
                                        .collect::<Result<Vec<_>, _>>()?,
                                ),
                                HirVariantPayload::Record(items) => {
                                    MetaVariantPayload::Record(fields(items)?)
                                }
                            };
                            MetaVariant::new(
                                variant_member.name().as_str(),
                                payload,
                                ordinal as u32,
                                origin(variant_member.name().as_str()),
                                variant_member.docs(),
                            )
                            .map_err(|error| invalid(error.to_string()))
                        })
                        .collect::<Result<Vec<_>, DeriveFrontendError>>()?,
                ),
            },
            HirTypeDeclarationKind::Trait(_) => {
                return Err(invalid("unexpected synthetic nominal trait"));
            }
        };
        MetaDeclaration::new(
            symbol.name().as_str(),
            module,
            MetaVisibility::Public,
            generics,
            bounds,
            MetaOrigin::Builtin(symbol.identity().canonical_name()),
            None::<String>,
            kind,
        )
        .map_err(|error| invalid(error.to_string()))
    }

    fn enqueue_trait(
        &self,
        reference: &crate::hir::HirTraitReference,
        identities: &BTreeMap<&crate::package::SymbolIdentity, SymbolId>,
        pending: &mut Vec<SymbolId>,
        pending_builtins: &mut BTreeSet<String>,
    ) {
        match reference.constructor() {
            HirTraitConstructor::Symbol(id) => pending.push(*id),
            HirTraitConstructor::External(identity) => {
                if let Some(id) = identities.get(identity) {
                    pending.push(*id);
                }
            }
            HirTraitConstructor::Prelude(name) => {
                pending_builtins.insert(name.as_str().to_owned());
            }
        }
    }

    fn enqueue_types(
        &self,
        interner: &crate::types::TypeInterner,
        mut types: Vec<TypeId>,
        identities: &BTreeMap<&crate::package::SymbolIdentity, SymbolId>,
        pending: &mut Vec<SymbolId>,
        pending_builtins: &mut BTreeSet<String>,
    ) -> Result<(), DeriveFrontendError> {
        let mut visited_types = BTreeSet::new();
        while let Some(ty) = types.pop() {
            if !visited_types.insert(ty) {
                continue;
            }
            match interner
                .kind(ty)
                .map_err(|error| invalid(error.to_string()))?
            {
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => {
                    if let Some(symbol) = identities.get(identity) {
                        pending.push(*symbol);
                    }
                    types.extend_from_slice(arguments);
                }
                TypeKind::OpaqueResult {
                    identity,
                    arguments,
                } => {
                    types.extend_from_slice(arguments);
                    if let Some(result) = self.hir.opaque_result(identity) {
                        for bound in result.bounds() {
                            self.enqueue_trait(bound, identities, pending, pending_builtins);
                            types.extend_from_slice(bound.arguments());
                        }
                    }
                }
                TypeKind::Tuple(items)
                | TypeKind::Union(items)
                | TypeKind::Intrinsic {
                    arguments: items, ..
                }
                | TypeKind::Generated {
                    arguments: items, ..
                } => types.extend_from_slice(items),
                TypeKind::Function(function) => {
                    types.push(function.outcome());
                    types.extend(function.variadic());
                    types.extend(function.parameters().iter().map(|parameter| parameter.ty()));
                }
                TypeKind::Option(item) => types.push(*item),
                TypeKind::Result { success, error } => types.extend([*success, *error]),
                TypeKind::Cursor { collection, .. } => types.push(*collection),
                TypeKind::Error
                | TypeKind::Scalar(_)
                | TypeKind::GenericParameter(_)
                | TypeKind::Inference(_) => {}
            }
        }
        Ok(())
    }

    fn value_declaration<'a>(
        &'a self,
        symbol: &crate::resolve::Symbol,
        module: &str,
    ) -> Result<Option<ValueDeclaration<'a>>, DeriveFrontendError> {
        let (ty, parameters, span, function) =
            if let Some(callable) = self.hir.callable(HirCallableId::Symbol(symbol.id())) {
                (
                    callable.function_type(),
                    callable.generics(),
                    callable.span(),
                    true,
                )
            } else if let Some(constant) = self.hir.constant(symbol.id()) {
                (
                    constant
                        .declared_type()
                        .ok_or_else(|| invalid("public constant has no declared type"))?,
                    &[][..],
                    constant.span(),
                    false,
                )
            } else {
                return Ok(None);
            };
        self.value_signature(
            symbol.name().as_str(),
            module,
            ty,
            parameters,
            span,
            function,
            Visibility::Public,
        )
        .map(Some)
    }

    #[allow(clippy::too_many_arguments)]
    fn value_signature<'a>(
        &self,
        name: &str,
        module: &str,
        ty: TypeId,
        parameters: &'a [crate::hir::HirGenericParameter],
        span: crate::source::Span,
        function: bool,
        visibility: Visibility,
    ) -> Result<ValueDeclaration<'a>, DeriveFrontendError> {
        use crate::meta::{MetaDeclaration, MetaDeclarationKind, MetaVisibility};
        let names = crate::meta_binders::names(self.hir, self.resolved, parameters, None)
            .map_err(invalid)?;
        let generics = crate::meta_binders::parameters(self.hir, self.resolved, parameters)
            .map_err(invalid)?;
        let bounds = crate::meta_binders::bounds(&generics).map_err(invalid)?;
        let reference = crate::meta_type::MetaTypeRef::resolved(
            self.hir,
            self.resolved,
            self.packages,
            self.owner,
            ty,
            self.hir
                .interner()
                .canonical_interface(ty)
                .map_err(|error| invalid(error.to_string()))?,
            &names,
        )
        .map_err(|error| invalid(error.to_string()))?;
        let docs = self
            .parsed
            .iter()
            .find(|(file, _)| *file == span.file())
            .and_then(|(_, parsed)| crate::resolve::leading_docs(self.sources, parsed, span));
        let model = MetaDeclaration::new(
            name,
            module,
            match visibility {
                Visibility::Public => MetaVisibility::Public,
                Visibility::Private => MetaVisibility::Private,
            },
            generics,
            bounds,
            span,
            docs,
            if function {
                MetaDeclarationKind::Function(reference)
            } else {
                MetaDeclarationKind::Constant(reference)
            },
        )
        .map_err(|error| invalid(error.to_string()))?;
        Ok(ValueDeclaration {
            model,
            ty,
            parameters,
        })
    }

    fn trait_constructor(
        &self,
        constructor: &HirTraitConstructor,
    ) -> Result<String, DeriveFrontendError> {
        Ok(match constructor {
            HirTraitConstructor::Symbol(id) => self
                .resolved
                .symbol(*id)
                .ok_or_else(|| invalid("implementation trait is absent"))?
                .identity()
                .canonical_name(),
            HirTraitConstructor::Prelude(name) => name.as_str().into(),
            HirTraitConstructor::External(identity) => identity.canonical_name(),
        })
    }

    fn trait_reference(
        &self,
        reference: &crate::hir::HirTraitReference,
    ) -> Result<String, DeriveFrontendError> {
        let name = self.trait_constructor(reference.constructor())?;
        if reference.arguments().is_empty() {
            return Ok(name);
        }
        let arguments = reference
            .arguments()
            .iter()
            .map(|ty| {
                self.hir
                    .interner()
                    .canonical_interface(*ty)
                    .map_err(|error| invalid(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(format!("{name}[{}]", arguments.join(", ")))
    }

    fn field_types(&self, fields: &[HirField], private: bool, output: &mut Vec<TypeId>) {
        output.extend(
            fields
                .iter()
                .filter(|field| {
                    private
                        || self
                            .resolved
                            .member(field.member())
                            .is_some_and(|member| member.visibility() == Visibility::Public)
                })
                .map(HirField::ty),
        );
    }
}

fn invalid(message: impl Into<String>) -> DeriveFrontendError {
    DeriveFrontendError::Invariant(message.into())
}
