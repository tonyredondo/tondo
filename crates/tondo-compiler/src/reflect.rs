//! Statically retained, metadata-only reflection for `std.reflect`.
//!
//! The linker receives a closed catalog and explicit `typeInfo[T]()` roots. It
//! retains only the public descriptor closure and emits immutable artifact-local
//! handles. There is deliberately no value channel, name lookup, constructor,
//! invocation hook, layout, address, or global type enumeration API.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::artifact::{sha256, validate_sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReflectTypeId {
    artifact_tag: [u8; 32],
    slot: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectTypeKind {
    Primitive(ReflectPrimitiveKind),
    Record,
    Enum,
    Newtype,
    Tuple,
    Union,
    Function,
    Applied(ReflectAppliedKind),
    Reference(ReflectReferenceKind),
    Opaque,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectPrimitiveKind {
    Bool,
    Int,
    Int8,
    Int16,
    Int32,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float,
    Float32,
    Byte,
    Char,
    String,
    Unit,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectAppliedKind {
    Array,
    Map,
    Set,
    Range,
    Option,
    Result,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectReferenceKind {
    Ref,
    Pointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectCapability {
    Copy,
    Discard,
    Equatable,
    Key,
    Send,
    Share,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReflectParameterMode {
    Value,
    Ref,
    Mut,
    Var,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectFieldTemplate {
    name: String,
    ty: String,
    ordinal: u32,
    docs: Option<String>,
    public: bool,
}

impl ReflectFieldTemplate {
    pub fn new(
        name: impl Into<String>,
        ty: impl Into<String>,
        ordinal: u32,
        docs: Option<impl Into<String>>,
        public: bool,
    ) -> Result<Self, ReflectError> {
        Ok(Self {
            name: required("field name", name.into())?,
            ty: required("field type", ty.into())?,
            ordinal,
            docs: optional_docs(docs.map(Into::into))?,
            public,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectVariantPayloadTemplate {
    Unit,
    Tuple(Vec<String>),
    Record(Vec<ReflectFieldTemplate>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectVariantTemplate {
    name: String,
    ordinal: u32,
    docs: Option<String>,
    payload: ReflectVariantPayloadTemplate,
}

impl ReflectVariantTemplate {
    pub fn new(
        name: impl Into<String>,
        ordinal: u32,
        docs: Option<impl Into<String>>,
        payload: ReflectVariantPayloadTemplate,
    ) -> Result<Self, ReflectError> {
        Ok(Self {
            name: required("variant name", name.into())?,
            ordinal,
            docs: optional_docs(docs.map(Into::into))?,
            payload,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectParameterTemplate {
    ty: String,
    mode: ReflectParameterMode,
}

impl ReflectParameterTemplate {
    pub fn new(ty: impl Into<String>, mode: ReflectParameterMode) -> Result<Self, ReflectError> {
        Ok(Self {
            ty: required("parameter type", ty.into())?,
            mode,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectFunctionTemplate {
    parameters: Vec<ReflectParameterTemplate>,
    outcome: String,
    variadic: bool,
    asynchronous: bool,
    unsafe_: bool,
}

impl ReflectFunctionTemplate {
    pub fn new(
        parameters: impl IntoIterator<Item = ReflectParameterTemplate>,
        outcome: impl Into<String>,
        variadic: bool,
        asynchronous: bool,
        unsafe_: bool,
    ) -> Result<Self, ReflectError> {
        Ok(Self {
            parameters: parameters.into_iter().collect(),
            outcome: required("function outcome", outcome.into())?,
            variadic,
            asynchronous,
            unsafe_,
        })
    }
}

/// Compiler-owned normalized description before reachability and compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectTypeTemplate {
    qualified_name: String,
    kind: ReflectTypeKind,
    generic_arguments: Vec<String>,
    capabilities: Vec<ReflectCapability>,
    fields: Vec<ReflectFieldTemplate>,
    variants: Vec<ReflectVariantTemplate>,
    tuple_elements: Vec<String>,
    function: Option<ReflectFunctionTemplate>,
    additional_dependencies: Vec<String>,
}

impl ReflectTypeTemplate {
    pub fn new(
        qualified_name: impl Into<String>,
        kind: ReflectTypeKind,
    ) -> Result<Self, ReflectError> {
        Ok(Self {
            qualified_name: required("type name", qualified_name.into())?,
            kind,
            generic_arguments: Vec::new(),
            capabilities: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            tuple_elements: Vec::new(),
            function: None,
            additional_dependencies: Vec::new(),
        })
    }

    pub fn generic_arguments(
        mut self,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ReflectError> {
        self.generic_arguments = arguments
            .into_iter()
            .map(|argument| required("generic argument", argument.into()))
            .collect::<Result<_, _>>()?;
        Ok(self)
    }

    pub fn capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = ReflectCapability>,
    ) -> Self {
        self.capabilities = capabilities.into_iter().collect();
        self.capabilities.sort();
        self.capabilities.dedup();
        self
    }

    pub fn fields(mut self, fields: impl IntoIterator<Item = ReflectFieldTemplate>) -> Self {
        self.fields = fields.into_iter().collect();
        self
    }

    pub fn variants(mut self, variants: impl IntoIterator<Item = ReflectVariantTemplate>) -> Self {
        self.variants = variants.into_iter().collect();
        self
    }

    pub fn tuple_elements(
        mut self,
        elements: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ReflectError> {
        self.tuple_elements = elements
            .into_iter()
            .map(|element| required("tuple element", element.into()))
            .collect::<Result<_, _>>()?;
        Ok(self)
    }

    pub fn function(mut self, function: ReflectFunctionTemplate) -> Self {
        self.function = Some(function);
        self
    }

    /// Records a structural dependency such as a newtype base without adding
    /// another runtime reflection accessor.
    pub fn dependency(mut self, ty: impl Into<String>) -> Result<Self, ReflectError> {
        self.additional_dependencies
            .push(required("type dependency", ty.into())?);
        Ok(self)
    }

    fn validate(&mut self) -> Result<(), ReflectError> {
        self.fields
            .sort_by_key(|field| (field.ordinal, field.name.clone()));
        unique_ordinal_name(
            self.fields
                .iter()
                .map(|field| (field.ordinal, field.name.as_str())),
            "field",
        )?;
        self.variants
            .sort_by_key(|variant| (variant.ordinal, variant.name.clone()));
        unique_ordinal_name(
            self.variants
                .iter()
                .map(|variant| (variant.ordinal, variant.name.as_str())),
            "variant",
        )?;
        for variant in &mut self.variants {
            match &mut variant.payload {
                ReflectVariantPayloadTemplate::Unit => {}
                ReflectVariantPayloadTemplate::Tuple(types) => {
                    for ty in types {
                        required("variant payload type", std::mem::take(ty))
                            .map(|value| *ty = value)?;
                    }
                }
                ReflectVariantPayloadTemplate::Record(fields) => {
                    fields.sort_by_key(|field| (field.ordinal, field.name.clone()));
                    unique_ordinal_name(
                        fields
                            .iter()
                            .map(|field| (field.ordinal, field.name.as_str())),
                        "variant field",
                    )?;
                }
            }
        }
        let shape_valid = match self.kind {
            ReflectTypeKind::Record => !self.fields.is_empty() && self.variants.is_empty(),
            ReflectTypeKind::Enum => !self.variants.is_empty() && self.fields.is_empty(),
            ReflectTypeKind::Tuple | ReflectTypeKind::Union => !self.tuple_elements.is_empty(),
            ReflectTypeKind::Function => self.function.is_some(),
            _ => {
                self.fields.is_empty()
                    && self.variants.is_empty()
                    && self.tuple_elements.is_empty()
                    && self.function.is_none()
            }
        };
        if !shape_valid {
            return Err(ReflectError::InvalidShape(self.qualified_name.clone()));
        }
        self.additional_dependencies.sort();
        self.additional_dependencies.dedup();
        Ok(())
    }

    fn references(&self) -> Vec<&str> {
        let mut references = self
            .generic_arguments
            .iter()
            .map(String::as_str)
            .chain(
                self.fields
                    .iter()
                    .filter(|field| field.public)
                    .map(|field| field.ty.as_str()),
            )
            .chain(self.tuple_elements.iter().map(String::as_str))
            .chain(self.additional_dependencies.iter().map(String::as_str))
            .collect::<Vec<_>>();
        for variant in &self.variants {
            match &variant.payload {
                ReflectVariantPayloadTemplate::Unit => {}
                ReflectVariantPayloadTemplate::Tuple(types) => {
                    references.extend(types.iter().map(String::as_str));
                }
                ReflectVariantPayloadTemplate::Record(fields) => {
                    references.extend(
                        fields
                            .iter()
                            .filter(|field| field.public)
                            .map(|field| field.ty.as_str()),
                    );
                }
            }
        }
        if let Some(function) = &self.function {
            references.extend(
                function
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty.as_str()),
            );
            references.push(&function.outcome);
        }
        references
    }
}

#[derive(Debug, Default)]
pub struct ReflectCatalog {
    types: BTreeMap<String, ReflectTypeTemplate>,
}

impl ReflectCatalog {
    pub fn insert(&mut self, mut template: ReflectTypeTemplate) -> Result<(), ReflectError> {
        template.validate()?;
        let identity = template.qualified_name.clone();
        if self.types.contains_key(&identity) {
            return Err(ReflectError::DuplicateType(identity));
        }
        self.types.insert(identity, template);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReflectMetadata {
    artifact_tag: [u8; 32],
    roots: Vec<ReflectTypeId>,
    records: Vec<ReflectRecord>,
}

impl ReflectMetadata {
    pub fn link(
        artifact_hash: &str,
        roots: impl IntoIterator<Item = impl AsRef<str>>,
        catalog: &ReflectCatalog,
    ) -> Result<Self, ReflectError> {
        validate_sha256(artifact_hash).map_err(|_| ReflectError::InvalidArtifactIdentity)?;
        let artifact_tag = artifact_tag(artifact_hash);
        let mut pending = roots
            .into_iter()
            .map(|root| root.as_ref().to_owned())
            .collect::<BTreeSet<_>>();
        let root_names = pending.clone();
        let mut retained = BTreeSet::new();
        while let Some(identity) = pending.pop_first() {
            if !retained.insert(identity.clone()) {
                continue;
            }
            let template = catalog
                .types
                .get(&identity)
                .ok_or_else(|| ReflectError::UnknownType(identity.clone()))?;
            for dependency in template.references() {
                if !retained.contains(dependency) {
                    pending.insert(dependency.to_owned());
                }
            }
        }

        let mut order = retained.into_iter().collect::<Vec<_>>();
        order.sort_by_key(|identity| sha256(format!("{artifact_hash}\0{identity}").as_bytes()));
        let ids = order
            .iter()
            .enumerate()
            .map(|(slot, identity)| {
                (
                    identity.clone(),
                    ReflectTypeId {
                        artifact_tag,
                        slot: slot as u32,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut records = Vec::with_capacity(order.len());
        for identity in &order {
            records.push(build_record(
                catalog
                    .types
                    .get(identity)
                    .expect("retained identities came from the catalog"),
                &ids,
            )?);
        }
        let mut roots = root_names
            .iter()
            .map(|identity| {
                ids.get(identity)
                    .copied()
                    .ok_or_else(|| ReflectError::UnknownType(identity.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        roots.sort();
        Ok(Self {
            artifact_tag,
            roots,
            records,
        })
    }

    pub fn roots(&self) -> &[ReflectTypeId] {
        &self.roots
    }

    pub fn retained_len(&self) -> usize {
        self.records.len()
    }

    pub fn type_info(&self, id: ReflectTypeId) -> Option<ReflectTypeInfo<'_>> {
        if id.artifact_tag != self.artifact_tag {
            return None;
        }
        self.records
            .get(id.slot as usize)
            .map(|record| ReflectTypeInfo {
                metadata: self,
                id,
                record,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectRecord {
    qualified_name: String,
    kind: ReflectTypeKind,
    generic_arguments: Vec<ReflectTypeId>,
    capabilities: Vec<ReflectCapability>,
    fields: Vec<ReflectFieldRecord>,
    variants: Vec<ReflectVariantRecord>,
    tuple_elements: Vec<ReflectTypeId>,
    function: Option<ReflectFunctionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectFieldRecord {
    name: String,
    ty: ReflectTypeId,
    ordinal: u32,
    docs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectVariantRecord {
    name: String,
    ordinal: u32,
    docs: Option<String>,
    payload: ReflectVariantPayloadRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReflectVariantPayloadRecord {
    Unit,
    Tuple(Vec<ReflectTypeId>),
    Record(Vec<ReflectFieldRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectFunctionRecord {
    parameters: Vec<ReflectParameterRecord>,
    outcome: ReflectTypeId,
    variadic: bool,
    asynchronous: bool,
    unsafe_: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReflectParameterRecord {
    ty: ReflectTypeId,
    mode: ReflectParameterMode,
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectTypeInfo<'a> {
    metadata: &'a ReflectMetadata,
    id: ReflectTypeId,
    record: &'a ReflectRecord,
}

impl<'a> ReflectTypeInfo<'a> {
    pub fn id(self) -> ReflectTypeId {
        self.id
    }
    pub fn qualified_name(self) -> &'a str {
        &self.record.qualified_name
    }
    pub fn kind(self) -> ReflectTypeKind {
        self.record.kind
    }
    pub fn generic_arguments(self) -> impl ExactSizeIterator<Item = ReflectTypeInfo<'a>> {
        self.record.generic_arguments.iter().map(|id| {
            self.metadata
                .type_info(*id)
                .expect("linked generic arguments are retained")
        })
    }
    pub fn capabilities(self) -> &'a [ReflectCapability] {
        &self.record.capabilities
    }
    pub fn fields(self) -> impl ExactSizeIterator<Item = ReflectFieldInfo<'a>> {
        self.record.fields.iter().map(|field| ReflectFieldInfo {
            metadata: self.metadata,
            record: field,
        })
    }
    pub fn variants(self) -> impl ExactSizeIterator<Item = ReflectVariantInfo<'a>> {
        self.record
            .variants
            .iter()
            .map(|variant| ReflectVariantInfo {
                metadata: self.metadata,
                record: variant,
            })
    }
    pub fn tuple_elements(self) -> impl ExactSizeIterator<Item = ReflectTypeInfo<'a>> {
        self.record.tuple_elements.iter().map(|id| {
            self.metadata
                .type_info(*id)
                .expect("linked tuple elements are retained")
        })
    }
    pub fn function(self) -> Option<ReflectFunctionInfo<'a>> {
        self.record
            .function
            .as_ref()
            .map(|record| ReflectFunctionInfo {
                metadata: self.metadata,
                record,
            })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectFieldInfo<'a> {
    metadata: &'a ReflectMetadata,
    record: &'a ReflectFieldRecord,
}

impl<'a> ReflectFieldInfo<'a> {
    pub fn name(self) -> &'a str {
        &self.record.name
    }
    pub fn ty(self) -> ReflectTypeInfo<'a> {
        self.metadata
            .type_info(self.record.ty)
            .expect("linked field types are retained")
    }
    pub fn ordinal(self) -> u32 {
        self.record.ordinal
    }
    pub fn docs(self) -> Option<&'a str> {
        self.record.docs.as_deref()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectVariantInfo<'a> {
    metadata: &'a ReflectMetadata,
    record: &'a ReflectVariantRecord,
}

impl<'a> ReflectVariantInfo<'a> {
    pub fn name(self) -> &'a str {
        &self.record.name
    }
    pub fn ordinal(self) -> u32 {
        self.record.ordinal
    }
    pub fn docs(self) -> Option<&'a str> {
        self.record.docs.as_deref()
    }
    pub fn payload(self) -> ReflectVariantPayloadInfo<'a> {
        ReflectVariantPayloadInfo {
            metadata: self.metadata,
            record: &self.record.payload,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflectVariantPayloadKind {
    Unit,
    Tuple,
    Record,
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectVariantPayloadInfo<'a> {
    metadata: &'a ReflectMetadata,
    record: &'a ReflectVariantPayloadRecord,
}

impl<'a> ReflectVariantPayloadInfo<'a> {
    pub fn kind(self) -> ReflectVariantPayloadKind {
        match self.record {
            ReflectVariantPayloadRecord::Unit => ReflectVariantPayloadKind::Unit,
            ReflectVariantPayloadRecord::Tuple(_) => ReflectVariantPayloadKind::Tuple,
            ReflectVariantPayloadRecord::Record(_) => ReflectVariantPayloadKind::Record,
        }
    }

    pub fn tuple_types(self) -> impl Iterator<Item = ReflectTypeInfo<'a>> {
        let types = match self.record {
            ReflectVariantPayloadRecord::Tuple(types) => types.as_slice(),
            _ => &[],
        };
        types
            .iter()
            .filter_map(move |id| self.metadata.type_info(*id))
    }

    pub fn record_fields(self) -> impl Iterator<Item = ReflectFieldInfo<'a>> {
        let fields = match self.record {
            ReflectVariantPayloadRecord::Record(fields) => fields.as_slice(),
            _ => &[],
        };
        fields.iter().map(move |record| ReflectFieldInfo {
            metadata: self.metadata,
            record,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectFunctionInfo<'a> {
    metadata: &'a ReflectMetadata,
    record: &'a ReflectFunctionRecord,
}

impl<'a> ReflectFunctionInfo<'a> {
    pub fn parameters(self) -> impl ExactSizeIterator<Item = ReflectParameterInfo<'a>> {
        self.record
            .parameters
            .iter()
            .enumerate()
            .map(|(position, record)| ReflectParameterInfo {
                metadata: self.metadata,
                position: position as u32,
                record,
            })
    }
    pub fn outcome(self) -> ReflectTypeInfo<'a> {
        self.metadata
            .type_info(self.record.outcome)
            .expect("linked function outcomes are retained")
    }
    pub fn variadic(self) -> bool {
        self.record.variadic
    }
    pub fn asynchronous(self) -> bool {
        self.record.asynchronous
    }
    pub fn unsafe_(self) -> bool {
        self.record.unsafe_
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReflectParameterInfo<'a> {
    metadata: &'a ReflectMetadata,
    position: u32,
    record: &'a ReflectParameterRecord,
}

impl<'a> ReflectParameterInfo<'a> {
    pub fn position(self) -> u32 {
        self.position
    }
    pub fn ty(self) -> ReflectTypeInfo<'a> {
        self.metadata
            .type_info(self.record.ty)
            .expect("linked parameter types are retained")
    }
    pub fn mode(self) -> ReflectParameterMode {
        self.record.mode
    }
}

fn build_record(
    template: &ReflectTypeTemplate,
    ids: &BTreeMap<String, ReflectTypeId>,
) -> Result<ReflectRecord, ReflectError> {
    let resolve = |identity: &str| {
        ids.get(identity)
            .copied()
            .ok_or_else(|| ReflectError::UnknownType(identity.into()))
    };
    let fields = template
        .fields
        .iter()
        .filter(|field| field.public)
        .map(|field| build_field(field, &resolve))
        .collect::<Result<_, _>>()?;
    let variants = template
        .variants
        .iter()
        .map(|variant| {
            let payload = match &variant.payload {
                ReflectVariantPayloadTemplate::Unit => ReflectVariantPayloadRecord::Unit,
                ReflectVariantPayloadTemplate::Tuple(types) => ReflectVariantPayloadRecord::Tuple(
                    types
                        .iter()
                        .map(|ty| resolve(ty))
                        .collect::<Result<_, _>>()?,
                ),
                ReflectVariantPayloadTemplate::Record(fields) => {
                    ReflectVariantPayloadRecord::Record(
                        fields
                            .iter()
                            .filter(|field| field.public)
                            .map(|field| build_field(field, &resolve))
                            .collect::<Result<_, _>>()?,
                    )
                }
            };
            Ok(ReflectVariantRecord {
                name: variant.name.clone(),
                ordinal: variant.ordinal,
                docs: variant.docs.clone(),
                payload,
            })
        })
        .collect::<Result<_, ReflectError>>()?;
    let function = template
        .function
        .as_ref()
        .map(|function| {
            Ok(ReflectFunctionRecord {
                parameters: function
                    .parameters
                    .iter()
                    .map(|parameter| {
                        Ok(ReflectParameterRecord {
                            ty: resolve(&parameter.ty)?,
                            mode: parameter.mode,
                        })
                    })
                    .collect::<Result<_, ReflectError>>()?,
                outcome: resolve(&function.outcome)?,
                variadic: function.variadic,
                asynchronous: function.asynchronous,
                unsafe_: function.unsafe_,
            })
        })
        .transpose()?;
    Ok(ReflectRecord {
        qualified_name: template.qualified_name.clone(),
        kind: template.kind,
        generic_arguments: template
            .generic_arguments
            .iter()
            .map(|argument| resolve(argument))
            .collect::<Result<_, _>>()?,
        capabilities: template.capabilities.clone(),
        fields,
        variants,
        tuple_elements: template
            .tuple_elements
            .iter()
            .map(|element| resolve(element))
            .collect::<Result<_, _>>()?,
        function,
    })
}

fn build_field(
    field: &ReflectFieldTemplate,
    resolve: &impl Fn(&str) -> Result<ReflectTypeId, ReflectError>,
) -> Result<ReflectFieldRecord, ReflectError> {
    Ok(ReflectFieldRecord {
        name: field.name.clone(),
        ty: resolve(&field.ty)?,
        ordinal: field.ordinal,
        docs: field.docs.clone(),
    })
}

fn artifact_tag(hash: &str) -> [u8; 32] {
    let digest = &hash["sha256:".len()..];
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16)
            .expect("validated SHA-256 bytes are hexadecimal");
    }
    bytes
}

fn required(field: &str, value: String) -> Result<String, ReflectError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        Err(ReflectError::InvalidText(field.into()))
    } else {
        Ok(value)
    }
}

fn optional_docs(value: Option<String>) -> Result<Option<String>, ReflectError> {
    value.map(|value| required("docs", value)).transpose()
}

fn unique_ordinal_name<'a>(
    values: impl IntoIterator<Item = (u32, &'a str)>,
    kind: &str,
) -> Result<(), ReflectError> {
    let mut ordinals = BTreeSet::new();
    let mut names = BTreeSet::new();
    for (ordinal, name) in values {
        if !ordinals.insert(ordinal) || !names.insert(name) {
            return Err(ReflectError::DuplicateMember(kind.into()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectError {
    InvalidArtifactIdentity,
    InvalidText(String),
    InvalidShape(String),
    DuplicateType(String),
    DuplicateMember(String),
    UnknownType(String),
}

impl fmt::Display for ReflectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArtifactIdentity => {
                formatter.write_str("invalid reflection artifact identity")
            }
            Self::InvalidText(field) => write!(formatter, "invalid reflection text `{field}`"),
            Self::InvalidShape(ty) => write!(formatter, "invalid reflection shape `{ty}`"),
            Self::DuplicateType(ty) => write!(formatter, "duplicate reflection type `{ty}`"),
            Self::DuplicateMember(kind) => write!(formatter, "duplicate reflection {kind}"),
            Self::UnknownType(ty) => write!(formatter, "unknown reflection type `{ty}`"),
        }
    }
}

impl Error for ReflectError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(label: &str) -> String {
        sha256(label.as_bytes())
    }

    fn primitive(name: &str, kind: ReflectPrimitiveKind) -> ReflectTypeTemplate {
        ReflectTypeTemplate::new(name, ReflectTypeKind::Primitive(kind)).unwrap()
    }

    fn field(name: &str, ty: &str, ordinal: u32, public: bool) -> ReflectFieldTemplate {
        ReflectFieldTemplate::new(name, ty, ordinal, Some(format!("{name} docs")), public).unwrap()
    }

    fn base_catalog() -> ReflectCatalog {
        let mut catalog = ReflectCatalog::default();
        for (name, kind) in [
            ("std.Bool", ReflectPrimitiveKind::Bool),
            ("std.Int", ReflectPrimitiveKind::Int),
            ("std.String", ReflectPrimitiveKind::String),
            ("std.Unit", ReflectPrimitiveKind::Unit),
        ] {
            catalog.insert(primitive(name, kind)).unwrap();
        }
        catalog
            .insert(ReflectTypeTemplate::new("app.Secret", ReflectTypeKind::Opaque).unwrap())
            .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.Unused", ReflectTypeKind::Record)
                    .unwrap()
                    .fields([field("value", "std.Int", 0, true)]),
            )
            .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.User", ReflectTypeKind::Record)
                    .unwrap()
                    .capabilities([
                        ReflectCapability::Share,
                        ReflectCapability::Copy,
                        ReflectCapability::Share,
                    ])
                    .fields([
                        field("name", "std.String", 0, true),
                        field("token", "app.Secret", 1, false),
                    ]),
            )
            .unwrap();
        catalog
    }

    fn root<'a>(metadata: &'a ReflectMetadata, name: &str) -> ReflectTypeInfo<'a> {
        metadata
            .roots()
            .iter()
            .filter_map(|id| metadata.type_info(*id))
            .find(|info| info.qualified_name() == name)
            .unwrap()
    }

    #[test]
    fn explicit_roots_retain_only_the_public_descriptor_closure() {
        let catalog = base_catalog();
        let metadata = ReflectMetadata::link(&hash("artifact"), ["app.User"], &catalog).unwrap();
        assert_eq!(metadata.roots().len(), 1);
        assert_eq!(metadata.retained_len(), 2);
        let user = root(&metadata, "app.User");
        assert_eq!(user.kind(), ReflectTypeKind::Record);
        assert_eq!(
            user.capabilities(),
            [ReflectCapability::Copy, ReflectCapability::Share]
        );
        let fields = user.fields().collect::<Vec<_>>();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name(), "name");
        assert_eq!(fields[0].ordinal(), 0);
        assert_eq!(fields[0].docs(), Some("name docs"));
        assert_eq!(fields[0].ty().qualified_name(), "std.String");
        assert!(user.variants().next().is_none());
        assert!(user.tuple_elements().next().is_none());
        assert!(user.function().is_none());
        assert!(user.generic_arguments().next().is_none());
    }

    #[test]
    fn removed_roots_remove_metadata_and_ids_are_artifact_local() {
        let catalog = base_catalog();
        let empty =
            ReflectMetadata::link(&hash("artifact"), std::iter::empty::<&str>(), &catalog).unwrap();
        assert!(empty.roots().is_empty());
        assert_eq!(empty.retained_len(), 0);

        let first = ReflectMetadata::link(&hash("first"), ["app.User"], &catalog).unwrap();
        let second = ReflectMetadata::link(&hash("second"), ["app.User"], &catalog).unwrap();
        let first_id = first.roots()[0];
        let second_id = second.roots()[0];
        assert_ne!(first_id, second_id);
        assert!(first.type_info(second_id).is_none());
        assert_eq!(first.type_info(first_id).unwrap().id(), first_id);
    }

    #[test]
    fn enums_tuples_generics_newtypes_and_functions_are_compacted_statically() {
        let mut catalog = base_catalog();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.State", ReflectTypeKind::Enum)
                    .unwrap()
                    .variants([
                        ReflectVariantTemplate::new(
                            "Idle",
                            0,
                            None::<String>,
                            ReflectVariantPayloadTemplate::Unit,
                        )
                        .unwrap(),
                        ReflectVariantTemplate::new(
                            "Ready",
                            1,
                            Some("ready docs"),
                            ReflectVariantPayloadTemplate::Tuple(vec!["app.User".into()]),
                        )
                        .unwrap(),
                        ReflectVariantTemplate::new(
                            "Failed",
                            2,
                            None::<String>,
                            ReflectVariantPayloadTemplate::Record(vec![
                                field("message", "std.String", 0, true),
                                field("secret", "app.Secret", 1, false),
                            ]),
                        )
                        .unwrap(),
                    ]),
            )
            .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.Pair", ReflectTypeKind::Tuple)
                    .unwrap()
                    .tuple_elements(["std.Int", "std.String"])
                    .unwrap(),
            )
            .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new(
                    "std.Array[app.User]",
                    ReflectTypeKind::Applied(ReflectAppliedKind::Array),
                )
                .unwrap()
                .generic_arguments(["app.User"])
                .unwrap(),
            )
            .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.UserId", ReflectTypeKind::Newtype)
                    .unwrap()
                    .dependency("std.Int")
                    .unwrap(),
            )
            .unwrap();
        let function = ReflectFunctionTemplate::new(
            [
                ReflectParameterTemplate::new("app.User", ReflectParameterMode::Value).unwrap(),
                ReflectParameterTemplate::new("std.String", ReflectParameterMode::Ref).unwrap(),
                ReflectParameterTemplate::new("std.Int", ReflectParameterMode::Mut).unwrap(),
                ReflectParameterTemplate::new("std.Bool", ReflectParameterMode::Var).unwrap(),
            ],
            "std.Unit",
            true,
            true,
            true,
        )
        .unwrap();
        catalog
            .insert(
                ReflectTypeTemplate::new("app.Callback", ReflectTypeKind::Function)
                    .unwrap()
                    .function(function),
            )
            .unwrap();

        let metadata = ReflectMetadata::link(
            &hash("complete"),
            [
                "app.State",
                "app.Pair",
                "std.Array[app.User]",
                "app.UserId",
                "app.Callback",
            ],
            &catalog,
        )
        .unwrap();
        let state = root(&metadata, "app.State");
        let variants = state.variants().collect::<Vec<_>>();
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0].name(), "Idle");
        assert_eq!(variants[0].ordinal(), 0);
        assert_eq!(variants[1].docs(), Some("ready docs"));
        assert_eq!(
            variants[0].payload().kind(),
            ReflectVariantPayloadKind::Unit
        );
        assert_eq!(
            variants[1]
                .payload()
                .tuple_types()
                .next()
                .unwrap()
                .qualified_name(),
            "app.User"
        );
        let failed_fields = variants[2].payload().record_fields().collect::<Vec<_>>();
        assert_eq!(failed_fields.len(), 1);
        assert_eq!(failed_fields[0].name(), "message");
        assert!(variants[1].payload().record_fields().next().is_none());
        assert!(variants[2].payload().tuple_types().next().is_none());

        let pair = root(&metadata, "app.Pair");
        assert_eq!(pair.kind(), ReflectTypeKind::Tuple);
        assert_eq!(
            pair.tuple_elements()
                .map(|info| info.qualified_name())
                .collect::<Vec<_>>(),
            ["std.Int", "std.String"]
        );
        let array = root(&metadata, "std.Array[app.User]");
        assert_eq!(
            array.generic_arguments().next().unwrap().qualified_name(),
            "app.User"
        );
        assert_eq!(
            root(&metadata, "app.UserId").kind(),
            ReflectTypeKind::Newtype
        );

        let function = root(&metadata, "app.Callback").function().unwrap();
        let parameters = function.parameters().collect::<Vec<_>>();
        assert_eq!(parameters.len(), 4);
        for (index, mode) in [
            ReflectParameterMode::Value,
            ReflectParameterMode::Ref,
            ReflectParameterMode::Mut,
            ReflectParameterMode::Var,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(parameters[index].position(), index as u32);
            assert_eq!(parameters[index].mode(), mode);
            assert!(!parameters[index].ty().qualified_name().is_empty());
        }
        assert_eq!(function.outcome().qualified_name(), "std.Unit");
        assert!(function.variadic());
        assert!(function.asynchronous());
        assert!(function.unsafe_());
    }

    #[test]
    fn every_type_kind_and_capability_has_a_closed_value() {
        let kinds = [
            ReflectTypeKind::Primitive(ReflectPrimitiveKind::Bool),
            ReflectTypeKind::Record,
            ReflectTypeKind::Enum,
            ReflectTypeKind::Newtype,
            ReflectTypeKind::Tuple,
            ReflectTypeKind::Union,
            ReflectTypeKind::Function,
            ReflectTypeKind::Applied(ReflectAppliedKind::Map),
            ReflectTypeKind::Reference(ReflectReferenceKind::Pointer),
            ReflectTypeKind::Opaque,
        ];
        assert_eq!(kinds.len(), 10);
        let primitives = [
            ReflectPrimitiveKind::Bool,
            ReflectPrimitiveKind::Int,
            ReflectPrimitiveKind::Int8,
            ReflectPrimitiveKind::Int16,
            ReflectPrimitiveKind::Int32,
            ReflectPrimitiveKind::UInt8,
            ReflectPrimitiveKind::UInt16,
            ReflectPrimitiveKind::UInt32,
            ReflectPrimitiveKind::UInt64,
            ReflectPrimitiveKind::Float,
            ReflectPrimitiveKind::Float32,
            ReflectPrimitiveKind::Byte,
            ReflectPrimitiveKind::Char,
            ReflectPrimitiveKind::String,
            ReflectPrimitiveKind::Unit,
            ReflectPrimitiveKind::Never,
        ];
        let applied = [
            ReflectAppliedKind::Array,
            ReflectAppliedKind::Map,
            ReflectAppliedKind::Set,
            ReflectAppliedKind::Range,
            ReflectAppliedKind::Option,
            ReflectAppliedKind::Result,
            ReflectAppliedKind::Other,
        ];
        let references = [ReflectReferenceKind::Ref, ReflectReferenceKind::Pointer];
        let capabilities = [
            ReflectCapability::Copy,
            ReflectCapability::Discard,
            ReflectCapability::Equatable,
            ReflectCapability::Key,
            ReflectCapability::Send,
            ReflectCapability::Share,
        ];
        assert_eq!(primitives.len(), 16);
        assert_eq!(applied.len(), 7);
        assert_eq!(references.len(), 2);
        assert_eq!(capabilities.len(), 6);
    }

    #[test]
    fn malformed_catalogs_and_unknown_public_dependencies_fail_before_linking() {
        let mut catalog = ReflectCatalog::default();
        let int = primitive("std.Int", ReflectPrimitiveKind::Int);
        catalog.insert(int.clone()).unwrap();
        assert!(matches!(
            catalog.insert(int),
            Err(ReflectError::DuplicateType(_))
        ));
        assert_eq!(
            ReflectMetadata::link(&hash("kept"), ["std.Int"], &catalog)
                .unwrap()
                .retained_len(),
            1
        );
        assert!(matches!(
            ReflectMetadata::link("bad", ["std.Int"], &catalog),
            Err(ReflectError::InvalidArtifactIdentity)
        ));
        assert!(matches!(
            ReflectMetadata::link(&hash("artifact"), ["missing.Type"], &catalog),
            Err(ReflectError::UnknownType(_))
        ));

        let mut public_unknown = ReflectCatalog::default();
        public_unknown
            .insert(
                ReflectTypeTemplate::new("app.Bad", ReflectTypeKind::Record)
                    .unwrap()
                    .fields([field("missing", "missing.Type", 0, true)]),
            )
            .unwrap();
        assert!(matches!(
            ReflectMetadata::link(&hash("artifact"), ["app.Bad"], &public_unknown),
            Err(ReflectError::UnknownType(_))
        ));

        let mut private_unknown = ReflectCatalog::default();
        private_unknown
            .insert(
                ReflectTypeTemplate::new("app.Safe", ReflectTypeKind::Record)
                    .unwrap()
                    .fields([field("hidden", "missing.Type", 0, false)]),
            )
            .unwrap();
        assert_eq!(
            ReflectMetadata::link(&hash("artifact"), ["app.Safe"], &private_unknown)
                .unwrap()
                .retained_len(),
            1
        );
    }

    #[test]
    fn malformed_shapes_members_and_text_are_rejected() {
        assert!(ReflectTypeTemplate::new("bad\nname", ReflectTypeKind::Opaque).is_err());
        assert!(ReflectFieldTemplate::new("", "std.Int", 0, None::<String>, true).is_err());
        assert!(ReflectFieldTemplate::new("x", "std.Int", 0, Some("bad\ndoc"), true).is_err());
        assert!(ReflectParameterTemplate::new("", ReflectParameterMode::Value).is_err());
        assert!(ReflectFunctionTemplate::new([], "", false, false, false).is_err());
        assert!(
            ReflectTypeTemplate::new("app.Array", ReflectTypeKind::Opaque)
                .unwrap()
                .generic_arguments([""])
                .is_err()
        );
        assert!(
            ReflectTypeTemplate::new("app.Tuple", ReflectTypeKind::Tuple)
                .unwrap()
                .tuple_elements([""])
                .is_err()
        );
        assert!(
            ReflectTypeTemplate::new("app.Newtype", ReflectTypeKind::Newtype)
                .unwrap()
                .dependency("")
                .is_err()
        );

        let mut catalog = ReflectCatalog::default();
        assert!(matches!(
            catalog.insert(ReflectTypeTemplate::new("app.Empty", ReflectTypeKind::Record).unwrap()),
            Err(ReflectError::InvalidShape(_))
        ));
        assert!(matches!(
            catalog.insert(
                ReflectTypeTemplate::new("app.Duplicate", ReflectTypeKind::Record)
                    .unwrap()
                    .fields([
                        field("a", "std.Int", 0, true),
                        field("b", "std.Int", 0, true)
                    ])
            ),
            Err(ReflectError::DuplicateMember(_))
        ));
        assert!(matches!(
            catalog.insert(
                ReflectTypeTemplate::new("app.Wrong", ReflectTypeKind::Opaque)
                    .unwrap()
                    .fields([field("a", "std.Int", 0, true)])
            ),
            Err(ReflectError::InvalidShape(_))
        ));
    }

    #[test]
    fn error_vocabulary_is_stable() {
        let errors = [
            ReflectError::InvalidArtifactIdentity,
            ReflectError::InvalidText("x".into()),
            ReflectError::InvalidShape("x".into()),
            ReflectError::DuplicateType("x".into()),
            ReflectError::DuplicateMember("x".into()),
            ReflectError::UnknownType("x".into()),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
