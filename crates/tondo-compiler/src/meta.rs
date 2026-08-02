//! Static validation shared by derive, reflection, and the meta target.
//!
//! This module deliberately stops at the semantic boundary. It resolves the
//! exact target/trait/provider identities and produces a deterministic plan;
//! executing a provider and constructing its immutable model belong to later
//! meta phases.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hir::HirProgram;
use crate::source::Span;

/// The only meta model accepted by the Tondo 0.1 toolchain.
pub const META_MODEL: &str = "tondo-meta-model-0.1/1";

/// A source position included in the immutable meta snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaSpan {
    file: u32,
    start: u32,
    end: u32,
}

impl MetaSpan {
    pub fn new(file: u32, start: u32, end: u32) -> Result<Self, MetaModelError> {
        if start > end {
            return Err(MetaModelError::InvalidSpan { start, end });
        }
        Ok(Self { file, start, end })
    }

    pub fn file(&self) -> u32 {
        self.file
    }

    pub fn start(&self) -> u32 {
        self.start
    }

    pub fn end(&self) -> u32 {
        self.end
    }
}

impl From<Span> for MetaSpan {
    fn from(span: Span) -> Self {
        Self {
            file: span.file().index(),
            start: span.range().start(),
            end: span.range().end(),
        }
    }
}

/// A root explicitly authorized by a generator or implicitly authorized by derive.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaRoot {
    package: String,
    module: String,
}

impl MetaRoot {
    pub fn new(
        package: impl Into<String>,
        module: impl Into<String>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            package: required_text("root.package", package.into())?,
            module: required_text("root.module", module.into())?,
        })
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn module(&self) -> &str {
        &self.module
    }
}

/// A module in the authorized semantic closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaModule {
    identity: String,
    docs: Option<String>,
}

impl MetaModule {
    pub fn new(
        identity: impl Into<String>,
        docs: Option<impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            identity: required_text("module.identity", identity.into())?,
            docs: canonical_docs(docs.map(Into::into))?,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }
}

/// Visibility exposed by the snapshot. A derive target may be private, while
/// its requested view is still restricted to that exact target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MetaVisibility {
    Public,
    Private,
}

/// A generic parameter and its canonical positive bounds.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaGenericParameter {
    name: String,
    bounds: Vec<String>,
}

impl MetaGenericParameter {
    pub fn new(
        name: impl Into<String>,
        bounds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        let mut parameter = Self {
            name: required_text("generic_parameter.name", name.into())?,
            bounds: bounds.into_iter().map(Into::into).collect(),
        };
        parameter.canonicalize()?;
        Ok(parameter)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn bounds(&self) -> &[String] {
        &self.bounds
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        for bound in &self.bounds {
            required_text("generic_parameter.bound", bound.clone())?;
        }
        self.bounds.sort();
        self.bounds.dedup();
        Ok(())
    }
}

/// A type bound retained in a declaration's semantic closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaBound {
    binder: String,
    trait_identity: String,
}

impl MetaBound {
    pub fn new(
        binder: impl Into<String>,
        trait_identity: impl Into<String>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            binder: required_text("bound.binder", binder.into())?,
            trait_identity: required_text("bound.trait", trait_identity.into())?,
        })
    }

    pub fn binder(&self) -> &str {
        &self.binder
    }

    pub fn trait_identity(&self) -> &str {
        &self.trait_identity
    }
}

/// A public or target-authorized field. `ordinal` preserves source order while
/// allowing canonical serialization independent of insertion order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaField {
    name: String,
    ty: String,
    visibility: MetaVisibility,
    ordinal: u32,
    span: MetaSpan,
    docs: Option<String>,
}

impl MetaField {
    pub fn new(
        name: impl Into<String>,
        ty: impl Into<String>,
        visibility: MetaVisibility,
        ordinal: u32,
        span: MetaSpan,
        docs: Option<impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            name: required_text("field.name", name.into())?,
            ty: required_text("field.type", ty.into())?,
            visibility,
            ordinal,
            span,
            docs: canonical_docs(docs.map(Into::into))?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ty(&self) -> &str {
        &self.ty
    }

    pub fn visibility(&self) -> MetaVisibility {
        self.visibility
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn span(&self) -> MetaSpan {
        self.span
    }

    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }
}

/// The payload shape of an enum variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetaVariantPayload {
    Unit,
    Tuple(Vec<String>),
    Record(Vec<MetaField>),
}

impl MetaVariantPayload {
    pub fn unit() -> Self {
        Self::Unit
    }

    pub fn tuple(types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Tuple(types.into_iter().map(Into::into).collect())
    }

    pub fn record(fields: impl IntoIterator<Item = MetaField>) -> Self {
        Self::Record(fields.into_iter().collect())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Tuple(_) => "tuple",
            Self::Record(_) => "record",
        }
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        match self {
            Self::Unit => Ok(()),
            Self::Tuple(types) => {
                for ty in types {
                    required_text("variant.type", ty.clone())?;
                }
                Ok(())
            }
            Self::Record(fields) => canonicalize_fields(fields),
        }
    }
}

/// An enum variant in the semantic closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaVariant {
    name: String,
    payload: MetaVariantPayload,
    ordinal: u32,
    span: MetaSpan,
    docs: Option<String>,
}

impl MetaVariant {
    pub fn new(
        name: impl Into<String>,
        payload: MetaVariantPayload,
        ordinal: u32,
        span: MetaSpan,
        docs: Option<impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            name: required_text("variant.name", name.into())?,
            payload,
            ordinal,
            span,
            docs: canonical_docs(docs.map(Into::into))?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn payload(&self) -> &MetaVariantPayload {
        &self.payload
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn span(&self) -> MetaSpan {
        self.span
    }

    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        required_text("variant.name", self.name.clone())?;
        self.payload.canonicalize()
    }
}

/// A trait operation retained without executable bodies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaOperation {
    name: String,
    signature: String,
    visibility: MetaVisibility,
    ordinal: u32,
    span: MetaSpan,
    docs: Option<String>,
}

impl MetaOperation {
    pub fn new(
        name: impl Into<String>,
        signature: impl Into<String>,
        visibility: MetaVisibility,
        ordinal: u32,
        span: MetaSpan,
        docs: Option<impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        Ok(Self {
            name: required_text("operation.name", name.into())?,
            signature: required_text("operation.signature", signature.into())?,
            visibility,
            ordinal,
            span,
            docs: canonical_docs(docs.map(Into::into))?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn visibility(&self) -> MetaVisibility {
        self.visibility
    }

    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn span(&self) -> MetaSpan {
        self.span
    }

    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }
}

/// The declaration shapes visible to a meta program. No variant contains a
/// body, value, layout, address, or garbage-collector state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetaDeclarationKind {
    Record(Vec<MetaField>),
    Enum(Vec<MetaVariant>),
    Newtype(String),
    Trait(Vec<MetaOperation>),
}

impl MetaDeclarationKind {
    pub fn record(fields: impl IntoIterator<Item = MetaField>) -> Self {
        Self::Record(fields.into_iter().collect())
    }

    pub fn enumeration(variants: impl IntoIterator<Item = MetaVariant>) -> Self {
        Self::Enum(variants.into_iter().collect())
    }

    pub fn newtype(underlying: impl Into<String>) -> Self {
        Self::Newtype(underlying.into())
    }

    pub fn trait_definition(operations: impl IntoIterator<Item = MetaOperation>) -> Self {
        Self::Trait(operations.into_iter().collect())
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Record(_) => "record",
            Self::Enum(_) => "enum",
            Self::Newtype(_) => "newtype",
            Self::Trait(_) => "trait",
        }
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        match self {
            Self::Record(fields) => canonicalize_fields(fields),
            Self::Enum(variants) => {
                for variant in variants.iter_mut() {
                    variant.canonicalize()?;
                }
                variants.sort_by_key(|variant| (variant.ordinal, variant.name.clone()));
                ensure_unique_ordinals_and_names(
                    variants
                        .iter()
                        .map(|variant| (variant.ordinal, variant.name.as_str())),
                    "variant",
                )
            }
            Self::Newtype(underlying) => {
                required_text("newtype.underlying", underlying.clone())?;
                Ok(())
            }
            Self::Trait(operations) => {
                for operation in operations.iter() {
                    required_text("operation.name", operation.name.clone())?;
                    required_text("operation.signature", operation.signature.clone())?;
                }
                operations.sort_by_key(|operation| (operation.ordinal, operation.name.clone()));
                ensure_unique_ordinals_and_names(
                    operations
                        .iter()
                        .map(|operation| (operation.ordinal, operation.name.as_str())),
                    "operation",
                )
            }
        }
    }
}

/// One declaration in the authorized semantic closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaDeclaration {
    identity: String,
    module: String,
    visibility: MetaVisibility,
    generic_parameters: Vec<MetaGenericParameter>,
    bounds: Vec<MetaBound>,
    span: MetaSpan,
    docs: Option<String>,
    kind: MetaDeclarationKind,
}

impl MetaDeclaration {
    pub fn new(
        identity: impl Into<String>,
        module: impl Into<String>,
        visibility: MetaVisibility,
        generic_parameters: impl IntoIterator<Item = MetaGenericParameter>,
        bounds: impl IntoIterator<Item = MetaBound>,
        span: MetaSpan,
        docs: Option<impl Into<String>>,
        kind: MetaDeclarationKind,
    ) -> Result<Self, MetaModelError> {
        let mut declaration = Self {
            identity: required_text("declaration.identity", identity.into())?,
            module: required_text("declaration.module", module.into())?,
            visibility,
            generic_parameters: generic_parameters.into_iter().collect(),
            bounds: bounds.into_iter().collect(),
            span,
            docs: canonical_docs(docs.map(Into::into))?,
            kind,
        };
        declaration.canonicalize()?;
        Ok(declaration)
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn visibility(&self) -> MetaVisibility {
        self.visibility
    }

    pub fn generic_parameters(&self) -> &[MetaGenericParameter] {
        &self.generic_parameters
    }

    pub fn bounds(&self) -> &[MetaBound] {
        &self.bounds
    }

    pub fn span(&self) -> MetaSpan {
        self.span
    }

    pub fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }

    pub fn kind(&self) -> &MetaDeclarationKind {
        &self.kind
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        required_text("declaration.identity", self.identity.clone())?;
        required_text("declaration.module", self.module.clone())?;
        for parameter in &mut self.generic_parameters {
            parameter.canonicalize()?;
        }
        let mut parameter_names = BTreeSet::new();
        for parameter in &self.generic_parameters {
            if !parameter_names.insert(parameter.name.as_str()) {
                return Err(MetaModelError::Duplicate {
                    kind: "generic parameter".into(),
                    identity: parameter.name.clone(),
                });
            }
        }
        for bound in &self.bounds {
            required_text("bound.binder", bound.binder.clone())?;
            required_text("bound.trait", bound.trait_identity.clone())?;
        }
        self.bounds.sort();
        self.bounds.dedup();
        self.kind.canonicalize()
    }
}

/// A deterministic, immutable snapshot of one authorized semantic closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaSnapshot {
    format: String,
    roots: Vec<MetaRoot>,
    modules: Vec<MetaModule>,
    declarations: Vec<MetaDeclaration>,
}

impl MetaSnapshot {
    pub fn new(
        roots: impl IntoIterator<Item = MetaRoot>,
        modules: impl IntoIterator<Item = MetaModule>,
        declarations: impl IntoIterator<Item = MetaDeclaration>,
    ) -> Result<Self, MetaModelError> {
        Self {
            format: META_MODEL.into(),
            roots: roots.into_iter().collect(),
            modules: modules.into_iter().collect(),
            declarations: declarations.into_iter().collect(),
        }
        .canonicalize()
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn roots(&self) -> &[MetaRoot] {
        &self.roots
    }

    pub fn modules(&self) -> &[MetaModule] {
        &self.modules
    }

    pub fn declarations(&self) -> &[MetaDeclaration] {
        &self.declarations
    }

    /// Serialize this snapshot as canonical UTF-8 JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MetaModelError> {
        let canonical = self.clone().canonicalize()?;
        serde_json::to_vec(&canonical)
            .map_err(|error| MetaModelError::Serialization(error.to_string()))
    }

    pub fn hash(&self) -> Result<String, MetaModelError> {
        Ok(crate::artifact::sha256(&self.canonical_bytes()?))
    }

    /// Decode only canonical encodings. This rejects reordered or extended
    /// snapshots before they can enter the meta execution boundary.
    pub fn decode(bytes: &[u8]) -> Result<Self, MetaModelError> {
        let snapshot: Self = serde_json::from_slice(bytes)
            .map_err(|error| MetaModelError::Serialization(error.to_string()))?;
        let canonical = snapshot.clone().canonicalize()?;
        let canonical_bytes = serde_json::to_vec(&canonical)
            .map_err(|error| MetaModelError::Serialization(error.to_string()))?;
        if bytes != canonical_bytes {
            return Err(MetaModelError::NonCanonicalEncoding);
        }
        Ok(canonical)
    }

    fn canonicalize(mut self) -> Result<Self, MetaModelError> {
        if self.format != META_MODEL {
            return Err(MetaModelError::UnsupportedFormat(self.format));
        }

        self.roots
            .sort_by_key(|root| (root.package.clone(), root.module.clone()));
        ensure_unique_keys(
            self.roots
                .iter()
                .map(|root| format!("{}::{}", root.package, root.module)),
            "root",
        )?;

        self.modules.sort_by_key(|module| module.identity.clone());
        ensure_unique_keys(
            self.modules.iter().map(|module| module.identity.clone()),
            "module",
        )?;
        for module in &mut self.modules {
            module.identity = required_text("module.identity", module.identity.clone())?;
            module.docs = canonical_docs(module.docs.take())?;
        }

        for declaration in &mut self.declarations {
            declaration.canonicalize()?;
        }
        self.declarations
            .sort_by_key(|declaration| (declaration.module.clone(), declaration.identity.clone()));
        ensure_unique_keys(
            self.declarations
                .iter()
                .map(|declaration| format!("{}::{}", declaration.module, declaration.identity)),
            "declaration",
        )?;
        Ok(self)
    }
}

/// Errors found while constructing or decoding the immutable meta model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaModelError {
    InvalidValue { field: String, reason: String },
    InvalidSpan { start: u32, end: u32 },
    Duplicate { kind: String, identity: String },
    UnsupportedFormat(String),
    NonCanonicalEncoding,
    Serialization(String),
}

impl fmt::Display for MetaModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::InvalidSpan { start, end } => {
                write!(formatter, "invalid meta span {start}..{end}")
            }
            Self::Duplicate { kind, identity } => {
                write!(formatter, "duplicate {kind} `{identity}`")
            }
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported meta model `{format}`")
            }
            Self::NonCanonicalEncoding => formatter.write_str("meta snapshot is not canonical"),
            Self::Serialization(error) => {
                write!(formatter, "meta snapshot encoding failed: {error}")
            }
        }
    }
}

impl Error for MetaModelError {}

fn required_text(field: &str, value: String) -> Result<String, MetaModelError> {
    if value.is_empty() {
        return Err(MetaModelError::InvalidValue {
            field: field.into(),
            reason: "must not be empty".into(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(MetaModelError::InvalidValue {
            field: field.into(),
            reason: "must not contain control characters".into(),
        });
    }
    Ok(value)
}

fn canonical_docs(docs: Option<String>) -> Result<Option<String>, MetaModelError> {
    match docs {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => Ok(Some(required_text("docs", value)?)),
        None => Ok(None),
    }
}

fn canonicalize_fields(fields: &mut Vec<MetaField>) -> Result<(), MetaModelError> {
    for field in fields.iter() {
        required_text("field.name", field.name.clone())?;
        required_text("field.type", field.ty.clone())?;
    }
    fields.sort_by_key(|field| (field.ordinal, field.name.clone()));
    ensure_unique_ordinals_and_names(
        fields
            .iter()
            .map(|field| (field.ordinal, field.name.as_str())),
        "field",
    )
}

fn ensure_unique_ordinals_and_names<'a>(
    values: impl Iterator<Item = (u32, &'a str)>,
    kind: &str,
) -> Result<(), MetaModelError> {
    let mut ordinals = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (ordinal, name) in values {
        if !ordinals.insert(ordinal) || !names.insert(name) {
            return Err(MetaModelError::Duplicate {
                kind: kind.into(),
                identity: name.into(),
            });
        }
    }
    Ok(())
}

fn ensure_unique_keys(
    values: impl Iterator<Item = String>,
    kind: &str,
) -> Result<(), MetaModelError> {
    let mut seen = BTreeSet::new();
    for identity in values {
        if !seen.insert(identity.clone()) {
            return Err(MetaModelError::Duplicate {
                kind: kind.into(),
                identity,
            });
        }
    }
    Ok(())
}

/// Nominal shapes that can authorize a derive request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeriveTargetKind {
    Record,
    Enum,
    Newtype,
    Alias,
    Other,
}

impl DeriveTargetKind {
    fn is_derivable(self) -> bool {
        matches!(self, Self::Record | Self::Enum | Self::Newtype)
    }
}

/// Metadata needed to authorize a target without exposing its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveTarget {
    identity: String,
    module: String,
    generic_parameters: Vec<String>,
    kind: DeriveTargetKind,
}

impl DeriveTarget {
    pub fn new(
        identity: impl Into<String>,
        module: impl Into<String>,
        generic_parameters: impl IntoIterator<Item = impl Into<String>>,
        kind: DeriveTargetKind,
    ) -> Self {
        Self {
            identity: identity.into(),
            module: module.into(),
            generic_parameters: generic_parameters.into_iter().map(Into::into).collect(),
            kind,
        }
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn generic_parameters(&self) -> &[String] {
        &self.generic_parameters
    }

    pub fn kind(&self) -> DeriveTargetKind {
        self.kind
    }
}

/// A provider selected by the exact nominal identity of a trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveProvider {
    trait_identity: String,
    provider_identity: String,
    introduced_bounds: Vec<String>,
}

impl DeriveProvider {
    pub fn new(
        trait_identity: impl Into<String>,
        provider_identity: impl Into<String>,
        introduced_bounds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut bounds = introduced_bounds
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        bounds.sort();
        bounds.dedup();
        Self {
            trait_identity: trait_identity.into(),
            provider_identity: provider_identity.into(),
            introduced_bounds: bounds,
        }
    }

    pub fn trait_identity(&self) -> &str {
        &self.trait_identity
    }

    pub fn provider_identity(&self) -> &str {
        &self.provider_identity
    }

    pub fn introduced_bounds(&self) -> &[String] {
        &self.introduced_bounds
    }
}

/// A lossless semantic input derived from one HIR `derive` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveRequest {
    module: String,
    target: String,
    generic_parameters: Vec<String>,
    traits: Vec<String>,
    span: Option<Span>,
}

impl DeriveRequest {
    pub fn new(
        module: impl Into<String>,
        target: impl Into<String>,
        generic_parameters: impl IntoIterator<Item = impl Into<String>>,
        traits: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            module: module.into(),
            target: target.into(),
            generic_parameters: generic_parameters.into_iter().map(Into::into).collect(),
            traits: traits.into_iter().map(Into::into).collect(),
            span: None,
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn generic_parameters(&self) -> &[String] {
        &self.generic_parameters
    }

    pub fn traits(&self) -> &[String] {
        &self.traits
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }

    pub fn from_hir(module: impl Into<String>, request: &crate::hir::HirDeriveRequest) -> Self {
        Self::new(
            module,
            request.target(),
            request.generic_parameters().iter().cloned(),
            request.traits().iter().cloned(),
        )
        .with_span(request.span())
    }
}

/// The semantic universe visible to derive validation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeriveContext {
    module: String,
    targets: BTreeMap<String, DeriveTarget>,
    traits: BTreeSet<String>,
    providers: BTreeMap<String, DeriveProvider>,
    manual_implementations: BTreeSet<(String, String)>,
}

impl DeriveContext {
    pub fn new(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            ..Self::default()
        }
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn add_target(&mut self, target: DeriveTarget) {
        self.targets.insert(target.identity.clone(), target);
    }

    pub fn add_trait(&mut self, identity: impl Into<String>) {
        self.traits.insert(identity.into());
    }

    pub fn add_provider(&mut self, provider: DeriveProvider) {
        self.providers
            .insert(provider.trait_identity.clone(), provider);
    }

    pub fn add_manual_implementation(
        &mut self,
        trait_identity: impl Into<String>,
        target_identity: impl Into<String>,
    ) {
        self.manual_implementations
            .insert((trait_identity.into(), target_identity.into()));
    }
}

/// A provider that passed all semantic checks for one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDerive {
    request_index: usize,
    target: DeriveTarget,
    traits: Vec<ValidatedTrait>,
}

impl ValidatedDerive {
    pub fn request_index(&self) -> usize {
        self.request_index
    }

    pub fn target(&self) -> &DeriveTarget {
        &self.target
    }

    pub fn traits(&self) -> &[ValidatedTrait] {
        &self.traits
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTrait {
    identity: String,
    provider: String,
    introduced_bounds: Vec<String>,
}

impl ValidatedTrait {
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn introduced_bounds(&self) -> &[String] {
        &self.introduced_bounds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetaDiagnosticCode {
    InvalidDeriveTarget,
    MissingDeriveProvider,
    InvalidDeriveRequest,
    CoherenceConflict,
}

impl MetaDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDeriveTarget => "E2101",
            Self::MissingDeriveProvider => "E2102",
            Self::InvalidDeriveRequest => "E2103",
            Self::CoherenceConflict => "E1111",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaDiagnostic {
    code: MetaDiagnosticCode,
    request_index: usize,
    trait_identity: Option<String>,
    message: String,
    span: Option<Span>,
}

impl MetaDiagnostic {
    pub fn code(&self) -> MetaDiagnosticCode {
        self.code
    }

    pub fn request_index(&self) -> usize {
        self.request_index
    }

    pub fn trait_identity(&self) -> Option<&str> {
        self.trait_identity.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn span(&self) -> Option<Span> {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaSemanticError {
    diagnostics: Vec<MetaDiagnostic>,
}

impl MetaSemanticError {
    pub fn diagnostics(&self) -> &[MetaDiagnostic] {
        &self.diagnostics
    }
}

impl fmt::Display for MetaSemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} derive diagnostic(s)", self.diagnostics.len())
    }
}

impl Error for MetaSemanticError {}

/// Validate every request and return an all-or-nothing deterministic plan.
pub fn validate_derive_requests(
    requests: &[DeriveRequest],
    context: &DeriveContext,
) -> Result<Vec<ValidatedDerive>, MetaSemanticError> {
    let mut diagnostics = Vec::new();
    let mut seen_pairs = BTreeSet::new();
    let mut validated = Vec::new();

    for (request_index, request) in requests.iter().enumerate() {
        let Some(target) = context.targets.get(request.target()) else {
            diagnostics.push(invalid_target(
                request_index,
                request,
                "target is not nominal",
            ));
            continue;
        };
        if request.module() != context.module()
            || target.module() != context.module()
            || !target.kind().is_derivable()
            || target.generic_parameters() != request.generic_parameters()
            || !unique_binders(request.generic_parameters())
        {
            diagnostics.push(invalid_target(
                request_index,
                request,
                "target ownership, shape, or generic binders are invalid",
            ));
            continue;
        }

        let mut request_traits = BTreeSet::new();
        let mut output_traits = Vec::new();
        if request.traits().is_empty() {
            diagnostics.push(invalid_request(
                request_index,
                request,
                None,
                "derive must request at least one trait",
            ));
        }
        for trait_identity in request.traits() {
            if !request_traits.insert(trait_identity.clone()) {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "a trait may appear only once in a derive request",
                ));
                continue;
            }
            if !context.traits.contains(trait_identity) {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "the requested identity is not a trait",
                ));
                continue;
            }
            let Some(provider) = context.providers.get(trait_identity) else {
                diagnostics.push(MetaDiagnostic {
                    code: MetaDiagnosticCode::MissingDeriveProvider,
                    request_index,
                    trait_identity: Some(trait_identity.clone()),
                    message: "no provider is locked for the exact trait identity".into(),
                    span: request.span(),
                });
                continue;
            };
            if provider.trait_identity() != trait_identity
                || provider.provider_identity().is_empty()
                || provider
                    .introduced_bounds()
                    .iter()
                    .any(|bound| !request.generic_parameters().contains(bound))
            {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "the provider introduces an invalid bound or identity",
                ));
                continue;
            }
            let pair = (trait_identity.clone(), request.target().to_owned());
            if context.manual_implementations.contains(&pair) {
                diagnostics.push(MetaDiagnostic {
                    code: MetaDiagnosticCode::CoherenceConflict,
                    request_index,
                    trait_identity: Some(trait_identity.clone()),
                    message: "a manual implementation already owns this trait/target pair".into(),
                    span: request.span(),
                });
                continue;
            }
            if !seen_pairs.insert(pair) {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "the same trait/target pair was requested more than once",
                ));
                continue;
            }
            output_traits.push(ValidatedTrait {
                identity: trait_identity.clone(),
                provider: provider.provider_identity().to_owned(),
                introduced_bounds: provider.introduced_bounds().to_vec(),
            });
        }
        if !output_traits.is_empty() {
            validated.push(ValidatedDerive {
                request_index,
                target: target.clone(),
                traits: output_traits,
            });
        }
    }

    if diagnostics.is_empty() {
        Ok(validated)
    } else {
        diagnostics.sort_by_key(|diagnostic| {
            (
                diagnostic.request_index,
                diagnostic.trait_identity.clone().unwrap_or_default(),
                diagnostic.code,
            )
        });
        Err(MetaSemanticError { diagnostics })
    }
}

/// Convert the HIR requests before passing them to the semantic validator.
pub fn validate_hir_derive_requests(
    module: impl Into<String>,
    program: &HirProgram,
    context: &DeriveContext,
) -> Result<Vec<ValidatedDerive>, MetaSemanticError> {
    let module = module.into();
    let requests = program
        .derive_requests()
        .iter()
        .map(|request| DeriveRequest::from_hir(module.clone(), request))
        .collect::<Vec<_>>();
    validate_derive_requests(&requests, context)
}

fn unique_binders(values: &[String]) -> bool {
    values.iter().all(|value| !value.is_empty()) && {
        let mut seen = BTreeSet::new();
        values.iter().all(|value| seen.insert(value))
    }
}

fn invalid_target(request_index: usize, request: &DeriveRequest, message: &str) -> MetaDiagnostic {
    MetaDiagnostic {
        code: MetaDiagnosticCode::InvalidDeriveTarget,
        request_index,
        trait_identity: None,
        message: message.into(),
        span: request.span(),
    }
}

fn invalid_request(
    request_index: usize,
    request: &DeriveRequest,
    trait_identity: Option<&str>,
    message: &str,
) -> MetaDiagnostic {
    MetaDiagnostic {
        code: MetaDiagnosticCode::InvalidDeriveRequest,
        request_index,
        trait_identity: trait_identity.map(str::to_owned),
        message: message.into(),
        span: request.span(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> DeriveContext {
        let mut context = DeriveContext::new("main");
        context.add_target(DeriveTarget::new(
            "User",
            "main",
            std::iter::empty::<String>(),
            DeriveTargetKind::Record,
        ));
        context.add_target(DeriveTarget::new(
            "Page",
            "main",
            ["T"],
            DeriveTargetKind::Newtype,
        ));
        context.add_trait("serialization.Serialize");
        context.add_trait("serialization.Deserialize");
        context.add_provider(DeriveProvider::new(
            "serialization.Serialize",
            "std.meta.serialization",
            ["T"],
        ));
        context.add_provider(DeriveProvider::new(
            "serialization.Deserialize",
            "std.meta.serialization",
            std::iter::empty::<String>(),
        ));
        context
    }

    fn request(target: &str, binders: &[&str], traits: &[&str]) -> DeriveRequest {
        DeriveRequest::new(
            "main",
            target,
            binders.iter().copied(),
            traits.iter().copied(),
        )
    }

    #[test]
    fn valid_requests_are_resolved_in_source_order_and_provider_bounds_are_canonical() {
        let mut context = context();
        context.add_provider(DeriveProvider::new(
            "serialization.Serialize",
            "std.meta.serialization",
            ["T", "T"],
        ));
        let requests = [
            request(
                "Page",
                &["T"],
                &["serialization.Serialize", "serialization.Deserialize"],
            ),
            request("User", &[], &["serialization.Deserialize"]),
        ];
        let result = validate_derive_requests(&requests, &context).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].request_index(), 0);
        assert_eq!(result[0].target().identity(), "Page");
        assert_eq!(result[0].traits()[0].introduced_bounds(), ["T"]);
        assert_eq!(result[1].target().kind(), DeriveTargetKind::Record);
        assert_eq!(META_MODEL, "tondo-meta-model-0.1/1");
    }

    #[test]
    fn invalid_target_shapes_and_binders_use_e2101() {
        let mut context = context();
        context.add_target(DeriveTarget::new(
            "Alias",
            "main",
            std::iter::empty::<String>(),
            DeriveTargetKind::Alias,
        ));
        let requests = [
            request("missing", &[], &["serialization.Deserialize"]),
            request("Alias", &[], &["serialization.Deserialize"]),
            request("Page", &[], &["serialization.Deserialize"]),
        ];
        let error = validate_derive_requests(&requests, &context).unwrap_err();
        assert_eq!(
            error
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.code().as_str())
                .collect::<Vec<_>>(),
            ["E2101", "E2101", "E2101"]
        );
    }

    #[test]
    fn missing_and_invalid_providers_are_distinguished() {
        let mut context = context();
        context.add_trait("serialization.Missing");
        context.add_provider(DeriveProvider::new(
            "serialization.Deserialize",
            "",
            std::iter::empty::<String>(),
        ));
        let requests = [
            request("User", &[], &["serialization.Missing"]),
            request("User", &[], &["serialization.Deserialize"]),
        ];
        let error = validate_derive_requests(&requests, &context).unwrap_err();
        assert_eq!(
            error.diagnostics()[0].code(),
            MetaDiagnosticCode::MissingDeriveProvider
        );
        assert_eq!(
            error.diagnostics()[1].code(),
            MetaDiagnosticCode::InvalidDeriveRequest
        );
    }

    #[test]
    fn duplicates_bounds_traits_requests_and_manual_impls_are_rejected() {
        let mut context = context();
        context.add_manual_implementation("serialization.Deserialize", "User");
        let requests = [
            request(
                "User",
                &[],
                &["serialization.Serialize", "serialization.Serialize"],
            ),
            request("User", &[], &["serialization.Deserialize"]),
            request("User", &[], &["serialization.Serialize"]),
        ];
        let error = validate_derive_requests(&requests, &context).unwrap_err();
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == MetaDiagnosticCode::CoherenceConflict)
        );
        assert!(
            error
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message().contains("only once"))
        );
        assert!(
            error
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.code() == MetaDiagnosticCode::InvalidDeriveRequest)
                .count()
                >= 2
        );
    }

    #[test]
    fn invalid_trait_and_provider_bound_use_e2103_and_errors_are_atomic() {
        let mut context = context();
        context.add_trait("serialization.Bad");
        context.add_provider(DeriveProvider::new(
            "serialization.Bad",
            "std.meta.bad",
            ["U"],
        ));
        let requests = [
            request("User", &[], &["serialization.NotATrait"]),
            request("User", &[], &["serialization.Bad"]),
        ];
        let error = validate_derive_requests(&requests, &context).unwrap_err();
        assert_eq!(error.diagnostics().len(), 2);
        assert!(
            error
                .diagnostics()
                .iter()
                .all(|diagnostic| diagnostic.code() == MetaDiagnosticCode::InvalidDeriveRequest)
        );
        assert_eq!(error.to_string(), "2 derive diagnostic(s)");
    }

    #[test]
    fn request_accessors_and_hir_conversion_preserve_identity_and_span_shape() {
        let request = DeriveRequest::new("main", "User", std::iter::empty::<String>(), ["Trait"]);
        assert_eq!(request.module(), "main");
        assert_eq!(request.target(), "User");
        assert!(request.generic_parameters().is_empty());
        assert_eq!(request.traits(), ["Trait"]);
        assert!(request.span().is_none());
        assert!(
            MetaDiagnosticCode::InvalidDeriveTarget
                .as_str()
                .starts_with('E')
        );
    }

    fn snapshot_span(offset: u32) -> MetaSpan {
        MetaSpan::new(3, offset, offset + 2).unwrap()
    }

    fn snapshot_field(name: &str, ordinal: u32) -> MetaField {
        MetaField::new(
            name,
            "String",
            MetaVisibility::Public,
            ordinal,
            snapshot_span(ordinal),
            Some(format!("{name} docs")),
        )
        .unwrap()
    }

    fn snapshot_declaration(identity: &str, kind: MetaDeclarationKind) -> MetaDeclaration {
        MetaDeclaration::new(
            identity,
            "app.models",
            MetaVisibility::Public,
            [MetaGenericParameter::new("T", ["Eq", "Eq"]).unwrap()],
            [MetaBound::new("T", "std.Eq").unwrap()],
            snapshot_span(10),
            Some("public declaration"),
            kind,
        )
        .unwrap()
    }

    fn snapshot(reverse: bool) -> MetaSnapshot {
        let user = snapshot_declaration(
            "User",
            MetaDeclarationKind::record([snapshot_field("name", 1), snapshot_field("id", 0)]),
        );
        let state = snapshot_declaration(
            "State",
            MetaDeclarationKind::enumeration([
                MetaVariant::new(
                    "Ready",
                    MetaVariantPayload::unit(),
                    1,
                    snapshot_span(20),
                    None::<String>,
                )
                .unwrap(),
                MetaVariant::new(
                    "Busy",
                    MetaVariantPayload::record([snapshot_field("reason", 0)]),
                    0,
                    snapshot_span(18),
                    Some("busy state"),
                )
                .unwrap(),
            ]),
        );
        let mut roots = vec![MetaRoot::new("app", "app.models").unwrap()];
        let mut modules = vec![MetaModule::new("app.models", Some("model docs")).unwrap()];
        let mut declarations = vec![user, state];
        if reverse {
            roots.reverse();
            modules.reverse();
            declarations.reverse();
        }
        MetaSnapshot::new(roots, modules, declarations).unwrap()
    }

    #[test]
    fn meta_snapshot_is_canonical_and_round_trips_with_a_stable_hash() {
        let first = snapshot(false);
        let second = snapshot(true);
        assert_eq!(first.format(), META_MODEL);
        assert_eq!(first.roots()[0].package(), "app");
        assert_eq!(first.roots()[0].module(), "app.models");
        assert_eq!(first.modules()[0].identity(), "app.models");
        assert_eq!(first.modules()[0].docs(), Some("model docs"));
        assert_eq!(first.declarations()[0].identity(), "State");
        assert_eq!(first.declarations()[0].module(), "app.models");
        assert_eq!(first.declarations()[0].visibility(), MetaVisibility::Public);
        assert_eq!(first.declarations()[0].generic_parameters()[0].name(), "T");
        assert_eq!(
            first.declarations()[0].generic_parameters()[0].bounds(),
            ["Eq"]
        );
        assert_eq!(first.declarations()[0].bounds()[0].binder(), "T");
        assert_eq!(
            first.declarations()[0].bounds()[0].trait_identity(),
            "std.Eq"
        );
        assert_eq!(first.declarations()[0].span(), snapshot_span(10));
        assert_eq!(first.declarations()[0].docs(), Some("public declaration"));
        assert_eq!(first.declarations()[0].kind().name(), "enum");
        assert_eq!(first.declarations()[1].kind().name(), "record");

        let bytes = first.canonical_bytes().unwrap();
        assert_eq!(bytes, second.canonical_bytes().unwrap());
        assert_eq!(first.hash().unwrap(), second.hash().unwrap());
        assert!(String::from_utf8_lossy(&bytes).contains("tondo-meta-model-0.1/1"));
        for forbidden in ["body", "value", "layout", "address", "gc"] {
            assert!(!String::from_utf8_lossy(&bytes).contains(forbidden));
        }
        let decoded = MetaSnapshot::decode(&bytes).unwrap();
        assert_eq!(decoded, first);
    }

    #[test]
    fn meta_snapshot_shapes_expose_only_structural_data() {
        let tuple = MetaVariantPayload::tuple(["Int", "String"]);
        assert_eq!(tuple.kind(), "tuple");
        assert_eq!(MetaVariantPayload::unit().kind(), "unit");
        let field = snapshot_field("x", 0);
        assert_eq!(field.name(), "x");
        assert_eq!(field.ty(), "String");
        assert_eq!(field.visibility(), MetaVisibility::Public);
        assert_eq!(field.ordinal(), 0);
        assert_eq!(field.span(), snapshot_span(0));
        assert_eq!(field.docs(), Some("x docs"));

        let variant =
            MetaVariant::new("Tuple", tuple, 0, snapshot_span(4), Some("tuple docs")).unwrap();
        assert_eq!(variant.name(), "Tuple");
        assert_eq!(variant.payload().kind(), "tuple");
        assert_eq!(variant.ordinal(), 0);
        assert_eq!(variant.span(), snapshot_span(4));
        assert_eq!(variant.docs(), Some("tuple docs"));

        let operation = MetaOperation::new(
            "encode",
            "fn(Self): Bytes",
            MetaVisibility::Public,
            0,
            snapshot_span(30),
            Some("operation docs"),
        )
        .unwrap();
        assert_eq!(operation.name(), "encode");
        assert_eq!(operation.signature(), "fn(Self): Bytes");
        assert_eq!(operation.visibility(), MetaVisibility::Public);
        assert_eq!(operation.ordinal(), 0);
        assert_eq!(operation.span(), snapshot_span(30));
        assert_eq!(operation.docs(), Some("operation docs"));

        let trait_declaration = MetaDeclaration::new(
            "Codec",
            "app.models",
            MetaVisibility::Private,
            std::iter::empty::<MetaGenericParameter>(),
            std::iter::empty::<MetaBound>(),
            snapshot_span(31),
            None::<String>,
            MetaDeclarationKind::trait_definition([operation]),
        )
        .unwrap();
        assert_eq!(trait_declaration.visibility(), MetaVisibility::Private);
        assert_eq!(trait_declaration.kind().name(), "trait");

        let newtype = MetaDeclaration::new(
            "UserId",
            "app.models",
            MetaVisibility::Public,
            std::iter::empty::<MetaGenericParameter>(),
            std::iter::empty::<MetaBound>(),
            snapshot_span(32),
            None::<String>,
            MetaDeclarationKind::newtype("Int"),
        )
        .unwrap();
        assert_eq!(newtype.kind().name(), "newtype");
    }

    #[test]
    fn meta_snapshot_rejects_invalid_values_duplicates_and_noncanonical_bytes() {
        assert!(matches!(
            MetaSpan::new(0, 3, 2),
            Err(MetaModelError::InvalidSpan { .. })
        ));
        assert!(matches!(
            MetaRoot::new("bad\nroot", "main"),
            Err(MetaModelError::InvalidValue { .. })
        ));
        assert!(matches!(
            MetaField::new(
                "x",
                "",
                MetaVisibility::Private,
                0,
                snapshot_span(0),
                None::<String>,
            ),
            Err(MetaModelError::InvalidValue { .. })
        ));
        let duplicate_root = MetaRoot::new("app", "main").unwrap();
        let duplicate = MetaSnapshot::new(
            [duplicate_root.clone(), duplicate_root],
            std::iter::empty::<MetaModule>(),
            std::iter::empty::<MetaDeclaration>(),
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate root"));

        let mut duplicate_json: serde_json::Value =
            serde_json::from_slice(&snapshot(false).canonical_bytes().unwrap()).unwrap();
        let field = duplicate_json["declarations"][1]["kind"]["Record"][0].clone();
        duplicate_json["declarations"][1]["kind"]["Record"] =
            serde_json::json!([field.clone(), field]);
        let duplicate_fields =
            MetaSnapshot::decode(&serde_json::to_vec(&duplicate_json).unwrap()).unwrap_err();
        assert!(duplicate_fields.to_string().contains("duplicate field"));

        let mut unsupported = serde_json::to_vec(&serde_json::json!({
            "format": "tondo-meta-model-0.2/1",
            "roots": [],
            "modules": [],
            "declarations": []
        }))
        .unwrap();
        assert!(matches!(
            MetaSnapshot::decode(&unsupported),
            Err(MetaModelError::UnsupportedFormat(_))
        ));
        unsupported.extend_from_slice(b" ");
        assert!(matches!(
            MetaSnapshot::decode(&unsupported),
            Err(MetaModelError::UnsupportedFormat(_))
        ));

        let canonical = snapshot(false).canonical_bytes().unwrap();
        let mut reordered = canonical.clone();
        reordered.extend_from_slice(b" ");
        assert_eq!(
            MetaSnapshot::decode(&reordered),
            Err(MetaModelError::NonCanonicalEncoding)
        );
        assert!(
            MetaModelError::NonCanonicalEncoding
                .to_string()
                .contains("canonical")
        );
        assert!(
            MetaModelError::Serialization("x".into())
                .to_string()
                .contains("encoding")
        );
    }
}
