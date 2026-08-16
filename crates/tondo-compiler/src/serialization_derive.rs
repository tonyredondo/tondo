//! Hermetic build-only providers for `std.serialization`.
//!
//! The provider consumes only the sealed semantic snapshot handed to the
//! derive boundary.  It emits ordinary Tondo source; no runtime reflection,
//! callbacks, filesystem access, or ambient state is involved.

use std::collections::BTreeSet;
use std::fmt::{self, Write};
use std::sync::Arc;

use crate::meta::{
    MetaDeclaration, MetaDeclarationKind, MetaField, MetaVariant, MetaVariantPayload,
};
use crate::meta_derive::{
    DeriveExecutionError, DeriveProviderCompiler, DeriveProviderRegistry, DeriveProviderRequest,
};
use crate::meta_test_support::string_artifact;
use crate::meta_vm::MetaVmArtifact;

pub const SERIALIZE_TRAIT: &str = "serialization.Serialize";
pub const DESERIALIZE_TRAIT: &str = "serialization.Deserialize";
pub const SERIALIZE_PROVIDER: &str = "std.derive.serialization.Serialize";
pub const DESERIALIZE_PROVIDER: &str = "std.derive.serialization.Deserialize";
/// Canonical 0.1 ABI identities.  The legacy names remain registered as
/// compatibility providers until all downstream source has migrated.
pub const ENCODE_TRAIT: &str = "serialization.Encode";
pub const DECODE_TRAIT: &str = "serialization.Decode";
pub const ENCODE_PROVIDER: &str = "std.derive.serialization.Encode";
pub const DECODE_PROVIDER: &str = "std.derive.serialization.Decode";

const SERIALIZER_TRAIT: &str = "serialization.Serializer";
const DESERIALIZER_TRAIT: &str = "serialization.Deserializer";
const EVENT_TYPE: &str = "serialization.SerializationEvent";
const ERROR_TYPE: &str = "serialization.SerializationError";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationDirection {
    Serialize,
    Deserialize,
    Encode,
    Decode,
}

impl SerializationDirection {
    pub const fn trait_identity(self) -> &'static str {
        match self {
            Self::Serialize => SERIALIZE_TRAIT,
            Self::Deserialize => DESERIALIZE_TRAIT,
            Self::Encode => ENCODE_TRAIT,
            Self::Decode => DECODE_TRAIT,
        }
    }

    pub const fn provider_identity(self) -> &'static str {
        match self {
            Self::Serialize => SERIALIZE_PROVIDER,
            Self::Deserialize => DESERIALIZE_PROVIDER,
            Self::Encode => ENCODE_PROVIDER,
            Self::Decode => DECODE_PROVIDER,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerializationDeriveProvider {
    direction: SerializationDirection,
}

impl SerializationDeriveProvider {
    pub const fn new(direction: SerializationDirection) -> Self {
        Self { direction }
    }

    pub const fn direction(self) -> SerializationDirection {
        self.direction
    }
}

/// Register both standard serialization providers in the locked registry.
pub fn register_serialization_providers(
    registry: &mut DeriveProviderRegistry,
) -> Result<(), DeriveExecutionError> {
    registry.insert(
        SERIALIZE_PROVIDER,
        Arc::new(SerializationDeriveProvider::new(
            SerializationDirection::Serialize,
        )),
    )?;
    registry.insert(
        DESERIALIZE_PROVIDER,
        Arc::new(SerializationDeriveProvider::new(
            SerializationDirection::Deserialize,
        )),
    )?;
    registry.insert(
        ENCODE_PROVIDER,
        Arc::new(SerializationDeriveProvider::new(
            SerializationDirection::Encode,
        )),
    )?;
    registry.insert(
        DECODE_PROVIDER,
        Arc::new(SerializationDeriveProvider::new(
            SerializationDirection::Decode,
        )),
    )?;
    Ok(())
}

impl DeriveProviderCompiler for SerializationDeriveProvider {
    fn compile(&self, request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
        let body = render_serialization_body(&request, self.direction)
            .map_err(|error| error.to_string())?;
        Ok(string_artifact(&body))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializationDeriveError {
    TraitMismatch {
        expected: String,
        found: String,
    },
    ProviderMismatch {
        expected: String,
        found: String,
    },
    TargetMissing {
        module: String,
        target: String,
    },
    UnsupportedTargetKind(String),
    MissingGenericBound(String),
    InvalidMemberName(String),
    InvalidCodec(String),
    InvalidAttribute {
        name: String,
        argument: Option<String>,
    },
    AttributeCodecMismatch {
        name: String,
        codec: String,
    },
    IgnoredFieldRequiresOption(String),
    JsonBase64RequiresBytes(String),
    InvalidProtoNumber(String),
    DuplicateWireField(String),
}

impl fmt::Display for SerializationDeriveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TraitMismatch { expected, found } => {
                write!(
                    formatter,
                    "serialization derive expected trait `{expected}`, found `{found}`"
                )
            }
            Self::ProviderMismatch { expected, found } => {
                write!(
                    formatter,
                    "serialization derive expected provider `{expected}`, found `{found}`"
                )
            }
            Self::TargetMissing { module, target } => {
                write!(
                    formatter,
                    "serialization target `{module}::{target}` is absent from the sealed snapshot"
                )
            }
            Self::UnsupportedTargetKind(kind) => {
                write!(
                    formatter,
                    "serialization derive does not support target kind `{kind}`"
                )
            }
            Self::MissingGenericBound(parameter) => {
                write!(
                    formatter,
                    "serialization derive requires a bound for generic `{parameter}`"
                )
            }
            Self::InvalidMemberName(name) => {
                write!(
                    formatter,
                    "serialization derive cannot use member name `{name}`"
                )
            }
            Self::InvalidCodec(codec) => {
                write!(formatter, "serialization derive cannot use codec `{codec}`")
            }
            Self::InvalidAttribute { name, argument } => {
                write!(
                    formatter,
                    "serialization derive does not support attribute `@{name}`"
                )?;
                if let Some(argument) = argument {
                    write!(formatter, "({argument})")?;
                }
                Ok(())
            }
            Self::AttributeCodecMismatch { name, codec } => {
                write!(
                    formatter,
                    "serialization attribute `@{name}` is not valid for codec `{codec}`"
                )
            }
            Self::IgnoredFieldRequiresOption(name) => write!(
                formatter,
                "ignored serialization field `{name}` must have an Option type"
            ),
            Self::JsonBase64RequiresBytes(name) => {
                write!(formatter, "@json(base64) requires Bytes field `{name}`")
            }
            Self::InvalidProtoNumber(value) => {
                write!(formatter, "invalid @proto field number `{value}`")
            }
            Self::DuplicateWireField(name) => {
                write!(
                    formatter,
                    "serialization wire field `{name}` is declared more than once"
                )
            }
        }
    }
}

impl std::error::Error for SerializationDeriveError {}

/// Render one deterministic impl body.  The caller still wraps and parses it
/// through the normal generated-source validation boundary.
pub fn render_serialization_body(
    request: &DeriveProviderRequest<'_>,
    direction: SerializationDirection,
) -> Result<String, SerializationDeriveError> {
    let codec = codec_for_request(request.trait_identity(), direction)?;
    if request.provider_identity() != direction.provider_identity() {
        return Err(SerializationDeriveError::ProviderMismatch {
            expected: direction.provider_identity().into(),
            found: request.provider_identity().into(),
        });
    }
    let declaration = request
        .snapshot()
        .declarations()
        .iter()
        .find(|declaration| {
            declaration.module() == request.module() && declaration.identity() == request.target()
        })
        .ok_or_else(|| SerializationDeriveError::TargetMissing {
            module: request.module().into(),
            target: request.target().into(),
        })?;
    let target_type = target_type(declaration);
    // Validate the generic contract inside the hermetic provider.  The
    // execution boundary owns the final impl header so every provider emits
    // only an ordinary impl body.
    let _ = generic_header(declaration, request, direction)?;
    let mut output = String::new();
    match direction {
        SerializationDirection::Serialize => {
            render_serialize_impl(&mut output, declaration)?;
        }
        SerializationDirection::Deserialize => {
            render_deserialize_impl(&mut output, declaration, &target_type)?;
        }
        SerializationDirection::Encode => {
            render_encode_impl(&mut output, declaration, &target_type, &codec)?;
        }
        SerializationDirection::Decode => {
            render_decode_impl(&mut output, declaration, &target_type, &codec)?;
        }
    }
    if codec != "C" {
        output = output
            .replace(
                "serialization.Encoder[C, E]",
                &format!("serialization.Encoder[{codec}, E]"),
            )
            .replace(
                "serialization.Decoder[C, E]",
                &format!("serialization.Decoder[{codec}, E]"),
            )
            .replace(
                "serialization.Encode[C]",
                &format!("serialization.Encode[{codec}]"),
            )
            .replace(
                "serialization.Decode[C]",
                &format!("serialization.Decode[{codec}]"),
            );
    }
    Ok(output)
}

fn codec_for_request(
    identity: &str,
    direction: SerializationDirection,
) -> Result<String, SerializationDeriveError> {
    if !matches!(
        direction,
        SerializationDirection::Encode | SerializationDirection::Decode
    ) {
        return Ok("C".into());
    }
    let base = direction.trait_identity();
    if identity == base {
        return Ok("C".into());
    }
    let prefix = format!("{base}[");
    let Some(codec) = identity
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix(']'))
    else {
        return Err(SerializationDeriveError::TraitMismatch {
            expected: base.into(),
            found: identity.into(),
        });
    };
    if codec.is_empty()
        || codec.split('.').any(|part| {
            part.is_empty()
                || part.chars().enumerate().any(|(index, character)| {
                    !(character == '_' || character.is_alphanumeric())
                        || (index == 0 && !character.is_alphabetic() && character != '_')
                })
        })
    {
        return Err(SerializationDeriveError::InvalidCodec(codec.into()));
    }
    Ok(codec.into())
}

fn target_type(declaration: &MetaDeclaration) -> String {
    if declaration.generic_parameters().is_empty() {
        declaration.identity().to_owned()
    } else {
        format!(
            "{}[{}]",
            declaration.identity(),
            declaration
                .generic_parameters()
                .iter()
                .map(|parameter| parameter.name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn generic_header(
    declaration: &MetaDeclaration,
    request: &DeriveProviderRequest<'_>,
    _direction: SerializationDirection,
) -> Result<String, SerializationDeriveError> {
    if declaration.generic_parameters().is_empty() {
        return Ok(String::new());
    }
    let mut binders = Vec::new();
    for parameter in declaration.generic_parameters() {
        let mut bounds = parameter.bounds().to_vec();
        if declaration_uses_parameter(declaration, parameter.name())
            && !bounds.iter().any(|bound| bound == request.trait_identity())
        {
            if !request
                .introduced_bounds()
                .iter()
                .any(|bound| bound == parameter.name())
            {
                return Err(SerializationDeriveError::MissingGenericBound(
                    parameter.name().into(),
                ));
            }
            bounds.push(request.trait_identity().into());
        }
        bounds.sort();
        bounds.dedup();
        if bounds.is_empty() {
            binders.push(parameter.name().to_owned());
        } else {
            binders.push(format!("{}: {}", parameter.name(), bounds.join(" + ")));
        }
    }
    Ok(format!("[{}]", binders.join(", ")))
}

fn declaration_uses_parameter(declaration: &MetaDeclaration, parameter: &str) -> bool {
    let uses = |ty: &str| type_mentions_parameter(ty, parameter);
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => fields.iter().any(|field| uses(field.ty())),
        MetaDeclarationKind::Enum(variants) => variants.iter().any(|variant| {
            variant_payload_types(variant)
                .into_iter()
                .any(|ty| uses(&ty))
        }),
        MetaDeclarationKind::Newtype(underlying) => uses(underlying),
        MetaDeclarationKind::Trait(_) => false,
    }
}

fn variant_payload_types(variant: &MetaVariant) -> Vec<String> {
    match variant.payload() {
        MetaVariantPayload::Unit => Vec::new(),
        MetaVariantPayload::Tuple(types) => types.clone(),
        MetaVariantPayload::Record(fields) => {
            fields.iter().map(|field| field.ty().to_owned()).collect()
        }
    }
}

fn type_mentions_parameter(ty: &str, parameter: &str) -> bool {
    ty.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|part| part == parameter)
}

fn render_serialize_impl(
    output: &mut String,
    declaration: &MetaDeclaration,
) -> Result<(), SerializationDeriveError> {
    line(output, 0, "{");
    writeln!(
        output,
        "    fn serialize[E, S: {}[E]](self, serializer: var S): Unit ! E {{",
        SERIALIZER_TRAIT
    )
    .expect("writing to String cannot fail");
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => {
            render_record_encode(output, declaration, fields, "self")?
        }
        MetaDeclarationKind::Enum(variants) => render_enum_encode(output, declaration, variants)?,
        MetaDeclarationKind::Newtype(_) => {
            line(
                output,
                2,
                "serialization.Serialize.serialize(self.value, var serializer)?",
            );
        }
        MetaDeclarationKind::Trait(_) => {
            return Err(SerializationDeriveError::UnsupportedTargetKind(
                declaration.kind().name().into(),
            ));
        }
    }
    line(output, 1, "}");
    line(output, 0, "}");
    Ok(())
}

/// Render the canonical `Encode[C]` implementation.  The codec parameter is
/// intentionally left on the trait method's `Encoder[C, E]` bound: one derive
/// expansion is therefore usable by JSON, MessagePack, Protobuf, and future
/// codecs without a runtime registry or a second generated copy.
fn render_encode_impl(
    output: &mut String,
    declaration: &MetaDeclaration,
    target_type: &str,
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 0, "{");
    writeln!(
        output,
        "    fn encode[E, S: serialization.Encoder[C, E]](value: {target_type}, encoder: var S): Unit ! E {{"
    )
    .expect("writing to String cannot fail");
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => {
            render_record_encode_static(output, declaration, fields, target_type, codec)?
        }
        MetaDeclarationKind::Enum(variants) => {
            if codec == "Json" {
                render_json_enum_encode_static(output, declaration, variants, "value")?
            } else if codec == "MessagePack" {
                render_messagepack_enum_encode_static(output, declaration, variants, "value")?
            } else {
                render_enum_encode_static(output, declaration, variants, "value", codec)?
            }
        }
        MetaDeclarationKind::Newtype(_) => {
            line(output, 2, &format!("let {target_type}(inner) = value"));
            line(
                output,
                2,
                "serialization.Encode[C].encode[E, S](inner, var encoder)?",
            );
        }
        MetaDeclarationKind::Trait(_) => {
            return Err(SerializationDeriveError::UnsupportedTargetKind(
                declaration.kind().name().into(),
            ));
        }
    }
    line(output, 1, "}");
    line(output, 0, "}");
    Ok(())
}

fn render_record_encode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    fields: &[MetaField],
    target_type: &str,
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    let fields = field_policies(fields, codec)?;
    let bindings = fields
        .iter()
        .enumerate()
        .filter(|(_, (_, policy))| !policy.ignored)
        .map(|(index, (field, _))| (field.name(), format!("__tondo_field_{index}")))
        .collect::<Vec<_>>();
    let mut pattern_fields = bindings
        .iter()
        .map(|(field, binding)| format!("{field}: {binding}"))
        .collect::<Vec<_>>();
    for (field, policy) in &fields {
        if policy.ignored {
            pattern_fields.push(format!("{}: _", field.name()));
        }
    }
    line(
        output,
        2,
        &format!(
            "let {target_type} {{ {} }} = value",
            pattern_fields.join(", ")
        ),
    );
    if codec == "MessagePack" {
        line(
            output,
            2,
            &format!(
                "encoder.startMap({})?",
                fields.iter().filter(|(_, policy)| !policy.ignored).count()
            ),
        );
        for ((_, policy), (_, binding)) in fields
            .iter()
            .filter(|(_, policy)| !policy.ignored)
            .zip(&bindings)
        {
            line(output, 2, "encoder.mapKey()?");
            line(
                output,
                2,
                &format!("encoder.string({})?", quote(&policy.event_name(codec))),
            );
            render_static_field_encode(output, 2, policy, binding, codec);
        }
        line(output, 2, "encoder.endMap()?");
        return Ok(());
    }
    line(
        output,
        2,
        &format!(
            "encoder.startRecord({}, {})?",
            quote(declaration.identity()),
            fields.iter().filter(|(_, policy)| !policy.ignored).count()
        ),
    );
    for ((field, policy), (_, binding)) in fields
        .into_iter()
        .filter(|(_, policy)| !policy.ignored)
        .zip(bindings)
    {
        member_name(field.name())?;
        line(
            output,
            2,
            &format!("encoder.field({})?", quote(&policy.event_name(codec))),
        );
        render_static_field_encode(output, 2, &policy, &binding, codec);
    }
    line(output, 2, "encoder.endRecord()?");
    Ok(())
}

fn render_static_field_encode(
    output: &mut String,
    indent: usize,
    policy: &FieldPolicy,
    binding: &str,
    codec: &str,
) {
    if codec == "Json" && policy.json_base64 {
        line(output, indent, &format!("encoder.base64({binding})?"));
    } else if codec == "MessagePack" && policy.messagepack_binary {
        line(output, indent, &format!("encoder.bytes({binding})?"));
    } else {
        line(
            output,
            indent,
            &format!(
                "serialization.Encode[C].encode[E, S]({}, var encoder)?",
                binding
            ),
        );
    }
}

fn render_enum_encode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
    receiver: &str,
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 2, &format!("match {receiver} {{"));
    for variant in variants {
        member_name(variant.name())?;
        let path = format!("{}.{}", declaration.identity(), variant.name());
        match variant.payload() {
            MetaVariantPayload::Unit => {
                line(output, 3, &format!("{} => {{", path));
                render_enum_start_end_static(output, declaration, variant, 4, None, codec)?;
                line(output, 3, "}");
            }
            MetaVariantPayload::Tuple(types) => {
                let names = (0..types.len())
                    .map(|index| format!("value_{index}"))
                    .collect::<Vec<_>>();
                line(output, 3, &format!("{}({}) => {{", path, names.join(", ")));
                render_enum_start_end_static(output, declaration, variant, 4, Some(&names), codec)?;
                line(output, 3, "}");
            }
            MetaVariantPayload::Record(fields) => {
                for field in fields {
                    member_name(field.name())?;
                }
                let bindings = fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| (field.name(), format!("__tondo_variant_field_{index}")))
                    .collect::<Vec<_>>();
                line(
                    output,
                    3,
                    &format!(
                        "{} {{ {} }} => {{",
                        path,
                        bindings
                            .iter()
                            .map(|(field, binding)| format!("{field}: {binding}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
                let names = bindings
                    .into_iter()
                    .map(|(_, binding)| binding)
                    .collect::<Vec<_>>();
                render_enum_start_end_static(output, declaration, variant, 4, Some(&names), codec)?;
                line(output, 3, "}");
            }
        }
    }
    line(output, 2, "}");
    Ok(())
}

fn render_json_enum_encode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
    receiver: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 2, &format!("match {receiver} {{"));
    for variant in variants {
        member_name(variant.name())?;
        let path = format!("{}.{}", declaration.identity(), variant.name());
        match variant.payload() {
            MetaVariantPayload::Unit => {
                line(output, 3, &format!("{path} => {{"));
                render_json_enum_prefix(output, variant.name(), 4);
                line(output, 4, "encoder.null()?");
                line(output, 4, "encoder.endRecord()?");
                line(output, 3, "}");
            }
            MetaVariantPayload::Tuple(types) => {
                let names = (0..types.len())
                    .map(|index| format!("value_{index}"))
                    .collect::<Vec<_>>();
                line(output, 3, &format!("{path}({}) => {{", names.join(", ")));
                render_json_enum_prefix(output, variant.name(), 4);
                line(output, 4, &format!("encoder.startArray({})?", names.len()));
                for name in &names {
                    line(
                        output,
                        4,
                        &format!("serialization.Encode[C].encode[E, S]({name}, var encoder)?"),
                    );
                }
                line(output, 4, "encoder.endArray()?");
                line(output, 4, "encoder.endRecord()?");
                line(output, 3, "}");
            }
            MetaVariantPayload::Record(fields) => {
                let fields = field_policies(fields, "Json")?;
                let bindings = fields
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, policy))| !policy.ignored)
                    .map(|(index, (field, _))| {
                        (field.name(), format!("__tondo_variant_field_{index}"))
                    })
                    .collect::<Vec<_>>();
                let mut pattern_fields = bindings
                    .iter()
                    .map(|(field, binding)| format!("{field}: {binding}"))
                    .collect::<Vec<_>>();
                for (field, policy) in &fields {
                    if policy.ignored {
                        pattern_fields.push(format!("{}: _", field.name()));
                    }
                }
                line(
                    output,
                    3,
                    &format!("{path} {{ {} }} => {{", pattern_fields.join(", ")),
                );
                render_json_enum_prefix(output, variant.name(), 4);
                line(
                    output,
                    4,
                    &format!(
                        "encoder.startRecord({}, {})?",
                        quote(declaration.identity()),
                        fields.iter().filter(|(_, policy)| !policy.ignored).count()
                    ),
                );
                for ((_, policy), (_, binding)) in fields
                    .iter()
                    .filter(|(_, policy)| !policy.ignored)
                    .zip(&bindings)
                {
                    line(
                        output,
                        4,
                        &format!("encoder.field({})?", quote(&policy.event_name("Json"))),
                    );
                    render_static_field_encode(output, 4, policy, binding, "Json");
                }
                line(output, 4, "encoder.endRecord()?");
                line(output, 4, "encoder.endRecord()?");
                line(output, 3, "}");
            }
        }
    }
    line(output, 2, "}");
    Ok(())
}

/// MessagePack has no record/enum event vocabulary of its own.  Static derives
/// use the natural wire representation: an externally tagged map for enums
/// and string-keyed maps for record payloads.  Generic maps remain unchanged,
/// so a typed map never depends on reflection or on a runtime shape flag.
fn render_messagepack_enum_encode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
    receiver: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 2, &format!("match {receiver} {{"));
    for variant in variants {
        member_name(variant.name())?;
        let path = format!("{}.{}", declaration.identity(), variant.name());
        match variant.payload() {
            MetaVariantPayload::Unit => {
                line(output, 3, &format!("{path} => {{"));
                line(output, 4, "encoder.startMap(1)?");
                line(output, 4, "encoder.mapKey()?");
                line(
                    output,
                    4,
                    &format!("encoder.string({})?", quote(variant.name())),
                );
                line(output, 4, "encoder.null()?");
                line(output, 4, "encoder.endMap()?");
                line(output, 3, "}");
            }
            MetaVariantPayload::Tuple(types) => {
                let names = (0..types.len())
                    .map(|index| format!("value_{index}"))
                    .collect::<Vec<_>>();
                line(output, 3, &format!("{path}({}) => {{", names.join(", ")));
                line(output, 4, "encoder.startMap(1)?");
                line(output, 4, "encoder.mapKey()?");
                line(
                    output,
                    4,
                    &format!("encoder.string({})?", quote(variant.name())),
                );
                line(output, 4, &format!("encoder.startArray({})?", names.len()));
                for name in &names {
                    line(
                        output,
                        4,
                        &format!("serialization.Encode[C].encode[E, S]({name}, var encoder)?"),
                    );
                }
                line(output, 4, "encoder.endArray()?");
                line(output, 4, "encoder.endMap()?");
                line(output, 3, "}");
            }
            MetaVariantPayload::Record(fields) => {
                let fields = field_policies(fields, "MessagePack")?;
                let bindings = fields
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, policy))| !policy.ignored)
                    .map(|(index, (field, _))| {
                        (field.name(), format!("__tondo_variant_field_{index}"))
                    })
                    .collect::<Vec<_>>();
                let mut pattern_fields = bindings
                    .iter()
                    .map(|(field, binding)| format!("{field}: {binding}"))
                    .collect::<Vec<_>>();
                for (field, policy) in &fields {
                    if policy.ignored {
                        pattern_fields.push(format!("{}: _", field.name()));
                    }
                }
                line(
                    output,
                    3,
                    &format!("{path} {{ {} }} => {{", pattern_fields.join(", ")),
                );
                line(output, 4, "encoder.startMap(1)?");
                line(output, 4, "encoder.mapKey()?");
                line(
                    output,
                    4,
                    &format!("encoder.string({})?", quote(variant.name())),
                );
                line(
                    output,
                    4,
                    &format!(
                        "encoder.startMap({})?",
                        fields.iter().filter(|(_, policy)| !policy.ignored).count()
                    ),
                );
                for ((_, policy), (_, binding)) in fields
                    .iter()
                    .filter(|(_, policy)| !policy.ignored)
                    .zip(&bindings)
                {
                    line(output, 4, "encoder.mapKey()?");
                    line(
                        output,
                        4,
                        &format!(
                            "encoder.string({})?",
                            quote(&policy.event_name("MessagePack"))
                        ),
                    );
                    render_static_field_encode(output, 4, policy, binding, "MessagePack");
                }
                line(output, 4, "encoder.endMap()?");
                line(output, 4, "encoder.endMap()?");
                line(output, 3, "}");
            }
        }
    }
    line(output, 2, "}");
    Ok(())
}

fn render_json_enum_prefix(output: &mut String, variant: &str, indent: usize) {
    line(output, indent, "encoder.startRecord(\"\", 1)?");
    line(
        output,
        indent,
        &format!("encoder.field({})?", quote(variant)),
    );
}

fn render_enum_start_end_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variant: &MetaVariant,
    indent: usize,
    payload_names: Option<&[String]>,
    _codec: &str,
) -> Result<(), SerializationDeriveError> {
    line(
        output,
        indent,
        &format!(
            "encoder.startEnum({}, {})?",
            quote(declaration.identity()),
            quote(variant.name())
        ),
    );
    if let Some(names) = payload_names {
        for name in names {
            line(
                output,
                indent,
                &format!(
                    "serialization.Encode[C].encode[E, S]({}, var encoder)?",
                    name
                ),
            );
        }
    }
    line(output, indent, "encoder.endEnum()?");
    Ok(())
}

fn render_record_encode(
    output: &mut String,
    declaration: &MetaDeclaration,
    fields: &[MetaField],
    receiver: &str,
) -> Result<(), SerializationDeriveError> {
    line(
        output,
        2,
        &format!(
            "serializer.startRecord({}, {})?",
            quote(declaration.identity()),
            fields.len()
        ),
    );
    for field in fields {
        member_name(field.name())?;
        line(
            output,
            2,
            &format!("serializer.field({})?", quote(field.name())),
        );
        line(
            output,
            2,
            &format!(
                "serialization.Serialize.serialize({}.{}, var serializer)?",
                receiver,
                field.name()
            ),
        );
    }
    line(output, 2, "serializer.endRecord()?");
    Ok(())
}

fn render_enum_encode(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
) -> Result<(), SerializationDeriveError> {
    line(output, 2, "match self {");
    for variant in variants {
        member_name(variant.name())?;
        let path = format!("{}.{}", declaration.identity(), variant.name());
        match variant.payload() {
            MetaVariantPayload::Unit => {
                line(output, 3, &format!("{} => {{", path));
                render_enum_start_end(output, declaration, variant, 4, None)?;
                line(output, 3, "}");
            }
            MetaVariantPayload::Tuple(types) => {
                let names = (0..types.len())
                    .map(|index| format!("value_{index}"))
                    .collect::<Vec<_>>();
                line(output, 3, &format!("{}({}) => {{", path, names.join(", ")));
                render_enum_start_end(output, declaration, variant, 4, Some(&names))?;
                line(output, 3, "}");
            }
            MetaVariantPayload::Record(fields) => {
                for field in fields {
                    member_name(field.name())?;
                }
                let names = fields
                    .iter()
                    .map(|field| field.name().to_owned())
                    .collect::<Vec<_>>();
                line(
                    output,
                    3,
                    &format!("{} {{ {} }} => {{", path, names.join(", ")),
                );
                render_enum_start_end(output, declaration, variant, 4, Some(&names))?;
                line(output, 3, "}");
            }
        }
    }
    line(output, 2, "}");
    Ok(())
}

fn render_enum_start_end(
    output: &mut String,
    declaration: &MetaDeclaration,
    variant: &MetaVariant,
    indent: usize,
    payload_names: Option<&[String]>,
) -> Result<(), SerializationDeriveError> {
    line(
        output,
        indent,
        &format!(
            "serializer.startEnum({}, {})?",
            quote(declaration.identity()),
            quote(variant.name())
        ),
    );
    if let Some(names) = payload_names {
        for name in names {
            line(
                output,
                indent,
                &format!(
                    "serialization.Serialize.serialize({}, var serializer)?",
                    name
                ),
            );
        }
    }
    line(output, indent, "serializer.endEnum()?");
    Ok(())
}

fn render_deserialize_impl(
    output: &mut String,
    declaration: &MetaDeclaration,
    target_type: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 0, "{");
    writeln!(
        output,
        "    fn deserialize[E, D: {}[E]](deserializer: var D): {} ! E {{",
        DESERIALIZER_TRAIT, target_type
    )
    .expect("writing to String cannot fail");
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => render_record_decode(output, declaration, fields)?,
        MetaDeclarationKind::Enum(variants) => render_enum_decode(output, declaration, variants)?,
        MetaDeclarationKind::Newtype(underlying) => {
            line(
                output,
                2,
                &format!(
                    "let value: {} = serialization.Deserialize.deserialize(var deserializer)?",
                    underlying
                ),
            );
            line(output, 2, &format!("{}(value)", declaration.identity()));
        }
        MetaDeclarationKind::Trait(_) => {
            return Err(SerializationDeriveError::UnsupportedTargetKind(
                declaration.kind().name().into(),
            ));
        }
    }
    line(output, 1, "}");
    line(output, 0, "}");
    Ok(())
}

fn render_decode_impl(
    output: &mut String,
    declaration: &MetaDeclaration,
    target_type: &str,
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 0, "{");
    writeln!(
        output,
        "    fn decode[E, D: serialization.Decoder[C, E]](decoder: var D): {} ! E {{",
        target_type
    )
    .expect("writing to String cannot fail");
    match declaration.kind() {
        MetaDeclarationKind::Record(fields) => {
            render_record_decode_static(output, declaration, fields, codec)?
        }
        MetaDeclarationKind::Enum(variants) => {
            if codec == "Json" {
                render_json_enum_decode_static(output, declaration, variants)?
            } else if codec == "MessagePack" {
                render_messagepack_enum_decode_static(output, declaration, variants)?
            } else {
                render_enum_decode_static(output, declaration, variants, codec)?
            }
        }
        MetaDeclarationKind::Newtype(underlying) => {
            line(
                output,
                2,
                &format!(
                    "let value: {} = serialization.Decode[C].decode[E, D](var decoder)?",
                    underlying
                ),
            );
            line(output, 2, &format!("{}(value)", declaration.identity()));
        }
        MetaDeclarationKind::Trait(_) => {
            return Err(SerializationDeriveError::UnsupportedTargetKind(
                declaration.kind().name().into(),
            ));
        }
    }
    line(output, 1, "}");
    line(output, 0, "}");
    Ok(())
}

fn render_record_decode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    fields: &[MetaField],
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    let fields = field_policies(fields, codec)?;
    if codec == "MessagePack" {
        return render_messagepack_record_decode_static(output, declaration, &fields);
    }
    line(output, 2, "let start = decoder.next()?");
    line(output, 2, "match start {");
    line(
        output,
        3,
        "serialization.SerializationEvent.StartRecord(_, _) => {",
    );
    let values = render_record_decode_machine(output, &fields, codec, 4, RecordEventStyle::Record)?;
    line(
        output,
        4,
        &format!(
            "{} {{ {} }}",
            declaration.identity(),
            values
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    line(output, 3, "}");
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    Ok(())
}

fn render_messagepack_record_decode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    fields: &[(&MetaField, FieldPolicy)],
) -> Result<(), SerializationDeriveError> {
    line(output, 2, "let start = decoder.next()?");
    line(output, 2, "match start {");
    line(
        output,
        3,
        "serialization.SerializationEvent.StartMap(_) => {",
    );
    let values =
        render_record_decode_machine(output, fields, "MessagePack", 4, RecordEventStyle::Map)?;
    line(
        output,
        4,
        &format!(
            "{} {{ {} }}",
            declaration.identity(),
            values
                .iter()
                .map(|(name, value)| format!("{name}: {value}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    line(output, 3, "}");
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    Ok(())
}

/// Emit the bounded, order-independent record decoder used by records and
/// enum record payloads.  Each field is decoded exactly once into an Option
/// slot; required fields are checked only after the complete input record has
/// been consumed, which preserves atomic publication and affine cleanup.
#[derive(Debug, Clone, Copy)]
enum RecordEventStyle {
    Record,
    Map,
}

fn render_record_decode_machine(
    output: &mut String,
    fields: &[(&MetaField, FieldPolicy)],
    codec: &str,
    indent: usize,
    style: RecordEventStyle,
) -> Result<Vec<(String, String)>, SerializationDeriveError> {
    for (index, (field, _)) in fields.iter().enumerate() {
        member_name(field.name())?;
        line(
            output,
            indent,
            &format!("var __tondo_seen_{index}: Bool = false"),
        );
        line(
            output,
            indent,
            &format!("var __tondo_slot_{index}: Option[{}] = none", field.ty()),
        );
    }
    line(output, indent, "for {");
    line(output, indent + 1, "match decoder.peek()? {");
    let end_event = match style {
        RecordEventStyle::Record => "EndRecord",
        RecordEventStyle::Map => "EndMap",
    };
    line(
        output,
        indent + 2,
        &format!("some(serialization.SerializationEvent.{end_event}) => {{"),
    );
    line(output, indent + 3, "_ = decoder.next()?");
    line(output, indent + 3, "break");
    line(output, indent + 2, "}");
    if matches!(style, RecordEventStyle::Record) {
        line(
            output,
            indent + 2,
            "some(serialization.SerializationEvent.Field(_)) => {",
        );
        line(
            output,
            indent + 3,
            "let __tondo_field_name = match decoder.next()? {",
        );
        line(
            output,
            indent + 4,
            "serialization.SerializationEvent.Field(name) => name",
        );
        line(
            output,
            indent + 4,
            "_ => fail decoder.reject(serialization.SerializationError.UnexpectedEvent)",
        );
        line(output, indent + 3, "}");
    } else {
        line(
            output,
            indent + 2,
            "some(serialization.SerializationEvent.MapKey) => {",
        );
        line(output, indent + 3, "_ = decoder.next()?");
        line(
            output,
            indent + 3,
            "let __tondo_field_name = match decoder.next()? {",
        );
        line(
            output,
            indent + 4,
            "serialization.SerializationEvent.String(name) => name",
        );
        line(
            output,
            indent + 4,
            "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
        );
        line(output, indent + 3, "}");
    }
    line(output, indent + 3, "match __tondo_field_name {");
    for (index, (field, policy)) in fields.iter().enumerate() {
        line(
            output,
            indent + 4,
            &format!("{} => {{", quote(&policy.event_name(codec))),
        );
        line(output, indent + 5, &format!("if __tondo_seen_{index} {{"));
        line(
            output,
            indent + 6,
            "fail decoder.reject(serialization.SerializationError.DuplicateField)",
        );
        line(output, indent + 5, "}");
        line(output, indent + 5, &format!("__tondo_seen_{index} = true"));
        if policy.ignored {
            if codec == "Json" && policy.json_base64 {
                line(
                    output,
                    indent + 5,
                    &format!(
                        "let __tondo_ignored_{index}: {} = decoder.base64()?",
                        field.ty()
                    ),
                );
            } else {
                line(
                    output,
                    indent + 5,
                    &format!(
                        "let __tondo_ignored_{index}: {} = serialization.Decode[C].decode[E, D](var decoder)?",
                        field.ty()
                    ),
                );
            }
            line(output, indent + 5, &format!("_ = __tondo_ignored_{index}"));
        } else if codec == "Json" && policy.json_base64 {
            line(
                output,
                indent + 5,
                &format!("__tondo_slot_{index} = some(decoder.base64()?)"),
            );
        } else {
            line(
                output,
                indent + 5,
                &format!(
                    "let __tondo_decoded_{index}: {} = serialization.Decode[C].decode[E, D](var decoder)?",
                    field.ty()
                ),
            );
            line(
                output,
                indent + 5,
                &format!("__tondo_slot_{index} = some(__tondo_decoded_{index})"),
            );
        }
        line(output, indent + 4, "}");
    }
    line(
        output,
        indent + 4,
        "_ => fail decoder.reject(serialization.SerializationError.UnknownField)",
    );
    line(output, indent + 3, "}");
    line(output, indent + 2, "}");
    line(
        output,
        indent + 2,
        "some(_) => fail decoder.reject(serialization.SerializationError.UnexpectedEvent)",
    );
    line(
        output,
        indent + 2,
        "none => fail decoder.reject(serialization.SerializationError.UnexpectedEvent)",
    );
    line(output, indent + 1, "}");
    line(output, indent, "}");

    let mut values = Vec::with_capacity(fields.len());
    for (index, (field, policy)) in fields.iter().enumerate() {
        let value_name = format!("__tondo_value_{index}");
        if policy.ignored {
            line(
                output,
                indent,
                &format!("let {value_name}: {} = none", field.ty()),
            );
        } else if is_option_type(field.ty()) {
            line(
                output,
                indent,
                &format!(
                    "let {value_name}: {} = match __tondo_slot_{index} {{",
                    field.ty()
                ),
            );
            line(output, indent + 1, "some(value) => value");
            line(output, indent + 1, "none => none");
            line(output, indent, "}");
        } else {
            line(
                output,
                indent,
                &format!(
                    "let {value_name}: {} = match __tondo_slot_{index} {{",
                    field.ty()
                ),
            );
            line(output, indent + 1, "some(value) => value");
            line(
                output,
                indent + 1,
                "none => fail decoder.reject(serialization.SerializationError.MissingField)",
            );
            line(output, indent, "}");
        }
        values.push((field.name().to_owned(), value_name));
    }
    Ok(values)
}

fn render_enum_decode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
    codec: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, 2, "let start = decoder.next()?");
    line(output, 2, "match start {");
    for variant in variants {
        member_name(variant.name())?;
        line(
            output,
            3,
            &format!(
                "serialization.SerializationEvent.StartEnum(_, {}) => {{",
                quote(variant.name())
            ),
        );
        match variant.payload() {
            MetaVariantPayload::Unit => {
                render_enum_end_static(
                    output,
                    4,
                    &format!("{}.{}", declaration.identity(), variant.name()),
                )?;
            }
            MetaVariantPayload::Tuple(types) => {
                let mut names = Vec::new();
                for (index, ty) in types.iter().enumerate() {
                    let name = format!("value_{index}");
                    line(
                        output,
                        4,
                        &format!(
                            "let {}: {} = serialization.Decode[C].decode[E, D](var decoder)?",
                            name, ty
                        ),
                    );
                    names.push(name);
                }
                render_enum_end_static(
                    output,
                    4,
                    &format!(
                        "{}.{}({})",
                        declaration.identity(),
                        variant.name(),
                        names.join(", ")
                    ),
                )?;
            }
            MetaVariantPayload::Record(fields) => {
                let fields = field_policies(fields, codec)?;
                let values = render_record_decode_machine(
                    output,
                    &fields,
                    codec,
                    4,
                    RecordEventStyle::Record,
                )?;
                let value = format!(
                    "{}.{} {{ {} }}",
                    declaration.identity(),
                    variant.name(),
                    values
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                render_enum_end_static(output, 4, &value)?;
            }
        }
        line(output, 3, "}");
    }
    line(
        output,
        3,
        "serialization.SerializationEvent.Field(_) => fail decoder.reject(serialization.SerializationError.UnknownField)",
    );
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    Ok(())
}

fn render_json_enum_decode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
) -> Result<(), SerializationDeriveError> {
    render_expect_event(output, 2, "StartRecord(_, _)");
    line(output, 2, "match decoder.next()? {");
    for variant in variants {
        member_name(variant.name())?;
        line(
            output,
            3,
            &format!(
                "serialization.SerializationEvent.Field({}) => {{",
                quote(variant.name())
            ),
        );
        match variant.payload() {
            MetaVariantPayload::Unit => {
                render_expect_event(output, 4, "Null");
                render_json_enum_end(
                    output,
                    4,
                    &format!("{}.{}", declaration.identity(), variant.name()),
                );
            }
            MetaVariantPayload::Tuple(types) => {
                render_expect_event(output, 4, "StartArray(_)");
                let mut names = Vec::new();
                for (index, ty) in types.iter().enumerate() {
                    let name = format!("value_{index}");
                    line(
                        output,
                        4,
                        &format!(
                            "let {name}: {ty} = serialization.Decode[C].decode[E, D](var decoder)?"
                        ),
                    );
                    names.push(name);
                }
                render_expect_event(output, 4, "EndArray");
                render_json_enum_end(
                    output,
                    4,
                    &format!(
                        "{}.{}({})",
                        declaration.identity(),
                        variant.name(),
                        names.join(", ")
                    ),
                );
            }
            MetaVariantPayload::Record(fields) => {
                let fields = field_policies(fields, "Json")?;
                render_expect_event(output, 4, "StartRecord(_, _)");
                let values = render_record_decode_machine(
                    output,
                    &fields,
                    "Json",
                    4,
                    RecordEventStyle::Record,
                )?;
                render_json_enum_end(
                    output,
                    4,
                    &format!(
                        "{}.{} {{ {} }}",
                        declaration.identity(),
                        variant.name(),
                        values
                            .iter()
                            .map(|(name, value)| format!("{name}: {value}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
        line(output, 3, "}");
    }
    line(
        output,
        3,
        "serialization.SerializationEvent.Field(_) => fail decoder.reject(serialization.SerializationError.UnknownField)",
    );
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    Ok(())
}

fn render_messagepack_enum_decode_static(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
) -> Result<(), SerializationDeriveError> {
    render_expect_event(output, 2, "StartMap(_)");
    line(output, 2, "match decoder.next()? {");
    line(output, 3, "serialization.SerializationEvent.MapKey => ()");
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    line(
        output,
        2,
        "let __tondo_variant_name = match decoder.next()? {",
    );
    line(
        output,
        3,
        "serialization.SerializationEvent.String(name) => name",
    );
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, 2, "}");
    line(output, 2, "match __tondo_variant_name {");
    for variant in variants {
        member_name(variant.name())?;
        line(output, 3, &format!("{} => {{", quote(variant.name())));
        match variant.payload() {
            MetaVariantPayload::Unit => {
                render_expect_event(output, 4, "Null");
                render_messagepack_enum_end(
                    output,
                    4,
                    &format!("{}.{}", declaration.identity(), variant.name()),
                );
            }
            MetaVariantPayload::Tuple(types) => {
                render_expect_event(output, 4, "StartArray(_)");
                let mut names = Vec::new();
                for (index, ty) in types.iter().enumerate() {
                    let name = format!("value_{index}");
                    line(
                        output,
                        4,
                        &format!(
                            "let {name}: {ty} = serialization.Decode[C].decode[E, D](var decoder)?"
                        ),
                    );
                    names.push(name);
                }
                render_expect_event(output, 4, "EndArray");
                render_messagepack_enum_end(
                    output,
                    4,
                    &format!(
                        "{}.{}({})",
                        declaration.identity(),
                        variant.name(),
                        names.join(", ")
                    ),
                );
            }
            MetaVariantPayload::Record(fields) => {
                let fields = field_policies(fields, "MessagePack")?;
                let values = render_record_decode_machine(
                    output,
                    &fields,
                    "MessagePack",
                    4,
                    RecordEventStyle::Map,
                )?;
                render_messagepack_enum_end(
                    output,
                    4,
                    &format!(
                        "{}.{} {{ {} }}",
                        declaration.identity(),
                        variant.name(),
                        values
                            .iter()
                            .map(|(name, value)| format!("{name}: {value}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
        line(output, 3, "}");
    }
    line(
        output,
        3,
        "_ => fail decoder.reject(serialization.SerializationError.UnknownField)",
    );
    line(output, 2, "}");
    Ok(())
}

fn render_messagepack_enum_end(output: &mut String, indent: usize, value: &str) {
    line(output, indent, "match decoder.next()? {");
    line(
        output,
        indent + 1,
        &format!("serialization.SerializationEvent.EndMap => {value}"),
    );
    line(
        output,
        indent + 1,
        "serialization.SerializationEvent.MapKey => fail decoder.reject(serialization.SerializationError.DuplicateField)",
    );
    line(
        output,
        indent + 1,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, indent, "}");
}

fn render_expect_event(output: &mut String, indent: usize, pattern: &str) {
    line(output, indent, "match decoder.next()? {");
    line(
        output,
        indent + 1,
        &format!("serialization.SerializationEvent.{pattern} => ()"),
    );
    line(
        output,
        indent + 1,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, indent, "}");
}

fn render_json_enum_end(output: &mut String, indent: usize, value: &str) {
    line(output, indent, "match decoder.next()? {");
    line(
        output,
        indent + 1,
        &format!("serialization.SerializationEvent.EndRecord => {value}"),
    );
    line(
        output,
        indent + 1,
        "serialization.SerializationEvent.Field(_) => fail decoder.reject(serialization.SerializationError.DuplicateField)",
    );
    line(
        output,
        indent + 1,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, indent, "}");
}

fn render_enum_end_static(
    output: &mut String,
    indent: usize,
    value: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, indent, "match decoder.next()? {");
    line(
        output,
        indent + 1,
        &format!("serialization.SerializationEvent.EndEnum => {}", value),
    );
    line(
        output,
        indent + 1,
        "_ => fail decoder.reject(serialization.SerializationError.TypeMismatch)",
    );
    line(output, indent, "}");
    Ok(())
}

fn render_record_decode(
    output: &mut String,
    declaration: &MetaDeclaration,
    fields: &[MetaField],
) -> Result<(), SerializationDeriveError> {
    line(output, 2, "let start = deserializer.next()?");
    line(output, 2, "match start {");
    line(
        output,
        3,
        &format!("{}.StartRecord(_, _) => {{", EVENT_TYPE),
    );
    for field in fields {
        member_name(field.name())?;
        line(output, 4, "match deserializer.next()? {");
        line(
            output,
            5,
            &format!("{}.Field({}) => ()", EVENT_TYPE, quote(field.name())),
        );
        line(output, 5, &format!("_ => fail {}.TypeMismatch", ERROR_TYPE));
        line(output, 4, "}");
        line(
            output,
            4,
            &format!(
                "let {}: {} = serialization.Deserialize.deserialize(var deserializer)?",
                field.name(),
                field.ty()
            ),
        );
    }
    line(output, 4, "match deserializer.next()? {");
    line(output, 5, &format!("{}.EndRecord => ()", EVENT_TYPE));
    line(output, 5, &format!("_ => fail {}.TypeMismatch", ERROR_TYPE));
    line(output, 4, "}");
    line(
        output,
        4,
        &format!(
            "{} {{ {} }}",
            declaration.identity(),
            fields
                .iter()
                .map(|field| field.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    line(output, 3, "}");
    line(output, 3, &format!("_ => fail {}.TypeMismatch", ERROR_TYPE));
    line(output, 2, "}");
    Ok(())
}

fn render_enum_decode(
    output: &mut String,
    declaration: &MetaDeclaration,
    variants: &[MetaVariant],
) -> Result<(), SerializationDeriveError> {
    line(output, 2, "let start = deserializer.next()?");
    line(output, 2, "match start {");
    for variant in variants {
        member_name(variant.name())?;
        line(
            output,
            3,
            &format!(
                "{}.StartEnum(_, {}) => {{",
                EVENT_TYPE,
                quote(variant.name())
            ),
        );
        match variant.payload() {
            MetaVariantPayload::Unit => {
                render_enum_end(
                    output,
                    4,
                    &format!("{}.{}", declaration.identity(), variant.name()),
                )?;
            }
            MetaVariantPayload::Tuple(types) => {
                let mut names = Vec::new();
                for (index, ty) in types.iter().enumerate() {
                    let name = format!("value_{index}");
                    line(
                        output,
                        4,
                        &format!(
                            "let {}: {} = serialization.Deserialize.deserialize(var deserializer)?",
                            name, ty
                        ),
                    );
                    names.push(name);
                }
                render_enum_end(
                    output,
                    4,
                    &format!(
                        "{}.{}({})",
                        declaration.identity(),
                        variant.name(),
                        names.join(", ")
                    ),
                )?;
            }
            MetaVariantPayload::Record(fields) => {
                for field in fields {
                    member_name(field.name())?;
                    line(output, 4, "match deserializer.next()? {");
                    line(
                        output,
                        5,
                        &format!("{}.Field({}) => ()", EVENT_TYPE, quote(field.name())),
                    );
                    line(output, 5, &format!("_ => fail {}.TypeMismatch", ERROR_TYPE));
                    line(output, 4, "}");
                    line(
                        output,
                        4,
                        &format!(
                            "let {}: {} = serialization.Deserialize.deserialize(var deserializer)?",
                            field.name(),
                            field.ty()
                        ),
                    );
                }
                let value = format!(
                    "{}.{} {{ {} }}",
                    declaration.identity(),
                    variant.name(),
                    fields
                        .iter()
                        .map(|field| field.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                render_enum_end(output, 4, &value)?;
            }
        }
        line(output, 3, "}");
    }
    line(output, 3, &format!("_ => fail {}.TypeMismatch", ERROR_TYPE));
    line(output, 2, "}");
    Ok(())
}

fn render_enum_end(
    output: &mut String,
    indent: usize,
    value: &str,
) -> Result<(), SerializationDeriveError> {
    line(output, indent, "match deserializer.next()? {");
    line(
        output,
        indent + 1,
        &format!("{}.EndEnum => {}", EVENT_TYPE, value),
    );
    line(
        output,
        indent + 1,
        &format!("_ => fail {}.TypeMismatch", ERROR_TYPE),
    );
    line(output, indent, "}");
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldPolicy {
    wire_name: String,
    ignored: bool,
    json_base64: bool,
    messagepack_binary: bool,
    proto_number: Option<u32>,
}

impl FieldPolicy {
    fn event_name(&self, codec: &str) -> String {
        if codec == "Protobuf" {
            // The common event ABI is name-based.  Protobuf's schema-owned
            // number is carried in a reserved, lossless field token so the
            // codec adapter can lower it to the actual wire tag without
            // consulting reflection or declaration order.
            self.proto_number
                .map_or_else(|| self.wire_name.clone(), |number| format!("#{number}"))
        } else {
            self.wire_name.clone()
        }
    }
}

fn field_policies<'a>(
    fields: &'a [MetaField],
    codec: &str,
) -> Result<Vec<(&'a MetaField, FieldPolicy)>, SerializationDeriveError> {
    fields
        .iter()
        .map(|field| {
            member_name(field.name())?;
            let mut wire_name = field.name().to_owned();
            let mut ignored = false;
            let mut json_base64 = false;
            let mut messagepack_binary = false;
            let mut proto_number = None;
            let mut seen_name = false;
            let mut seen_ignore = false;
            let mut seen_json = false;
            let mut seen_messagepack = false;
            let mut seen_proto = false;
            for attribute in field.attributes() {
                let name = attribute.name();
                match name {
                    "name" => {
                        if seen_name || attribute.argument().is_none() {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: attribute.argument().map(str::to_owned),
                            });
                        }
                        let value = attribute.argument().unwrap();
                        if value.is_empty() {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: Some(value.into()),
                            });
                        }
                        wire_name = value.to_owned();
                        seen_name = true;
                    }
                    "ignore" => {
                        if seen_ignore || attribute.argument().is_some() {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: attribute.argument().map(str::to_owned),
                            });
                        }
                        ignored = true;
                        seen_ignore = true;
                    }
                    "json" => {
                        if seen_json {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: attribute.argument().map(str::to_owned),
                            });
                        }
                        seen_json = true;
                        let Some(argument) = attribute.argument() else {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: None,
                            });
                        };
                        if argument != "base64" {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: Some(argument.into()),
                            });
                        }
                        if codec == "Json" {
                            if !is_bytes_type(field.ty()) {
                                return Err(SerializationDeriveError::JsonBase64RequiresBytes(
                                    field.name().into(),
                                ));
                            }
                            json_base64 = true;
                        }
                    }
                    "messagepack" => {
                        if seen_messagepack {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: attribute.argument().map(str::to_owned),
                            });
                        }
                        seen_messagepack = true;
                        let Some(argument) = attribute.argument() else {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: None,
                            });
                        };
                        if argument != "binary" {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: Some(argument.into()),
                            });
                        }
                        if codec == "MessagePack" && !is_bytes_type(field.ty()) {
                            return Err(SerializationDeriveError::AttributeCodecMismatch {
                                name: name.into(),
                                codec: codec.into(),
                            });
                        }
                        if codec == "MessagePack" {
                            messagepack_binary = true;
                        }
                    }
                    "proto" => {
                        if seen_proto {
                            return Err(SerializationDeriveError::InvalidAttribute {
                                name: name.into(),
                                argument: attribute.argument().map(str::to_owned),
                            });
                        }
                        seen_proto = true;
                        let Some(argument) = attribute.argument() else {
                            return Err(SerializationDeriveError::InvalidProtoNumber(
                                "missing".into(),
                            ));
                        };
                        let number = argument
                            .parse::<u32>()
                            .ok()
                            .filter(|number| *number > 0 && *number <= 536_870_911)
                            .filter(|number| !(19_000..=19_999).contains(number))
                            .ok_or_else(|| {
                                SerializationDeriveError::InvalidProtoNumber(argument.into())
                            })?;
                        if codec == "Protobuf" {
                            proto_number = Some(number);
                        }
                    }
                    _ => {
                        return Err(SerializationDeriveError::InvalidAttribute {
                            name: name.into(),
                            argument: attribute.argument().map(str::to_owned),
                        });
                    }
                }
            }
            if ignored && !is_option_type(field.ty()) {
                return Err(SerializationDeriveError::IgnoredFieldRequiresOption(
                    field.name().into(),
                ));
            }
            if codec == "Protobuf" && proto_number.is_none() {
                return Err(SerializationDeriveError::InvalidProtoNumber(format!(
                    "missing for `{}`",
                    field.name()
                )));
            }
            Ok((
                field,
                FieldPolicy {
                    wire_name,
                    ignored,
                    json_base64,
                    messagepack_binary,
                    proto_number,
                },
            ))
        })
        .collect::<Result<Vec<_>, _>>()
        .and_then(|policies| {
            let mut seen = BTreeSet::new();
            for (_, policy) in &policies {
                if !policy.ignored {
                    let event_name = policy.event_name(codec);
                    if !seen.insert(event_name.clone()) {
                        return Err(SerializationDeriveError::DuplicateWireField(event_name));
                    }
                }
            }
            Ok(policies)
        })
}

fn is_bytes_type(ty: &str) -> bool {
    matches!(ty.trim(), "Bytes" | "bytes.Bytes")
}

fn is_option_type(ty: &str) -> bool {
    let ty = ty.trim();
    ty.ends_with('?') || ty.starts_with("Option[")
}

fn member_name(name: &str) -> Result<(), SerializationDeriveError> {
    let mut characters = name.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err(SerializationDeriveError::InvalidMemberName(name.into()))
    }
}

fn quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn line(output: &mut String, indent: usize, text: &str) {
    for _ in 0..indent {
        output.push_str("    ");
    }
    output.push_str(text);
    output.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{
        DeriveContext, DeriveProvider, DeriveRequest, DeriveTarget, DeriveTargetKind,
        MetaAttribute, MetaDeclaration, MetaDeclarationKind, MetaField, MetaGenericParameter,
        MetaLimits, MetaSnapshot, MetaSpan, MetaVisibility, validate_derive_requests,
    };
    use crate::meta_derive::execute_derive_plan;

    fn span(start: u32, end: u32) -> MetaSpan {
        MetaSpan::new(7, start, end).unwrap()
    }

    fn field(name: &str, ty: &str, ordinal: u32) -> MetaField {
        MetaField::new(
            name,
            ty,
            MetaVisibility::Private,
            ordinal,
            span(ordinal * 10, ordinal * 10 + 5),
            None::<String>,
        )
        .unwrap()
    }

    fn record_snapshot() -> MetaSnapshot {
        MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "User",
                "app",
                MetaVisibility::Private,
                [],
                [],
                span(10, 42),
                None::<String>,
                MetaDeclarationKind::record([field("name", "String", 1), field("id", "Int", 0)]),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn enum_snapshot() -> MetaSnapshot {
        MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "Choice",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span(50, 92),
                None::<String>,
                MetaDeclarationKind::enumeration([
                    MetaVariant::new(
                        "Empty",
                        MetaVariantPayload::unit(),
                        0,
                        span(50, 55),
                        None::<String>,
                    )
                    .unwrap(),
                    MetaVariant::new(
                        "Item",
                        MetaVariantPayload::tuple(["Int"]),
                        1,
                        span(56, 66),
                        None::<String>,
                    )
                    .unwrap(),
                    MetaVariant::new(
                        "Named",
                        MetaVariantPayload::record([field("message", "String", 0)]),
                        2,
                        span(67, 91),
                        None::<String>,
                    )
                    .unwrap(),
                ]),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn generic_record_snapshot() -> MetaSnapshot {
        MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "Boxed",
                "app",
                MetaVisibility::Public,
                [MetaGenericParameter::new("T", Vec::<String>::new()).unwrap()],
                [],
                span(100, 130),
                None::<String>,
                MetaDeclarationKind::record([field("value", "T", 0)]),
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn derive(
        snapshot: MetaSnapshot,
        direction: SerializationDirection,
        generics: &[&str],
        introduced_bounds: &[&str],
        kind: DeriveTargetKind,
    ) -> Result<crate::meta_derive::DeriveExecution, crate::meta_derive::DeriveExecutionError> {
        derive_named(
            "User",
            snapshot,
            direction,
            generics,
            introduced_bounds,
            kind,
        )
    }

    fn derive_named(
        target: &str,
        snapshot: MetaSnapshot,
        direction: SerializationDirection,
        generics: &[&str],
        introduced_bounds: &[&str],
        kind: DeriveTargetKind,
    ) -> Result<crate::meta_derive::DeriveExecution, crate::meta_derive::DeriveExecutionError> {
        derive_named_codec(
            target,
            snapshot,
            direction,
            generics,
            introduced_bounds,
            kind,
            None,
        )
    }

    fn derive_named_codec(
        target: &str,
        snapshot: MetaSnapshot,
        direction: SerializationDirection,
        generics: &[&str],
        introduced_bounds: &[&str],
        kind: DeriveTargetKind,
        codec: Option<&str>,
    ) -> Result<crate::meta_derive::DeriveExecution, crate::meta_derive::DeriveExecutionError> {
        let mut context = DeriveContext::new("app");
        context.add_target(DeriveTarget::new(
            target,
            "app",
            generics.iter().copied(),
            kind,
        ));
        let trait_identity = codec.map_or_else(
            || direction.trait_identity().to_owned(),
            |codec| format!("{}[{codec}]", direction.trait_identity()),
        );
        context.add_trait(&trait_identity);
        context.add_provider(DeriveProvider::new(
            direction.trait_identity(),
            direction.provider_identity(),
            introduced_bounds.iter().copied(),
        ));
        let request = DeriveRequest::new("app", target, generics.iter().copied(), [trait_identity]);
        let plan = validate_derive_requests(&[request], &context).unwrap();
        let mut registry = DeriveProviderRegistry::default();
        register_serialization_providers(&mut registry).unwrap();
        execute_derive_plan(
            &plan,
            snapshot,
            MetaLimits::new(100_000, 1 << 20, 1 << 20).unwrap(),
            &registry,
        )
    }

    #[test]
    fn record_provider_is_deterministic_and_maps_to_target() {
        let execution = derive(
            record_snapshot(),
            SerializationDirection::Serialize,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let output = &execution.response().outputs()[0];
        let source = std::str::from_utf8(output.bytes()).unwrap();
        assert!(source.contains("serializer.startRecord(\"User\", 2)"));
        assert!(
            source.find("serializer.field(\"id\")").unwrap()
                < source.find("serializer.field(\"name\")").unwrap()
        );
        assert_eq!(output.mappings().len(), 1);
        assert_eq!(output.mappings()[0].origin(), span(10, 42));
    }

    #[test]
    fn deserialize_provider_and_enum_shapes_are_generated() {
        let record = derive(
            record_snapshot(),
            SerializationDirection::Deserialize,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let record_source = std::str::from_utf8(record.response().outputs()[0].bytes()).unwrap();
        assert!(record_source.contains("StartRecord"));
        assert!(record_source.contains("EndRecord"));
        assert!(record_source.contains("User { id, name }"));

        for direction in [
            SerializationDirection::Serialize,
            SerializationDirection::Deserialize,
            SerializationDirection::Encode,
            SerializationDirection::Decode,
        ] {
            let execution = derive_named(
                "Choice",
                enum_snapshot(),
                direction,
                &[],
                &[],
                DeriveTargetKind::Enum,
            )
            .unwrap();
            let source = std::str::from_utf8(execution.response().outputs()[0].bytes()).unwrap();
            assert!(source.contains("Empty"));
            assert!(source.contains("Item"));
            assert!(source.contains("Named"));
            match direction {
                SerializationDirection::Serialize => assert!(source.contains("endEnum")),
                SerializationDirection::Deserialize => assert!(source.contains("EndEnum")),
                SerializationDirection::Encode => assert!(source.contains("encoder.endEnum")),
                SerializationDirection::Decode => {
                    assert!(source.contains("SerializationEvent.EndEnum"))
                }
            }
        }
    }

    #[test]
    fn json_enum_provider_uses_one_externally_tagged_object() {
        let encoded = derive_named_codec(
            "Choice",
            enum_snapshot(),
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Enum,
            Some("Json"),
        )
        .unwrap();
        let encoded = std::str::from_utf8(encoded.response().outputs()[0].bytes()).unwrap();
        assert!(encoded.contains("encoder.startRecord(\"\", 1)"));
        assert!(encoded.contains("encoder.field(\"Empty\")"));
        assert!(encoded.contains("encoder.null()"));
        assert!(encoded.contains("encoder.field(\"Item\")"));
        assert!(encoded.contains("encoder.startArray(1)"));
        assert!(encoded.contains("encoder.field(\"Named\")"));
        assert!(!encoded.contains("encoder.startEnum"));

        let decoded = derive_named_codec(
            "Choice",
            enum_snapshot(),
            SerializationDirection::Decode,
            &[],
            &[],
            DeriveTargetKind::Enum,
            Some("Json"),
        )
        .unwrap();
        let decoded = std::str::from_utf8(decoded.response().outputs()[0].bytes()).unwrap();
        assert!(decoded.contains("SerializationEvent.StartRecord(_, _)"));
        assert!(decoded.contains("SerializationEvent.Field(\"Empty\")"));
        assert!(decoded.contains("SerializationEvent.Null"));
        assert!(decoded.contains("SerializationEvent.StartArray(_)"));
        assert!(decoded.contains("SerializationEvent.EndArray"));
        assert!(!decoded.contains("SerializationEvent.StartEnum"));
    }

    #[test]
    fn canonical_providers_emit_codec_generic_static_dispatch() {
        let encoded = derive(
            record_snapshot(),
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let encoded_source = std::str::from_utf8(encoded.response().outputs()[0].bytes()).unwrap();
        assert!(encoded_source.contains("fn encode[E, S: serialization.Encoder[C, E]]"));
        assert!(
            encoded_source
                .contains("let User { id: __tondo_field_0, name: __tondo_field_1 } = value")
        );
        assert!(encoded_source.contains("serialization.Encode[C].encode[E, S](__tondo_field_0"));
        assert!(encoded_source.contains("encoder.startRecord(\"User\", 2)"));

        let decoded = derive(
            record_snapshot(),
            SerializationDirection::Decode,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let decoded_source = std::str::from_utf8(decoded.response().outputs()[0].bytes()).unwrap();
        assert!(decoded_source.contains("fn decode[E, D: serialization.Decoder[C, E]]"));
        assert!(decoded_source.contains("serialization.Decode[C].decode[E, D](var decoder)"));
        assert!(decoded_source.contains("serialization.SerializationEvent.EndRecord"));

        let generic = derive_named(
            "Boxed",
            generic_record_snapshot(),
            SerializationDirection::Encode,
            &["T"],
            &["T"],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let generic_source = std::str::from_utf8(generic.response().outputs()[0].bytes()).unwrap();
        assert!(
            generic_source.contains("impl [T: Discard + serialization.Encode]serialization.Encode")
        );
        assert!(generic_source.contains("for Boxed[T]"));

        let generic_decode = derive_named(
            "Boxed",
            generic_record_snapshot(),
            SerializationDirection::Decode,
            &["T"],
            &["T"],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let generic_decode_source =
            std::str::from_utf8(generic_decode.response().outputs()[0].bytes()).unwrap();
        assert!(
            generic_decode_source
                .contains("impl [T: Discard + serialization.Decode]serialization.Decode")
        );
    }

    #[test]
    fn specialized_codecs_and_field_annotations_are_deterministic() {
        let payload = field("payload", "Bytes", 0)
            .with_attributes([
                MetaAttribute::new("name", Some("wire_payload")).unwrap(),
                MetaAttribute::new("json", Some("base64")).unwrap(),
            ])
            .unwrap();
        let hidden = field("hidden", "Option[Int]", 1)
            .with_attributes([MetaAttribute::new("ignore", None::<String>).unwrap()])
            .unwrap();
        let snapshot = MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "Annotated",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span(300, 350),
                None::<String>,
                MetaDeclarationKind::record([payload, hidden]),
            )
            .unwrap()],
        )
        .unwrap();
        let execution = derive_named_codec(
            "Annotated",
            snapshot,
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Record,
            Some("Json"),
        )
        .unwrap();
        let source = std::str::from_utf8(execution.response().outputs()[0].bytes()).unwrap();
        assert!(source.contains("Encoder[Json, E]"));
        assert!(source.contains("encoder.base64(__tondo_field_0)"));
        assert!(source.contains("encoder.startRecord(\"Annotated\", 1)"));
        assert!(source.contains("encoder.field(\"wire_payload\")"));
        assert!(!source.contains("encoder.field(\"hidden\")"));

        let decoded = derive_named_codec(
            "Annotated",
            MetaSnapshot::new(
                [],
                [],
                [MetaDeclaration::new(
                    "Annotated",
                    "app",
                    MetaVisibility::Public,
                    [],
                    [],
                    span(300, 350),
                    None::<String>,
                    MetaDeclarationKind::record([
                        field("payload", "Bytes", 0)
                            .with_attributes([
                                MetaAttribute::new("name", Some("wire_payload")).unwrap(),
                                MetaAttribute::new("json", Some("base64")).unwrap(),
                            ])
                            .unwrap(),
                        field("hidden", "Option[Int]", 1)
                            .with_attributes(
                                [MetaAttribute::new("ignore", None::<String>).unwrap()],
                            )
                            .unwrap(),
                    ]),
                )
                .unwrap()],
            )
            .unwrap(),
            SerializationDirection::Decode,
            &[],
            &[],
            DeriveTargetKind::Record,
            Some("Json"),
        )
        .unwrap();
        let decoded_source = std::str::from_utf8(decoded.response().outputs()[0].bytes()).unwrap();
        assert!(decoded_source.contains("Decoder[Json, E]"));
        assert!(decoded_source.contains("wire_payload"));
        assert!(decoded_source.contains("decoder.base64()?"));
        assert!(decoded_source.contains("SerializationError.UnknownField"));
        assert!(decoded_source.contains("SerializationError.DuplicateField"));
        assert!(decoded_source.contains("SerializationError.MissingField"));
        assert!(decoded_source.contains("let __tondo_value_1: Int? = none"));

        let messagepack = derive_named_codec(
            "Annotated",
            MetaSnapshot::new(
                [],
                [],
                [MetaDeclaration::new(
                    "Annotated",
                    "app",
                    MetaVisibility::Public,
                    [],
                    [],
                    span(300, 350),
                    None::<String>,
                    MetaDeclarationKind::record([field("payload", "bytes.Bytes", 0)
                        .with_attributes([
                            MetaAttribute::new("messagepack", Some("binary")).unwrap()
                        ])
                        .unwrap()]),
                )
                .unwrap()],
            )
            .unwrap(),
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Record,
            Some("MessagePack"),
        )
        .unwrap();
        let messagepack_source =
            std::str::from_utf8(messagepack.response().outputs()[0].bytes()).unwrap();
        assert!(messagepack_source.contains("encoder.startMap(1)"));
        assert!(messagepack_source.contains("encoder.mapKey()"));
        assert!(messagepack_source.contains("encoder.bytes(__tondo_field_0)"));

        let messagepack_decode = derive_named_codec(
            "Annotated",
            MetaSnapshot::new(
                [],
                [],
                [MetaDeclaration::new(
                    "Annotated",
                    "app",
                    MetaVisibility::Public,
                    [],
                    [],
                    span(300, 350),
                    None::<String>,
                    MetaDeclarationKind::record([field("payload", "bytes.Bytes", 0)]),
                )
                .unwrap()],
            )
            .unwrap(),
            SerializationDirection::Decode,
            &[],
            &[],
            DeriveTargetKind::Record,
            Some("MessagePack"),
        )
        .unwrap();
        let messagepack_decode_source =
            std::str::from_utf8(messagepack_decode.response().outputs()[0].bytes()).unwrap();
        assert!(messagepack_decode_source.contains("SerializationEvent.StartMap(_)"));
        assert!(messagepack_decode_source.contains("SerializationEvent.MapKey"));

        let messagepack_enum = derive_named_codec(
            "Choice",
            enum_snapshot(),
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Enum,
            Some("MessagePack"),
        )
        .unwrap();
        let messagepack_enum_source =
            std::str::from_utf8(messagepack_enum.response().outputs()[0].bytes()).unwrap();
        assert!(messagepack_enum_source.contains("encoder.startMap(1)"));
        assert!(messagepack_enum_source.contains("encoder.string(\"Empty\")"));

        let protobuf_field = field("id", "Int", 0)
            .with_attributes([MetaAttribute::new("proto", Some("7")).unwrap()])
            .unwrap();
        let protobuf_snapshot = MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "ProtoUser",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span(360, 390),
                None::<String>,
                MetaDeclarationKind::record([protobuf_field]),
            )
            .unwrap()],
        )
        .unwrap();
        let protobuf = derive_named_codec(
            "ProtoUser",
            protobuf_snapshot,
            SerializationDirection::Encode,
            &[],
            &[],
            DeriveTargetKind::Record,
            Some("Protobuf"),
        )
        .unwrap();
        assert!(
            std::str::from_utf8(protobuf.response().outputs()[0].bytes())
                .unwrap()
                .contains("encoder.field(\"#7\")")
        );
    }

    #[test]
    fn generic_and_newtype_providers_preserve_bounds_and_shapes() {
        let generic = derive_named(
            "Boxed",
            generic_record_snapshot(),
            SerializationDirection::Serialize,
            &["T"],
            &["T"],
            DeriveTargetKind::Record,
        )
        .unwrap();
        let generic_source = std::str::from_utf8(generic.response().outputs()[0].bytes()).unwrap();
        assert!(generic_source.contains("impl [T: serialization.Serialize]"));
        assert!(generic_source.contains("for Boxed[T]"));

        let newtype_snapshot = MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "UserId",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span(140, 155),
                None::<String>,
                MetaDeclarationKind::newtype("Int"),
            )
            .unwrap()],
        )
        .unwrap();
        let newtype = derive_named(
            "UserId",
            newtype_snapshot.clone(),
            SerializationDirection::Deserialize,
            &[],
            &[],
            DeriveTargetKind::Newtype,
        )
        .unwrap();
        let newtype_source = std::str::from_utf8(newtype.response().outputs()[0].bytes()).unwrap();
        assert!(newtype_source.contains("let value: Int"));
        assert!(newtype_source.contains("UserId(value)"));
        let serialized = derive_named(
            "UserId",
            newtype_snapshot.clone(),
            SerializationDirection::Serialize,
            &[],
            &[],
            DeriveTargetKind::Newtype,
        )
        .unwrap();
        assert!(
            std::str::from_utf8(serialized.response().outputs()[0].bytes())
                .unwrap()
                .contains("self.value")
        );

        for direction in [
            SerializationDirection::Encode,
            SerializationDirection::Decode,
        ] {
            let execution = derive_named(
                "UserId",
                newtype_snapshot.clone(),
                direction,
                &[],
                &[],
                DeriveTargetKind::Newtype,
            )
            .unwrap();
            let source = std::str::from_utf8(execution.response().outputs()[0].bytes()).unwrap();
            assert!(source.contains("UserId(inner)") || source.contains("UserId(value)"));
            assert!(source.contains("serialization::") || source.contains("serialization."));
        }
    }

    #[test]
    fn provider_errors_and_unsupported_shapes_are_closed() {
        let snapshot = MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "TraitOnly",
                "app",
                MetaVisibility::Public,
                [],
                [],
                span(190, 205),
                None::<String>,
                MetaDeclarationKind::trait_definition([]),
            )
            .unwrap()],
        )
        .unwrap();
        assert!(matches!(
            derive_named(
                "TraitOnly",
                snapshot,
                SerializationDirection::Encode,
                &[],
                &[],
                DeriveTargetKind::Record,
            ),
            Err(crate::meta_derive::DeriveExecutionError::ProviderFailed { .. })
        ));
    }

    #[test]
    fn provider_rejects_missing_targets_bounds_and_member_names() {
        let missing = derive_named(
            "Missing",
            MetaSnapshot::new([], [], []).unwrap(),
            SerializationDirection::Serialize,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap_err();
        assert!(matches!(
            missing,
            crate::meta_derive::DeriveExecutionError::ProviderFailed { .. }
        ));

        let missing_bound = derive_named(
            "Boxed",
            generic_record_snapshot(),
            SerializationDirection::Serialize,
            &["T"],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap_err();
        assert!(matches!(
            missing_bound,
            crate::meta_derive::DeriveExecutionError::ProviderFailed { .. }
        ));

        let invalid_field = MetaSnapshot::new(
            [],
            [],
            [MetaDeclaration::new(
                "Broken",
                "app",
                MetaVisibility::Private,
                [],
                [],
                span(170, 185),
                None::<String>,
                MetaDeclarationKind::record([field("bad-name", "Int", 0)]),
            )
            .unwrap()],
        )
        .unwrap();
        let invalid = derive_named(
            "Broken",
            invalid_field,
            SerializationDirection::Serialize,
            &[],
            &[],
            DeriveTargetKind::Record,
        )
        .unwrap_err();
        assert!(matches!(
            invalid,
            crate::meta_derive::DeriveExecutionError::ProviderFailed { .. }
        ));
    }

    #[test]
    fn annotation_and_codec_validation_is_closed_before_source_publication() {
        assert_eq!(
            codec_for_request("serialization.Encode[Json]", SerializationDirection::Encode)
                .unwrap(),
            "Json"
        );
        assert!(matches!(
            codec_for_request("serialization.Encode[]", SerializationDirection::Encode),
            Err(SerializationDeriveError::InvalidCodec(_))
        ));
        let invalid = field("value", "Int", 0)
            .with_attributes([MetaAttribute::new("unknown", None::<String>).unwrap()])
            .unwrap();
        assert!(matches!(
            field_policies(&[invalid], "Json"),
            Err(SerializationDeriveError::InvalidAttribute { .. })
        ));
        let invalid_ignored = field("value", "Int", 0)
            .with_attributes([MetaAttribute::new("ignore", None::<String>).unwrap()])
            .unwrap();
        assert!(matches!(
            field_policies(&[invalid_ignored], "Json"),
            Err(SerializationDeriveError::IgnoredFieldRequiresOption(_))
        ));
        let duplicate = field("left", "Int", 0)
            .with_attributes([MetaAttribute::new("name", Some("same")).unwrap()])
            .unwrap();
        let duplicate_right = field("right", "Int", 1)
            .with_attributes([MetaAttribute::new("name", Some("same")).unwrap()])
            .unwrap();
        assert!(matches!(
            field_policies(&[duplicate, duplicate_right], "Json"),
            Err(SerializationDeriveError::DuplicateWireField(_))
        ));
        let duplicate_proto = field("id", "Int", 0)
            .with_attributes([
                MetaAttribute::new("proto", Some("7")).unwrap(),
                MetaAttribute::new("proto", Some("8")).unwrap(),
            ])
            .unwrap();
        assert!(matches!(
            field_policies(&[duplicate_proto], "Protobuf"),
            Err(SerializationDeriveError::InvalidAttribute { name, .. }) if name == "proto"
        ));
    }
}
