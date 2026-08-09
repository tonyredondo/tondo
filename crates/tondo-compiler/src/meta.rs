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
/// The closed companion API exposed to a Tondo `std.meta` program.
pub const META_API: &str = "tondo-std-meta-0.1/1";

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

/// A compile-time annotation retained for a derive provider.
///
/// The meta model intentionally keeps annotations as inert, canonical data.
/// Providers validate the closed vocabulary for their owner; no executable
/// callback or host value can cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaAttribute {
    name: String,
    argument: Option<String>,
}

impl MetaAttribute {
    pub fn new(
        name: impl Into<String>,
        argument: Option<impl Into<String>>,
    ) -> Result<Self, MetaModelError> {
        let name = required_text("attribute.name", name.into())?;
        let argument = argument
            .map(Into::into)
            .map(|value| required_text("attribute.argument", value))
            .transpose()?;
        Ok(Self { name, argument })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn argument(&self) -> Option<&str> {
        self.argument.as_deref()
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<MetaAttribute>,
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
            attributes: Vec::new(),
        })
    }

    pub fn with_attributes(
        mut self,
        attributes: impl IntoIterator<Item = MetaAttribute>,
    ) -> Result<Self, MetaModelError> {
        self.attributes = attributes.into_iter().collect();
        self.canonicalize_attributes()?;
        Ok(self)
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

    pub fn attributes(&self) -> &[MetaAttribute] {
        &self.attributes
    }

    fn canonicalize_attributes(&mut self) -> Result<(), MetaModelError> {
        for attribute in &self.attributes {
            required_text("attribute.name", attribute.name.clone())?;
            if let Some(argument) = &attribute.argument {
                required_text("attribute.argument", argument.clone())?;
            }
        }
        self.attributes.sort();
        if self
            .attributes
            .windows(2)
            .any(|window| window[0] == window[1])
        {
            return Err(MetaModelError::Duplicate {
                kind: "field attribute".into(),
                identity: self.attributes[0].name.clone(),
            });
        }
        Ok(())
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<MetaAttribute>,
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
            attributes: Vec::new(),
        })
    }

    pub fn with_attributes(
        mut self,
        attributes: impl IntoIterator<Item = MetaAttribute>,
    ) -> Result<Self, MetaModelError> {
        self.attributes = attributes.into_iter().collect();
        self.canonicalize_attributes()?;
        Ok(self)
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

    pub fn attributes(&self) -> &[MetaAttribute] {
        &self.attributes
    }

    fn canonicalize(&mut self) -> Result<(), MetaModelError> {
        required_text("variant.name", self.name.clone())?;
        self.canonicalize_attributes()?;
        self.payload.canonicalize()
    }

    fn canonicalize_attributes(&mut self) -> Result<(), MetaModelError> {
        for attribute in &self.attributes {
            required_text("attribute.name", attribute.name.clone())?;
            if let Some(argument) = &attribute.argument {
                required_text("attribute.argument", argument.clone())?;
            }
        }
        self.attributes.sort();
        if self
            .attributes
            .windows(2)
            .any(|window| window[0] == window[1])
        {
            return Err(MetaModelError::Duplicate {
                kind: "variant attribute".into(),
                identity: self.attributes[0].name.clone(),
            });
        }
        Ok(())
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
    #[allow(clippy::too_many_arguments)]
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

/// Finite resources admitted to one hermetic meta run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaLimits {
    steps: u64,
    memory_bytes: u64,
    output_bytes: u64,
}

impl MetaLimits {
    pub fn new(
        steps: u64,
        memory_bytes: u64,
        output_bytes: u64,
    ) -> Result<Self, MetaContractError> {
        if steps == 0 || memory_bytes == 0 || output_bytes == 0 {
            return Err(MetaContractError::InvalidLimit);
        }
        Ok(Self {
            steps,
            memory_bytes,
            output_bytes,
        })
    }

    pub fn steps(&self) -> u64 {
        self.steps
    }

    pub fn memory_bytes(&self) -> u64 {
        self.memory_bytes
    }

    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
}

/// An input owned by the request. Inputs are values; the companion has no
/// filesystem, environment, callback, or capability channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaInput {
    name: String,
    bytes: Vec<u8>,
    hash: String,
}

impl MetaInput {
    pub fn new(
        name: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<Self, MetaContractError> {
        let name = required_contract_text("input.name", name.into())?;
        let bytes = bytes.into();
        let hash = crate::artifact::sha256(&bytes);
        Ok(Self { name, bytes, hash })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// One output path that the request authorizes exactly once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaOutputSpec {
    path: String,
    module: String,
}

impl MetaOutputSpec {
    pub fn new(
        path: impl Into<String>,
        module: impl Into<String>,
    ) -> Result<Self, MetaContractError> {
        let path = validate_meta_path(path.into())?;
        let module = required_contract_text("output.module", module.into())?;
        Ok(Self { path, module })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn module(&self) -> &str {
        &self.module
    }
}

/// A generated UTF-8 source file accepted by the source builder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaSource {
    path: String,
    module: String,
    bytes: Vec<u8>,
    hash: String,
    mappings: Vec<MetaSourceMapEntry>,
}

impl MetaSource {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn mappings(&self) -> &[MetaSourceMapEntry] {
        &self.mappings
    }
}

/// A half-open generated byte range associated with one authorized input span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaSourceMapEntry {
    generated_start: u32,
    generated_end: u32,
    origin: MetaSpan,
}

impl MetaSourceMapEntry {
    pub fn new(
        generated_start: u32,
        generated_end: u32,
        origin: MetaSpan,
    ) -> Result<Self, MetaContractError> {
        if generated_start > generated_end {
            return Err(MetaContractError::InvalidSourceMap);
        }
        Ok(Self {
            generated_start,
            generated_end,
            origin,
        })
    }

    pub fn generated_start(&self) -> u32 {
        self.generated_start
    }

    pub fn generated_end(&self) -> u32 {
        self.generated_end
    }

    pub fn origin(&self) -> MetaSpan {
        self.origin
    }
}

/// A request is an owned, immutable hand-off from the toolchain to one
/// companion run. It contains no callback or ambient capability slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaRequest {
    api: String,
    snapshot: MetaSnapshot,
    inputs: Vec<MetaInput>,
    outputs: Vec<MetaOutputSpec>,
    limits: MetaLimits,
}

impl MetaRequest {
    pub fn new(
        snapshot: MetaSnapshot,
        inputs: impl IntoIterator<Item = MetaInput>,
        outputs: impl IntoIterator<Item = MetaOutputSpec>,
        limits: MetaLimits,
    ) -> Result<Self, MetaContractError> {
        let mut request = Self {
            api: META_API.into(),
            snapshot,
            inputs: inputs.into_iter().collect(),
            outputs: outputs.into_iter().collect(),
            limits,
        };
        request.inputs.sort_by_key(|input| input.name.clone());
        ensure_unique_contract_keys(
            request.inputs.iter().map(|input| input.name.clone()),
            "input",
        )?;
        request.outputs.sort_by_key(|output| output.path.clone());
        ensure_unique_contract_keys(
            request.outputs.iter().map(|output| output.path.clone()),
            "output",
        )?;
        Ok(request)
    }

    pub fn api(&self) -> &str {
        &self.api
    }

    pub fn snapshot(&self) -> &MetaSnapshot {
        &self.snapshot
    }

    pub fn inputs(&self) -> &[MetaInput] {
        &self.inputs
    }

    pub fn outputs(&self) -> &[MetaOutputSpec] {
        &self.outputs
    }

    pub fn limits(&self) -> MetaLimits {
        self.limits
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MetaContractError> {
        let canonical = self.clone().canonicalize()?;
        serde_json::to_vec(&canonical)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))
    }

    pub fn hash(&self) -> Result<String, MetaContractError> {
        Ok(crate::artifact::sha256(&self.canonical_bytes()?))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaContractError> {
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))?;
        let canonical = request.canonicalize()?;
        if serde_json::to_vec(&canonical)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))?
            != bytes
        {
            return Err(MetaContractError::NonCanonicalEncoding);
        }
        Ok(canonical)
    }

    /// Transfer ownership to the only output construction API.
    pub fn into_source_builder(self) -> MetaSourceBuilder {
        MetaSourceBuilder {
            expected: self
                .outputs
                .into_iter()
                .map(|output| (output.path.clone(), output))
                .collect(),
            limits: self.limits,
            outputs: BTreeMap::new(),
        }
    }

    fn canonicalize(self) -> Result<Self, MetaContractError> {
        if self.api != META_API {
            return Err(MetaContractError::UnsupportedApi(self.api));
        }
        MetaLimits::new(
            self.limits.steps,
            self.limits.memory_bytes,
            self.limits.output_bytes,
        )?;
        let snapshot_bytes = self
            .snapshot
            .canonical_bytes()
            .map_err(|error| MetaContractError::InvalidSnapshot(error.to_string()))?;
        let snapshot = MetaSnapshot::decode(&snapshot_bytes)
            .map_err(|error| MetaContractError::InvalidSnapshot(error.to_string()))?;
        let mut inputs = Vec::with_capacity(self.inputs.len());
        for input in self.inputs {
            let canonical = MetaInput::new(input.name, input.bytes)?;
            if canonical.hash != input.hash {
                return Err(MetaContractError::InputHashMismatch(canonical.name));
            }
            inputs.push(canonical);
        }
        let outputs = self
            .outputs
            .into_iter()
            .map(|output| MetaOutputSpec::new(output.path, output.module))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(snapshot, inputs, outputs, self.limits)
    }
}

/// An owned builder that accepts only the output paths declared by its request.
#[derive(Debug)]
pub struct MetaSourceBuilder {
    expected: BTreeMap<String, MetaOutputSpec>,
    limits: MetaLimits,
    outputs: BTreeMap<String, MetaSource>,
}

impl MetaSourceBuilder {
    pub fn add_source(
        &mut self,
        path: impl Into<String>,
        module: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
    ) -> Result<(), MetaContractError> {
        self.add_mapped_source(path, module, bytes, [])
    }

    pub fn add_mapped_source(
        &mut self,
        path: impl Into<String>,
        module: impl Into<String>,
        bytes: impl Into<Vec<u8>>,
        mappings: impl IntoIterator<Item = MetaSourceMapEntry>,
    ) -> Result<(), MetaContractError> {
        let path = validate_meta_path(path.into())?;
        let module = required_contract_text("output.module", module.into())?;
        let Some(expected) = self.expected.get(&path) else {
            return Err(MetaContractError::UnknownOutput(path));
        };
        if expected.module != module {
            return Err(MetaContractError::OutputModuleMismatch { path });
        }
        if self.outputs.contains_key(&path) {
            return Err(MetaContractError::DuplicateOutput(path));
        }
        let bytes = bytes.into();
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| MetaContractError::InvalidSourceUtf8(path.clone()))?;
        let mut mappings = mappings.into_iter().collect::<Vec<_>>();
        mappings.sort();
        let mut previous_end = 0_u32;
        for mapping in &mappings {
            let start = usize::try_from(mapping.generated_start)
                .map_err(|_| MetaContractError::InvalidSourceMap)?;
            let end = usize::try_from(mapping.generated_end)
                .map_err(|_| MetaContractError::InvalidSourceMap)?;
            if start > text.len()
                || end > text.len()
                || !text.is_char_boundary(start)
                || !text.is_char_boundary(end)
                || mapping.generated_start < previous_end
            {
                return Err(MetaContractError::InvalidSourceMap);
            }
            previous_end = mapping.generated_end;
        }
        let current = self
            .outputs
            .values()
            .try_fold(0_u64, |total, source| {
                total.checked_add(source.bytes.len() as u64)
            })
            .and_then(|total| total.checked_add(bytes.len() as u64))
            .ok_or(MetaContractError::OutputLimit {
                limit: self.limits.output_bytes,
            })?;
        if current > self.limits.output_bytes {
            return Err(MetaContractError::OutputLimit {
                limit: self.limits.output_bytes,
            });
        }
        let hash = crate::artifact::sha256(&bytes);
        self.outputs.insert(
            path.clone(),
            MetaSource {
                path,
                module,
                bytes,
                hash,
                mappings,
            },
        );
        Ok(())
    }

    pub fn finish(self) -> Result<MetaResponse, MetaContractError> {
        for path in self.expected.keys() {
            if !self.outputs.contains_key(path) {
                return Err(MetaContractError::MissingOutput(path.clone()));
            }
        }
        Ok(MetaResponse {
            outputs: self.outputs.into_values().collect(),
        })
    }
}

/// A successful response contains all and only the declared outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaResponse {
    outputs: Vec<MetaSource>,
}

impl MetaResponse {
    pub fn outputs(&self) -> &[MetaSource] {
        &self.outputs
    }

    pub fn output(&self, path: &str) -> Option<&MetaSource> {
        self.outputs.iter().find(|source| source.path == path)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MetaContractError> {
        let canonical = self.clone().canonicalize()?;
        serde_json::to_vec(&canonical)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))
    }

    pub fn hash(&self) -> Result<String, MetaContractError> {
        Ok(crate::artifact::sha256(&self.canonical_bytes()?))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaContractError> {
        let response: Self = serde_json::from_slice(bytes)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))?;
        let canonical = response.canonicalize()?;
        if serde_json::to_vec(&canonical)
            .map_err(|error| MetaContractError::Serialization(error.to_string()))?
            != bytes
        {
            return Err(MetaContractError::NonCanonicalEncoding);
        }
        Ok(canonical)
    }

    fn canonicalize(self) -> Result<Self, MetaContractError> {
        let limits = MetaLimits::new(u64::MAX, u64::MAX, u64::MAX)?;
        let outputs = self.outputs;
        let specs = outputs
            .iter()
            .map(|source| MetaOutputSpec::new(source.path.clone(), source.module.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let snapshot = MetaSnapshot::new([], [], [])
            .map_err(|error| MetaContractError::InvalidSnapshot(error.to_string()))?;
        let mut builder = MetaRequest::new(snapshot, [], specs, limits)?.into_source_builder();
        for source in outputs {
            let actual_hash = crate::artifact::sha256(&source.bytes);
            if actual_hash != source.hash {
                return Err(MetaContractError::SourceHashMismatch(source.path));
            }
            builder.add_mapped_source(source.path, source.module, source.bytes, source.mappings)?;
        }
        builder.finish()
    }
}

/// Closed request/response boundary errors. There is intentionally no variant
/// for invoking a callback or acquiring a host capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaContractError {
    UnsupportedApi(String),
    InvalidLimit,
    InvalidSnapshot(String),
    InvalidText { field: String },
    InvalidPath(String),
    DuplicateInput(String),
    DuplicateOutput(String),
    UnknownOutput(String),
    OutputModuleMismatch { path: String },
    InvalidSourceUtf8(String),
    InvalidSourceMap,
    OutputLimit { limit: u64 },
    MissingOutput(String),
    InputHashMismatch(String),
    SourceHashMismatch(String),
    NonCanonicalEncoding,
    Serialization(String),
}

impl fmt::Display for MetaContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedApi(api) => write!(formatter, "unsupported std.meta API `{api}`"),
            Self::InvalidLimit => formatter.write_str("meta limits must be positive"),
            Self::InvalidSnapshot(error) => write!(formatter, "invalid meta snapshot: {error}"),
            Self::InvalidText { field } => write!(formatter, "invalid meta text `{field}`"),
            Self::InvalidPath(path) => write!(formatter, "invalid meta output path `{path}`"),
            Self::DuplicateInput(name) => write!(formatter, "duplicate meta input `{name}`"),
            Self::DuplicateOutput(path) => write!(formatter, "duplicate meta output `{path}`"),
            Self::UnknownOutput(path) => write!(formatter, "undeclared meta output `{path}`"),
            Self::OutputModuleMismatch { path } => {
                write!(formatter, "meta output module mismatch for `{path}`")
            }
            Self::InvalidSourceUtf8(path) => write!(formatter, "meta source is not UTF-8 `{path}`"),
            Self::InvalidSourceMap => formatter.write_str("invalid meta source map"),
            Self::OutputLimit { limit } => {
                write!(formatter, "meta output limit exceeded ({limit} bytes)")
            }
            Self::MissingOutput(path) => write!(formatter, "missing declared meta output `{path}`"),
            Self::InputHashMismatch(name) => write!(formatter, "meta input hash mismatch `{name}`"),
            Self::SourceHashMismatch(path) => {
                write!(formatter, "meta source hash mismatch `{path}`")
            }
            Self::NonCanonicalEncoding => formatter.write_str("meta value is not canonical"),
            Self::Serialization(error) => write!(formatter, "meta encoding failed: {error}"),
        }
    }
}

impl Error for MetaContractError {}

fn required_contract_text(field: &str, value: String) -> Result<String, MetaContractError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(MetaContractError::InvalidText {
            field: field.into(),
        });
    }
    Ok(value)
}

fn validate_meta_path(path: String) -> Result<String, MetaContractError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || !path.ends_with(".to")
    {
        return Err(MetaContractError::InvalidPath(path));
    }
    required_contract_text("output.path", path)
}

fn ensure_unique_contract_keys(
    values: impl Iterator<Item = String>,
    kind: &str,
) -> Result<(), MetaContractError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value.clone()) {
            return Err(if kind == "input" {
                MetaContractError::DuplicateInput(value)
            } else {
                MetaContractError::DuplicateOutput(value)
            });
        }
    }
    Ok(())
}

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

fn canonicalize_fields(fields: &mut [MetaField]) -> Result<(), MetaModelError> {
    for field in fields.iter_mut() {
        required_text("field.name", field.name.clone())?;
        required_text("field.type", field.ty.clone())?;
        field.canonicalize_attributes()?;
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
    DeriveExpansionFailed,
    InvalidGeneratedSource,
    GeneratorContractViolation,
    GeneratorResourceLimit,
    MetaCapabilityDenied,
    GenerationDependencyCycle,
    CoherenceConflict,
}

impl MetaDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDeriveTarget => "E2101",
            Self::MissingDeriveProvider => "E2102",
            Self::InvalidDeriveRequest => "E2103",
            Self::DeriveExpansionFailed => "E2104",
            Self::InvalidGeneratedSource => "E2105",
            Self::GeneratorContractViolation => "E2106",
            Self::GeneratorResourceLimit => "E2107",
            Self::MetaCapabilityDenied => "E2108",
            Self::GenerationDependencyCycle => "E2109",
            Self::CoherenceConflict => "E1111",
        }
    }

    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidDeriveTarget => "invalid-derive-target",
            Self::MissingDeriveProvider => "missing-derive-provider",
            Self::InvalidDeriveRequest => "invalid-derive-request",
            Self::DeriveExpansionFailed => "derive-expansion-failed",
            Self::InvalidGeneratedSource => "invalid-generated-source",
            Self::GeneratorContractViolation => "generator-contract-violation",
            Self::GeneratorResourceLimit => "generator-resource-limit",
            Self::MetaCapabilityDenied => "meta-capability-denied",
            Self::GenerationDependencyCycle => "generation-dependency-cycle",
            Self::CoherenceConflict => "coherence-conflict",
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
            let base_trait_identity = derive_trait_base_identity(trait_identity);
            if !context.traits.contains(trait_identity)
                && !context.traits.contains(base_trait_identity.as_str())
            {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "the requested identity is not a trait",
                ));
                continue;
            }
            let Some(provider) = context
                .providers
                .get(trait_identity)
                .or_else(|| context.providers.get(base_trait_identity.as_str()))
            else {
                diagnostics.push(MetaDiagnostic {
                    code: MetaDiagnosticCode::MissingDeriveProvider,
                    request_index,
                    trait_identity: Some(trait_identity.clone()),
                    message: "no provider is locked for the exact trait identity".into(),
                    span: request.span(),
                });
                continue;
            };
            if derive_trait_base_identity(provider.trait_identity()) != base_trait_identity
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

/// Return the nominal trait identity for an optionally specialized derive
/// request.  The generic codec arguments remain in `ValidatedTrait::identity`;
/// provider lookup may reuse the locked provider for the unspecialized trait.
fn derive_trait_base_identity(identity: &str) -> String {
    identity
        .split_once('[')
        .map_or(identity, |(base, _)| base)
        .to_owned()
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

    #[test]
    fn specialized_trait_requests_reuse_the_locked_base_provider() {
        let mut context = DeriveContext::new("main");
        context.add_target(DeriveTarget::new(
            "User",
            "main",
            std::iter::empty::<String>(),
            DeriveTargetKind::Record,
        ));
        context.add_trait("serialization.Encode");
        context.add_provider(DeriveProvider::new(
            "serialization.Encode",
            "std.derive.serialization.Encode",
            std::iter::empty::<String>(),
        ));
        let request = DeriveRequest::new(
            "main",
            "User",
            std::iter::empty::<String>(),
            ["serialization.Encode[Json]"],
        );
        let plan = validate_derive_requests(&[request], &context).unwrap();
        assert_eq!(plan[0].traits()[0].identity(), "serialization.Encode[Json]");
        assert_eq!(
            plan[0].traits()[0].provider(),
            "std.derive.serialization.Encode"
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

    fn contract_request() -> MetaRequest {
        MetaRequest::new(
            snapshot(false),
            [
                MetaInput::new("z-schema", b"{}".to_vec()).unwrap(),
                MetaInput::new("a-schema", b"[]".to_vec()).unwrap(),
            ],
            [
                MetaOutputSpec::new("generated/z.to", "generated.z").unwrap(),
                MetaOutputSpec::new("generated/a.to", "generated.a").unwrap(),
            ],
            MetaLimits::new(100, 4_096, 128).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn std_meta_request_owns_inputs_and_builder_returns_declared_sources() {
        assert_eq!(META_API, "tondo-std-meta-0.1/1");
        let request = contract_request();
        assert_eq!(request.api(), META_API);
        assert_eq!(request.snapshot().format(), META_MODEL);
        assert_eq!(request.inputs()[0].name(), "a-schema");
        assert_eq!(request.inputs()[0].bytes(), b"[]");
        assert!(request.inputs()[0].hash().starts_with("sha256:"));
        assert_eq!(request.inputs()[1].name(), "z-schema");
        assert_eq!(request.outputs()[0].path(), "generated/a.to");
        assert_eq!(request.outputs()[0].module(), "generated.a");
        let limits = request.limits();
        assert_eq!(limits.steps(), 100);
        assert_eq!(limits.memory_bytes(), 4_096);
        assert_eq!(limits.output_bytes(), 128);

        let mut builder = request.into_source_builder();
        builder
            .add_source("generated/z.to", "generated.z", "type Z = Int")
            .unwrap();
        builder
            .add_source("generated/a.to", "generated.a", "type A = String")
            .unwrap();
        let response = builder.finish().unwrap();
        assert_eq!(response.outputs().len(), 2);
        assert_eq!(response.outputs()[0].path(), "generated/a.to");
        assert_eq!(response.outputs()[0].module(), "generated.a");
        assert_eq!(response.outputs()[0].bytes(), b"type A = String");
        assert!(response.outputs()[0].hash().starts_with("sha256:"));
        assert!(response.output("generated/z.to").is_some());
        assert!(response.output("missing.to").is_none());
    }

    #[test]
    fn std_meta_contract_rejects_ambient_boundaries_and_incomplete_outputs() {
        assert!(matches!(
            MetaLimits::new(0, 1, 1),
            Err(MetaContractError::InvalidLimit)
        ));
        assert!(matches!(
            MetaInput::new("bad\ninput", Vec::<u8>::new()),
            Err(MetaContractError::InvalidText { .. })
        ));
        for path in ["", "/absolute.to", "a/../b.to", "a\\b.to", "a.txt"] {
            assert!(matches!(
                MetaOutputSpec::new(path, "main"),
                Err(MetaContractError::InvalidPath(_))
            ));
        }
        assert!(matches!(
            MetaOutputSpec::new("a.to", "bad\nmodule"),
            Err(MetaContractError::InvalidText { .. })
        ));

        let limits = MetaLimits::new(1, 1, 3).unwrap();
        let duplicate_input = MetaRequest::new(
            snapshot(false),
            [
                MetaInput::new("same", vec![1]).unwrap(),
                MetaInput::new("same", vec![2]).unwrap(),
            ],
            std::iter::empty::<MetaOutputSpec>(),
            limits,
        )
        .unwrap_err();
        assert!(duplicate_input.to_string().contains("duplicate meta input"));

        let duplicate_output = MetaRequest::new(
            snapshot(false),
            std::iter::empty::<MetaInput>(),
            [
                MetaOutputSpec::new("same.to", "main").unwrap(),
                MetaOutputSpec::new("same.to", "main").unwrap(),
            ],
            limits,
        )
        .unwrap_err();
        assert!(
            duplicate_output
                .to_string()
                .contains("duplicate meta output")
        );

        let request = MetaRequest::new(
            snapshot(false),
            std::iter::empty::<MetaInput>(),
            [MetaOutputSpec::new("only.to", "main").unwrap()],
            MetaLimits::new(1, 1, 3).unwrap(),
        )
        .unwrap();
        let mut builder = request.into_source_builder();
        assert!(matches!(
            builder.add_source("unknown.to", "main", "x"),
            Err(MetaContractError::UnknownOutput(_))
        ));
        assert!(matches!(
            builder.add_source("only.to", "other", "x"),
            Err(MetaContractError::OutputModuleMismatch { .. })
        ));
        assert!(matches!(
            builder.add_source("only.to", "main", vec![0xff]),
            Err(MetaContractError::InvalidSourceUtf8(_))
        ));
        builder.add_source("only.to", "main", "ok").unwrap();
        assert!(matches!(
            builder.add_source("only.to", "main", "again"),
            Err(MetaContractError::DuplicateOutput(_))
        ));

        let mut oversized = MetaRequest::new(
            snapshot(false),
            std::iter::empty::<MetaInput>(),
            [MetaOutputSpec::new("small.to", "main").unwrap()],
            MetaLimits::new(1, 1, 2).unwrap(),
        )
        .unwrap()
        .into_source_builder();
        assert!(matches!(
            oversized.add_source("small.to", "main", "too long"),
            Err(MetaContractError::OutputLimit { .. })
        ));

        let missing = MetaRequest::new(
            snapshot(false),
            std::iter::empty::<MetaInput>(),
            [MetaOutputSpec::new("missing.to", "main").unwrap()],
            limits,
        )
        .unwrap()
        .into_source_builder()
        .finish()
        .unwrap_err();
        assert!(missing.to_string().contains("missing declared"));
        for error in [
            MetaContractError::InvalidLimit,
            MetaContractError::InvalidText { field: "x".into() },
            MetaContractError::InvalidPath("x".into()),
            MetaContractError::DuplicateInput("x".into()),
            MetaContractError::DuplicateOutput("x.to".into()),
            MetaContractError::UnknownOutput("x.to".into()),
            MetaContractError::OutputModuleMismatch {
                path: "x.to".into(),
            },
            MetaContractError::InvalidSourceUtf8("x.to".into()),
            MetaContractError::OutputLimit { limit: 1 },
            MetaContractError::MissingOutput("x.to".into()),
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
