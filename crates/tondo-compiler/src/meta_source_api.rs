//! Typed values at the ordinary Tondo companion boundary. No JSON string or
//! compiler callback stands in for a provider's public request/response ABI.

use crate::meta::*;
use crate::meta_vm::MetaVmError;
use tondo_vm::runtime::{RuntimeValue as V, VmOutcome};

fn record(name: &str, values: Vec<V>) -> V {
    V::Record {
        name: name.into(),
        values,
    }
}
fn variant(name: &str, ordinal: u32, values: Vec<V>) -> V {
    V::Variant {
        name: name.into(),
        variant: ordinal,
        values,
    }
}
fn text(value: &str) -> V {
    V::String(value.into())
}
fn integer(value: impl Into<i128>) -> V {
    V::Integer(value.into())
}
fn array<T>(values: &[T], convert: impl Fn(&T) -> V) -> V {
    V::Array(values.iter().map(convert).collect())
}
fn optional_text(value: Option<&str>) -> V {
    value.map_or(V::OptionNone, |value| V::OptionSome(Box::new(text(value))))
}
fn span(value: MetaSpan) -> V {
    record(
        "Span",
        vec![
            integer(value.file()),
            integer(value.start()),
            integer(value.end()),
        ],
    )
}
fn model_origin(value: &MetaOrigin) -> V {
    match value {
        MetaOrigin::Source(value) => variant("Origin", 0, vec![span(*value)]),
        MetaOrigin::Builtin(identity) => variant("Origin", 1, vec![text(identity)]),
    }
}
fn visibility(value: MetaVisibility) -> V {
    variant(
        "Visibility",
        match value {
            MetaVisibility::Public => 0,
            MetaVisibility::Private => 1,
        },
        vec![],
    )
}
fn attributes(values: &[MetaAttribute]) -> V {
    array(values, |value| {
        record(
            "Attribute",
            vec![text(value.name()), optional_text(value.argument())],
        )
    })
}
fn field(value: &MetaField) -> V {
    record(
        "Field",
        vec![
            text(value.name()),
            type_ref(value.type_ref()),
            visibility(value.visibility()),
            integer(value.ordinal()),
            model_origin(value.origin()),
            optional_text(value.docs()),
            attributes(value.attributes()),
        ],
    )
}

fn type_ref(value: &crate::meta_type::MetaTypeRef) -> V {
    use crate::meta_type::MetaTypePart;
    record(
        "TypeRef",
        vec![
            text(value.identity()),
            array(value.parts(), |part| match part {
                MetaTypePart::Text(value) => variant("TypePart", 0, vec![text(value)]),
                MetaTypePart::Name(name) => variant(
                    "TypePart",
                    1,
                    vec![record(
                        "TypeName",
                        vec![
                            text(&name.module),
                            V::Bool(name.local),
                            V::Bool(name.public),
                            text(&name.name),
                            optional_text(name.import.as_deref()),
                            text(&name.alias),
                        ],
                    )],
                ),
                MetaTypePart::Unavailable(reason) => variant("TypePart", 2, vec![text(reason)]),
            }),
        ],
    )
}
fn enumeration_variant(value: &MetaVariant) -> V {
    let payload = match value.payload() {
        MetaVariantPayload::Unit => variant("VariantPayload", 0, vec![]),
        MetaVariantPayload::Tuple(types) => {
            variant("VariantPayload", 1, vec![array(types, type_ref)])
        }
        MetaVariantPayload::Record(fields) => {
            variant("VariantPayload", 2, vec![array(fields, field)])
        }
    };
    record(
        "Variant",
        vec![
            text(value.name()),
            payload,
            integer(value.ordinal()),
            model_origin(value.origin()),
            optional_text(value.docs()),
            attributes(value.attributes()),
        ],
    )
}
fn declaration(value: &MetaDeclaration) -> V {
    let kind = match value.kind() {
        MetaDeclarationKind::Record(fields) => {
            variant("DeclarationKind", 0, vec![array(fields, field)])
        }
        MetaDeclarationKind::Enum(variants) => variant(
            "DeclarationKind",
            1,
            vec![array(variants, enumeration_variant)],
        ),
        MetaDeclarationKind::Newtype(ty) => variant("DeclarationKind", 2, vec![type_ref(ty)]),
        MetaDeclarationKind::Alias(ty) => variant("DeclarationKind", 4, vec![type_ref(ty)]),
        MetaDeclarationKind::Function(ty) => variant("DeclarationKind", 5, vec![type_ref(ty)]),
        MetaDeclarationKind::Constant(ty) => variant("DeclarationKind", 6, vec![type_ref(ty)]),
        MetaDeclarationKind::Trait(operations) => variant(
            "DeclarationKind",
            3,
            vec![array(operations, |op| {
                record(
                    "Operation",
                    vec![
                        text(op.name()),
                        type_ref(op.type_ref()),
                        visibility(op.visibility()),
                        integer(op.ordinal()),
                        model_origin(op.origin()),
                        optional_text(op.docs()),
                        array(op.generic_parameters(), |parameter| {
                            record(
                                "GenericParameter",
                                vec![
                                    text(parameter.name()),
                                    array(parameter.bounds(), |bound| text(bound)),
                                ],
                            )
                        }),
                        V::Bool(op.has_default()),
                        V::Bool(op.requires_self_send()),
                    ],
                )
            })],
        ),
    };
    record(
        "Declaration",
        vec![
            text(value.identity()),
            text(value.module()),
            visibility(value.visibility()),
            array(value.generic_parameters(), |p| {
                record(
                    "GenericParameter",
                    vec![text(p.name()), array(p.bounds(), |bound| text(bound))],
                )
            }),
            array(value.bounds(), |b| {
                record("Bound", vec![text(b.binder()), text(b.trait_identity())])
            }),
            model_origin(value.origin()),
            optional_text(value.docs()),
            kind,
        ],
    )
}
fn snapshot(value: &MetaSnapshot) -> V {
    record(
        "Snapshot",
        vec![
            text(value.format()),
            record(
                "Environment",
                vec![
                    text(&value.environment().edition),
                    text(&value.environment().target),
                    text(&value.environment().profile),
                    array(&value.environment().capabilities, |value| text(value)),
                    array(&value.environment().features, |value| text(value)),
                    array(&value.environment().packages, |value| text(value)),
                ],
            ),
            array(value.roots(), |root| {
                record("Root", vec![text(root.package()), text(root.module())])
            }),
            array(value.modules(), |module| {
                record(
                    "Module",
                    vec![text(module.identity()), optional_text(module.docs())],
                )
            }),
            array(value.declarations(), declaration),
            array(value.implementations(), |implementation| {
                record(
                    "Implementation",
                    vec![
                        text(&implementation.module),
                        text(&implementation.target),
                        text(&implementation.trait_identity),
                        array(&implementation.arguments, |value| text(value)),
                        array(&implementation.generic_parameters, |parameter| {
                            record(
                                "GenericParameter",
                                vec![
                                    text(parameter.name()),
                                    array(parameter.bounds(), |bound| text(bound)),
                                ],
                            )
                        }),
                        model_origin(&implementation.origin),
                        optional_text(implementation.docs.as_deref()),
                    ],
                )
            }),
        ],
    )
}
fn limits(value: MetaLimits) -> V {
    record(
        "Limits",
        vec![
            integer(value.steps()),
            integer(value.memory_bytes()),
            integer(value.output_bytes()),
        ],
    )
}

pub fn generate_request(request: &MetaRequest) -> V {
    record(
        "GenerateRequest",
        vec![
            snapshot(request.snapshot()),
            array(request.inputs(), |input| {
                record(
                    "Input",
                    vec![
                        text(input.name()),
                        array(input.bytes(), |byte| V::Byte(*byte)),
                        text(input.hash()),
                    ],
                )
            }),
            array(request.outputs(), |output| {
                record(
                    "OutputSpec",
                    vec![text(output.path()), text(output.module())],
                )
            }),
            limits(request.limits()),
        ],
    )
}

pub fn derive_request(
    snapshot_value: &MetaSnapshot,
    target: &str,
    module: &str,
    trait_identity: &str,
    bounds: &[String],
    request_span: MetaSpan,
    execution_limits: MetaLimits,
) -> V {
    record(
        "DeriveRequest",
        vec![
            snapshot(snapshot_value),
            text(target),
            text(module),
            text(trait_identity),
            array(bounds, |bound| text(bound)),
            span(request_span),
            limits(execution_limits),
        ],
    )
}

fn invalid(message: &str) -> MetaVmError {
    MetaVmError::StructuredOutput(message.into())
}

fn fields<'a, const N: usize>(value: &'a V, expected: &str) -> Result<&'a [V; N], MetaVmError> {
    let V::Record { name, values } = value else {
        return Err(invalid("expected a companion record"));
    };
    if name != expected {
        return Err(invalid("response has the wrong companion record type"));
    }
    values
        .as_slice()
        .try_into()
        .map_err(|_| invalid("response record has the wrong field count"))
}

fn string(value: &V) -> Result<&str, MetaVmError> {
    let V::String(value) = value else {
        return Err(invalid("expected a string"));
    };
    Ok(value)
}

fn unsigned(value: &V) -> Result<u32, MetaVmError> {
    let V::Integer(value) = value else {
        return Err(invalid("expected a span offset"));
    };
    u32::try_from(*value).map_err(|_| invalid("span offset is not UInt32"))
}

fn response(outcome: &VmOutcome, request_limits: MetaLimits) -> Result<&V, MetaVmError> {
    match outcome {
        VmOutcome::Panicked(panic) => Err(MetaVmError::ProviderPanic {
            code: panic.code.code().into(),
            message: panic.message.clone(),
        }),
        VmOutcome::Returned(V::ResultOk(value)) => Ok(value),
        VmOutcome::Returned(V::ResultErr(value)) => {
            let V::Variant {
                name,
                variant,
                values,
            } = value.as_ref()
            else {
                return Err(invalid("expected meta.Error"));
            };
            if name != "Error" {
                return Err(invalid("expected meta.Error"));
            }
            if *variant == 4 && values.is_empty() {
                return Err(MetaVmError::ReportedOutputLimit {
                    limit: request_limits.output_bytes(),
                });
            }
            let [value] = values.as_slice() else {
                return Err(invalid("invalid meta.Error payload"));
            };
            let value = string(value)?.to_owned();
            Err(match variant {
                0 => MetaVmError::ProviderContract(MetaContractError::UnknownInput(value)),
                1 => MetaVmError::ProviderContract(MetaContractError::UnknownOutput(value)),
                2 => MetaVmError::ProviderContract(MetaContractError::DuplicateOutput(value)),
                3 => MetaVmError::ProviderContract(MetaContractError::MissingOutput(value)),
                5 | 6 => MetaVmError::ProviderDiagnostics(vec![MetaProviderDiagnostic {
                    origin: None,
                    severity: MetaDiagnosticSeverity::Error,
                    message: value,
                }]),
                _ => invalid("unknown meta.Error variant"),
            })
        }
        _ => Err(invalid("provider did not return a companion Result")),
    }
}

fn origin(value: &V) -> Result<MetaSpan, MetaVmError> {
    let [file, start, end] = fields(value, "Span")?;
    MetaSpan::new(unsigned(file)?, unsigned(start)?, unsigned(end)?)
        .map_err(|_| invalid("invalid source span"))
}

fn authorized_span(snapshot: &MetaSnapshot, value: MetaSpan) -> bool {
    let contains = |origin: &MetaOrigin| {
        origin.source_span().is_some_and(|owner| {
            owner.file() == value.file()
                && owner.start() <= value.start()
                && value.end() <= owner.end()
        })
    };
    snapshot.declarations().iter().any(|declaration| {
        contains(declaration.origin()) || match declaration.kind() {
            MetaDeclarationKind::Record(fields) => fields.iter().any(|field| contains(field.origin())),
            MetaDeclarationKind::Enum(variants) => variants.iter().any(|variant| {
                contains(variant.origin()) || matches!(variant.payload(), MetaVariantPayload::Record(fields) if fields.iter().any(|field| contains(field.origin())))
            }),
            MetaDeclarationKind::Trait(operations) => operations.iter().any(|operation| contains(operation.origin())),
            _ => false,
        }
    }) || snapshot.implementations().iter().any(|implementation| contains(&implementation.origin))
}

fn diagnostics(
    value: &V,
    snapshot: &MetaSnapshot,
    request_span: Option<MetaSpan>,
) -> Result<Vec<MetaProviderDiagnostic>, MetaVmError> {
    let V::Array(values) = value else {
        return Err(invalid("expected a diagnostic array"));
    };
    let mut diagnostics = Vec::with_capacity(values.len());
    for value in values {
        let [severity, message, source] = fields(value, "Diagnostic")?;
        let V::Variant {
            name,
            variant,
            values,
        } = severity
        else {
            return Err(invalid("expected DiagnosticSeverity"));
        };
        if name != "DiagnosticSeverity" || !values.is_empty() {
            return Err(invalid("invalid DiagnosticSeverity"));
        }
        let severity = match variant {
            0 => MetaDiagnosticSeverity::Note,
            1 => MetaDiagnosticSeverity::Warning,
            2 => MetaDiagnosticSeverity::Error,
            _ => return Err(invalid("invalid DiagnosticSeverity")),
        };
        let origin = match source {
            V::OptionNone => None,
            V::OptionSome(value) => {
                let span = origin(value)?;
                if !authorized_span(snapshot, span)
                    && !request_span.is_some_and(|request| {
                        request.file() == span.file()
                            && request.start() <= span.start()
                            && span.end() <= request.end()
                    })
                {
                    return Err(invalid(
                        "diagnostic origin is outside the authorized snapshot",
                    ));
                }
                Some(span)
            }
            _ => return Err(invalid("invalid diagnostic origin")),
        };
        let message = string(message)?;
        if message.is_empty()
            || message
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err(invalid("invalid diagnostic message"));
        }
        diagnostics.push(MetaProviderDiagnostic {
            origin,
            severity,
            message: message.into(),
        });
    }
    diagnostics.sort();
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == MetaDiagnosticSeverity::Error)
    {
        return Err(MetaVmError::ProviderDiagnostics(diagnostics));
    }
    Ok(diagnostics)
}

pub fn generate_response(
    outcome: &VmOutcome,
    request: &MetaRequest,
) -> Result<MetaResponse, MetaVmError> {
    let [outputs, messages] = fields(response(outcome, request.limits())?, "GenerateResponse")?;
    let diagnostics = diagnostics(messages, request.snapshot(), None)?;
    let V::Map(outputs) = outputs else {
        return Err(invalid("expected an output map"));
    };
    let mut builder = request.clone().into_source_builder();
    for (path, source) in outputs {
        let path = string(path)?;
        let module = request
            .outputs()
            .iter()
            .find(|output| output.path() == path)
            .ok_or_else(|| {
                MetaVmError::ProviderContract(MetaContractError::UnknownOutput(path.into()))
            })?
            .module();
        builder
            .add_source(path, module, string(source)?.as_bytes())
            .map_err(MetaVmError::ProviderContract)?;
    }
    builder
        .finish()
        .and_then(|response| response.with_diagnostics(diagnostics))
        .map_err(MetaVmError::ProviderContract)
}

pub fn measure_generate_output(
    outcome: &VmOutcome,
    request: &MetaRequest,
) -> Result<u64, MetaVmError> {
    let [outputs, messages] = fields(response(outcome, request.limits())?, "GenerateResponse")?;
    let V::Map(outputs) = outputs else {
        return Err(invalid("expected an output map"));
    };
    let mut ordered = std::collections::BTreeMap::new();
    for (path, source) in outputs {
        if ordered.insert(string(path)?, string(source)?).is_some() {
            return Err(invalid("duplicate output path"));
        }
    }
    #[derive(serde::Serialize)]
    struct GenerateResponse<'a> {
        outputs: std::collections::BTreeMap<&'a str, &'a str>,
        diagnostics: Vec<MetaProviderDiagnostic>,
    }
    crate::meta_vm::canonical_output_bytes(&GenerateResponse {
        outputs: ordered,
        diagnostics: diagnostics(messages, request.snapshot(), None)?,
    })
}

pub fn measure_derive_output(
    outcome: &VmOutcome,
    snapshot: &MetaSnapshot,
    request_limits: MetaLimits,
    request_span: MetaSpan,
) -> Result<u64, MetaVmError> {
    crate::meta_vm::canonical_output_bytes(&derive_response(
        outcome,
        snapshot,
        request_limits,
        request_span,
    )?)
}

#[derive(serde::Serialize)]
pub struct DeriveSourceResponse<'a> {
    pub source: &'a str,
    pub diagnostics: Vec<MetaProviderDiagnostic>,
    pub mappings: Vec<MetaSourceMapEntry>,
}

pub fn derive_response<'a>(
    outcome: &'a VmOutcome,
    snapshot: &MetaSnapshot,
    execution_limits: MetaLimits,
    request_span: MetaSpan,
) -> Result<DeriveSourceResponse<'a>, MetaVmError> {
    let [source, messages, mappings] =
        fields(response(outcome, execution_limits)?, "DeriveResponse")?;
    let diagnostics = diagnostics(messages, snapshot, Some(request_span))?;
    let V::Array(mappings) = mappings else {
        return Err(invalid("expected a source mapping array"));
    };
    let mappings = mappings
        .iter()
        .map(|mapping| {
            let [start, end, file, origin_start, origin_end] = fields(mapping, "SourceMap")?;
            let span = MetaSpan::new(
                unsigned(file)?,
                unsigned(origin_start)?,
                unsigned(origin_end)?,
            )
            .map_err(|_| invalid("invalid source mapping origin"))?;
            if !(authorized_span(snapshot, span)
                || request_span.file() == span.file()
                    && request_span.start() <= span.start()
                    && span.end() <= request_span.end())
            {
                return Err(invalid("mapping origin is outside the authorized snapshot"));
            }
            MetaSourceMapEntry::new(unsigned(start)?, unsigned(end)?, span)
                .map_err(MetaVmError::ProviderContract)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DeriveSourceResponse {
        source: string(source)?,
        mappings,
        diagnostics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_origins_authorize_only_actual_admitted_source_ranges() {
        let field_span = MetaSpan::new(0, 20, 30).unwrap();
        let declaration = MetaDeclaration::new(
            "User",
            "app",
            MetaVisibility::Public,
            [],
            [],
            MetaSpan::new(0, 0, 4).unwrap(),
            None::<String>,
            MetaDeclarationKind::Record(vec![
                MetaField::new(
                    "field",
                    "Int",
                    MetaVisibility::Public,
                    0,
                    field_span,
                    None::<String>,
                )
                .unwrap(),
            ]),
        )
        .unwrap();
        let builtin = MetaDeclaration::new(
            "Builtin",
            "prelude",
            MetaVisibility::Public,
            [],
            [],
            MetaOrigin::Builtin("prelude:Builtin".into()),
            None::<String>,
            MetaDeclarationKind::Trait(vec![]),
        )
        .unwrap();
        let snapshot = MetaSnapshot::new(
            MetaEnvironment::meta(),
            [],
            [
                MetaModule::new("app", None::<String>).unwrap(),
                MetaModule::new("prelude", None::<String>).unwrap(),
            ],
            [declaration, builtin],
        )
        .unwrap();
        assert!(authorized_span(&snapshot, field_span));
        assert!(authorized_span(
            &snapshot,
            MetaSpan::new(0, 21, 25).unwrap()
        ));
        assert!(!authorized_span(
            &snapshot,
            MetaSpan::new(0, 5, 10).unwrap()
        ));
        assert!(!authorized_span(&snapshot, MetaSpan::new(1, 0, 1).unwrap()));
        let builtin_only = MetaSnapshot::new(
            MetaEnvironment::meta(),
            [],
            [MetaModule::new("prelude", None::<String>).unwrap()],
            snapshot
                .declarations()
                .iter()
                .filter(|declaration| declaration.module() == "prelude")
                .cloned(),
        )
        .unwrap();
        assert!(!authorized_span(
            &builtin_only,
            MetaSpan::new(0, 0, 0).unwrap()
        ));
    }
}
