//! Immutable descriptor tables retained by explicit, statically typed queries.
//! These records describe types; they contain no value or callable access.

use crate::bytecode::BytecodeCallableId;

pub const TYPE_KIND_VARIANTS: &[&str] = &[
    "Primitive",
    "Record",
    "Enum",
    "Newtype",
    "Tuple",
    "Union",
    "Function",
    "Applied",
    "Reference",
    "Opaque",
];
pub const PRIMITIVE_KIND_VARIANTS: &[&str] = &[
    "Bool", "Int", "Int8", "Int16", "Int32", "UInt8", "UInt16", "UInt32", "UInt64", "Float",
    "Float32", "Byte", "Char", "String", "Unit", "Never",
];
pub const APPLIED_KIND_VARIANTS: &[&str] =
    &["Array", "Map", "Set", "Range", "Option", "Result", "Other"];
pub const REFERENCE_KIND_VARIANTS: &[&str] = &["Ref", "Pointer"];
pub const CAPABILITY_VARIANTS: &[&str] = &["Copy", "Discard", "Equatable", "Key", "Send", "Share"];
pub const PARAMETER_MODE_VARIANTS: &[&str] = &["Value", "Ref", "Mut", "Var"];
pub const VARIANT_PAYLOAD_VARIANTS: &[&str] = &["Unit", "Tuple", "Record"];
pub const REFLECTION_ENUMS: &[(&str, &[&str])] = &[
    ("TypeKind", TYPE_KIND_VARIANTS),
    ("PrimitiveKind", PRIMITIVE_KIND_VARIANTS),
    ("AppliedKind", APPLIED_KIND_VARIANTS),
    ("ReferenceKind", REFERENCE_KIND_VARIANTS),
    ("TypeCapability", CAPABILITY_VARIANTS),
    ("ParameterMode", PARAMETER_MODE_VARIANTS),
    ("VariantPayloadKind", VARIANT_PAYLOAD_VARIANTS),
];

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectionDescriptorKind {
    TypeInfo,
    TypeId,
    FieldInfo,
    VariantInfo,
    ParameterInfo,
    FunctionInfo,
}

impl ReflectionDescriptorKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::TypeInfo => "TypeInfo",
            Self::TypeId => "TypeId",
            Self::FieldInfo => "FieldInfo",
            Self::VariantInfo => "VariantInfo",
            Self::ParameterInfo => "ParameterInfo",
            Self::FunctionInfo => "FunctionInfo",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectionOperation {
    TypeInfo,
    Id,
    QualifiedName,
    Kind,
    GenericArguments,
    Capabilities,
    Fields,
    Variants,
    TupleElements,
    Function,
    FieldName,
    FieldType,
    FieldOrdinal,
    FieldDocs,
    VariantName,
    VariantOrdinal,
    VariantPayloadKind,
    VariantTupleElements,
    VariantFields,
    ParameterPosition,
    ParameterType,
    ParameterMode,
    FunctionParameters,
    FunctionOutcome,
    FunctionVariadic,
    FunctionSuspends,
    FunctionUnsafe,
}

impl ReflectionOperation {
    pub const QUERIES: [Self; 26] = [
        Self::Id,
        Self::QualifiedName,
        Self::Kind,
        Self::GenericArguments,
        Self::Capabilities,
        Self::Fields,
        Self::Variants,
        Self::TupleElements,
        Self::Function,
        Self::FieldName,
        Self::FieldType,
        Self::FieldOrdinal,
        Self::FieldDocs,
        Self::VariantName,
        Self::VariantOrdinal,
        Self::VariantPayloadKind,
        Self::VariantTupleElements,
        Self::VariantFields,
        Self::ParameterPosition,
        Self::ParameterType,
        Self::ParameterMode,
        Self::FunctionParameters,
        Self::FunctionOutcome,
        Self::FunctionVariadic,
        Self::FunctionSuspends,
        Self::FunctionUnsafe,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::TypeInfo => "std.reflect.typeInfo",
            Self::Id => "std.reflect.TypeInfo.id",
            Self::QualifiedName => "std.reflect.TypeInfo.qualifiedName",
            Self::Kind => "std.reflect.TypeInfo.kind",
            Self::GenericArguments => "std.reflect.TypeInfo.genericArguments",
            Self::Capabilities => "std.reflect.TypeInfo.capabilities",
            Self::Fields => "std.reflect.TypeInfo.fields",
            Self::Variants => "std.reflect.TypeInfo.variants",
            Self::TupleElements => "std.reflect.TypeInfo.tupleElements",
            Self::Function => "std.reflect.TypeInfo.function",
            Self::FieldName => "std.reflect.FieldInfo.name",
            Self::FieldType => "std.reflect.FieldInfo.typeInfo",
            Self::FieldOrdinal => "std.reflect.FieldInfo.ordinal",
            Self::FieldDocs => "std.reflect.FieldInfo.docs",
            Self::VariantName => "std.reflect.VariantInfo.name",
            Self::VariantOrdinal => "std.reflect.VariantInfo.ordinal",
            Self::VariantPayloadKind => "std.reflect.VariantInfo.payloadKind",
            Self::VariantTupleElements => "std.reflect.VariantInfo.tupleElements",
            Self::VariantFields => "std.reflect.VariantInfo.fields",
            Self::ParameterPosition => "std.reflect.ParameterInfo.position",
            Self::ParameterType => "std.reflect.ParameterInfo.typeInfo",
            Self::ParameterMode => "std.reflect.ParameterInfo.mode",
            Self::FunctionParameters => "std.reflect.FunctionInfo.parameters",
            Self::FunctionOutcome => "std.reflect.FunctionInfo.outcome",
            Self::FunctionVariadic => "std.reflect.FunctionInfo.variadic",
            Self::FunctionSuspends => "std.reflect.FunctionInfo.suspends",
            Self::FunctionUnsafe => "std.reflect.FunctionInfo.isUnsafe",
        }
    }

    pub const fn receiver(self) -> Option<ReflectionDescriptorKind> {
        use ReflectionDescriptorKind as D;
        Some(match self {
            Self::TypeInfo => return None,
            Self::Id
            | Self::QualifiedName
            | Self::Kind
            | Self::GenericArguments
            | Self::Capabilities
            | Self::Fields
            | Self::Variants
            | Self::TupleElements
            | Self::Function => D::TypeInfo,
            Self::FieldName | Self::FieldType | Self::FieldOrdinal | Self::FieldDocs => {
                D::FieldInfo
            }
            Self::VariantName
            | Self::VariantOrdinal
            | Self::VariantPayloadKind
            | Self::VariantTupleElements
            | Self::VariantFields => D::VariantInfo,
            Self::ParameterPosition | Self::ParameterType | Self::ParameterMode => D::ParameterInfo,
            Self::FunctionParameters
            | Self::FunctionOutcome
            | Self::FunctionVariadic
            | Self::FunctionSuspends
            | Self::FunctionUnsafe => D::FunctionInfo,
        })
    }

    pub fn query(receiver: ReflectionDescriptorKind, name: &str) -> Option<Self> {
        Self::QUERIES.into_iter().find(|operation| {
            operation.receiver() == Some(receiver)
                && operation.name().rsplit('.').next() == Some(name)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionTable {
    pub artifact_tag: [u8; 32],
    pub calls: Vec<ReflectionCall>,
    pub types: Vec<ReflectionTypeRecord>,
    pub fields: Vec<ReflectionFieldRecord>,
    pub variants: Vec<ReflectionVariantRecord>,
    pub parameters: Vec<ReflectionParameterRecord>,
    pub functions: Vec<ReflectionFunctionRecord>,
}

impl ReflectionTable {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionCall {
    pub callable: BytecodeCallableId,
    pub operation: ReflectionOperation,
    pub root: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionTypeRecord {
    pub source_type: crate::bytecode::BytecodeTypeId,
    pub qualified_name: String,
    pub kind: ReflectTypeKind,
    pub generic_arguments: Vec<u32>,
    pub capabilities: Vec<ReflectCapability>,
    pub fields: Vec<u32>,
    pub variants: Vec<u32>,
    pub tuple_elements: Vec<u32>,
    pub function: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionFieldRecord {
    pub name: String,
    pub ty: u32,
    pub ordinal: u32,
    pub docs: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionVariantRecord {
    pub name: String,
    pub ordinal: u32,
    pub payload_kind: ReflectVariantPayloadKind,
    pub tuple_types: Vec<u32>,
    pub record_fields: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionParameterRecord {
    pub position: u32,
    pub ty: u32,
    pub mode: ReflectParameterMode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflectionFunctionRecord {
    pub parameters: Vec<u32>,
    pub outcome: u32,
    pub variadic: bool,
    pub suspends: bool,
    pub unsafe_: bool,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectAppliedKind {
    Array,
    Map,
    Set,
    Range,
    Option,
    Result,
    Other,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectReferenceKind {
    Ref,
    Pointer,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectCapability {
    Copy,
    Discard,
    Equatable,
    Key,
    Send,
    Share,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum ReflectParameterMode {
    Value,
    Ref,
    Mut,
    Var,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReflectVariantPayloadKind {
    Unit,
    Tuple,
    Record,
}
