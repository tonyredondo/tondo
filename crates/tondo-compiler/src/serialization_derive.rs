//! Hermetic build-only providers for `std.serialization`.
//!
//! The provider consumes only the sealed semantic snapshot handed to the
//! derive boundary.  It emits ordinary Tondo source; no runtime reflection,
//! callbacks, filesystem access, or ambient state is involved.

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

const SERIALIZER_TRAIT: &str = "serialization.Serializer";
const DESERIALIZER_TRAIT: &str = "serialization.Deserializer";
const EVENT_TYPE: &str = "serialization.SerializationEvent";
const ERROR_TYPE: &str = "serialization.SerializationError";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationDirection {
    Serialize,
    Deserialize,
}

impl SerializationDirection {
    pub const fn trait_identity(self) -> &'static str {
        match self {
            Self::Serialize => SERIALIZE_TRAIT,
            Self::Deserialize => DESERIALIZE_TRAIT,
        }
    }

    pub const fn provider_identity(self) -> &'static str {
        match self {
            Self::Serialize => SERIALIZE_PROVIDER,
            Self::Deserialize => DESERIALIZE_PROVIDER,
        }
    }

    const fn required_bound(self) -> &'static str {
        self.trait_identity()
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
    TraitMismatch { expected: String, found: String },
    ProviderMismatch { expected: String, found: String },
    TargetMissing { module: String, target: String },
    UnsupportedTargetKind(String),
    MissingGenericBound(String),
    InvalidMemberName(String),
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
    if request.trait_identity() != direction.trait_identity() {
        return Err(SerializationDeriveError::TraitMismatch {
            expected: direction.trait_identity().into(),
            found: request.trait_identity().into(),
        });
    }
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
    }
    Ok(output)
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
    direction: SerializationDirection,
) -> Result<String, SerializationDeriveError> {
    if declaration.generic_parameters().is_empty() {
        return Ok(String::new());
    }
    let mut binders = Vec::new();
    for parameter in declaration.generic_parameters() {
        let mut bounds = parameter.bounds().to_vec();
        if declaration_uses_parameter(declaration, parameter.name())
            && !bounds
                .iter()
                .any(|bound| bound == direction.required_bound())
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
            bounds.push(direction.required_bound().into());
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
        MetaDeclaration, MetaDeclarationKind, MetaField, MetaGenericParameter, MetaLimits,
        MetaSnapshot, MetaSpan, MetaVisibility, validate_derive_requests,
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
        let mut context = DeriveContext::new("app");
        context.add_target(DeriveTarget::new(
            target,
            "app",
            generics.iter().copied(),
            kind,
        ));
        context.add_trait(direction.trait_identity());
        context.add_provider(DeriveProvider::new(
            direction.trait_identity(),
            direction.provider_identity(),
            introduced_bounds.iter().copied(),
        ));
        let request = DeriveRequest::new(
            "app",
            target,
            generics.iter().copied(),
            [direction.trait_identity()],
        );
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
            }
        }
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
            newtype_snapshot,
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
}
