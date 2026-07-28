use serde_json::{Value, json};
use tondo_compiler::driver::{CompilationStatus, execute};
use tondo_compiler::semantic::SemanticEntity;
use tondo_compiler::source::TextRange;
use tondo_conformance::manifest::SemanticQuery;
use tondo_conformance::protocol::{
    AdapterRequest, CompilationState, Observation, WireSemanticAction,
};

use crate::{normative_diagnostics, source_files_by_path, source_request};

pub(crate) fn observe_semantic(
    request: &AdapterRequest,
    action: &WireSemanticAction,
) -> Result<Observation, String> {
    let output =
        execute(source_request(request, &action.source)?).map_err(|error| error.to_string())?;
    let compilation = match output.status() {
        CompilationStatus::Success => CompilationState::Success,
        CompilationStatus::Rejected => CompilationState::Rejected,
    };
    let model = output
        .semantic_model()
        .ok_or_else(|| "semantic query case did not reach a semantic snapshot".to_owned())?;
    let by_path = source_files_by_path(&action.source);
    let mut results = Vec::with_capacity(action.queries.len());
    for query in &action.queries {
        results.push(run_query(model, &by_path, query)?);
    }
    let diagnostics = normative_diagnostics(output.diagnostics())?;
    Ok(Observation {
        compilation,
        exit_code: i32::from(output.exit_code()),
        diagnostics,
        stdout_hex: String::new(),
        stderr_hex: String::new(),
        formatted_hex: None,
        data: json!({
            "expression_check_complete": model.expression_check_complete(),
            "queries": results
        }),
    })
}

fn run_query(
    model: &tondo_compiler::semantic::SemanticModel,
    by_path: &std::collections::BTreeMap<&str, &tondo_conformance::protocol::WireSource>,
    query: &SemanticQuery,
) -> Result<Value, String> {
    match query {
        SemanticQuery::FormattedAst => Ok(json!({
            "query": "formatted-ast",
            "available": false
        })),
        SemanticQuery::ExpressionType { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let ty = model
                .expression_type_at(file_id, range)
                .map(|ty| model.canonical_type(ty))
                .transpose()
                .map_err(|error| error.to_string())?
                .flatten();
            Ok(json!({"query": "expression-type", "type": ty}))
        }
        SemanticQuery::Entities { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let entities = model
                .entities_at(file_id, range)
                .iter()
                .map(entity_json)
                .collect::<Vec<_>>();
            Ok(json!({"query": "entities", "entities": entities}))
        }
        SemanticQuery::References { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let entity = model
                .entities_at(file_id, range)
                .into_iter()
                .next()
                .ok_or_else(|| "reference query selected no entity".to_owned())?;
            let references = model
                .references(&entity)
                .iter()
                .map(|span| span_json(model.sources(), *span))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({"query": "references", "references": references}))
        }
        SemanticQuery::Signature { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let entity = model
                .entities_at(file_id, range)
                .into_iter()
                .next()
                .ok_or_else(|| "signature query selected no entity".to_owned())?;
            let signature = model
                .signature(&entity)
                .map(|signature| {
                    model
                        .canonical_type(signature.function_type())
                        .map_err(|error| error.to_string())
                })
                .transpose()?
                .flatten();
            Ok(json!({"query": "signature", "signature": signature}))
        }
        SemanticQuery::TypeMembers { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let ty = model
                .type_annotation_at(file_id, range)
                .or_else(|| model.expression_type_at(file_id, range));
            let members = ty
                .map(|ty| model.type_members(ty))
                .transpose()
                .map_err(|error| error.to_string())?
                .map(|members| format!("{members:?}"));
            Ok(json!({"query": "type-members", "members": members}))
        }
        SemanticQuery::ClosedCallErrors { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let errors = model
                .closed_call_errors_at(file_id, range)
                .map_err(|error| error.to_string())?
                .map(|errors| {
                    errors
                        .into_iter()
                        .map(|ty| {
                            model
                                .canonical_type(ty)
                                .map_err(|error| error.to_string())
                                .map(|value| value.unwrap_or_else(|| "<unknown>".into()))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            Ok(json!({"query": "closed-call-errors", "errors": errors}))
        }
        SemanticQuery::TypeFacts { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let ty = model
                .type_annotation_at(file_id, range)
                .or_else(|| model.expression_type_at(file_id, range));
            let result = ty
                .map(|ty| {
                    json!({
                        "type": model.canonical_type(ty).ok().flatten(),
                        "terminal": model.terminal_status(ty).map(|status| format!("{status:?}"))
                    })
                })
                .unwrap_or(Value::Null);
            Ok(json!({"query": "type-facts", "facts": result}))
        }
        SemanticQuery::ExpressionFacts { file, start, end } => {
            let (file_id, range) = locate(model, by_path, file, *start, *end)?;
            let result = model.expression_at(file_id, range).map(|(id, expression)| {
                json!({
                    "id": id.index(),
                    "kind": format!("{:?}", expression.kind()),
                    "type": model.canonical_type(expression.ty()).ok().flatten()
                })
            });
            Ok(json!({"query": "expression-facts", "facts": result}))
        }
    }
}

fn locate(
    model: &tondo_compiler::semantic::SemanticModel,
    by_path: &std::collections::BTreeMap<&str, &tondo_conformance::protocol::WireSource>,
    path: &str,
    start: u32,
    end: u32,
) -> Result<(tondo_compiler::source::FileId, TextRange), String> {
    let source = by_path
        .get(path)
        .ok_or_else(|| format!("query uses unknown file `{path}`"))?;
    let file = model
        .sources()
        .iter()
        .find(|(_, file)| {
            file.path().as_str() == source.logical_path
                && file.source_id().as_str() == source.source_id
        })
        .map(|(id, _)| id)
        .ok_or_else(|| format!("semantic snapshot lacks `{path}`"))?;
    let range = TextRange::new(start, end).map_err(|error| error.to_string())?;
    Ok((file, range))
}

fn entity_json(entity: &SemanticEntity) -> Value {
    json!({"kind": format!("{entity:?}")})
}

fn span_json(
    sources: &tondo_compiler::source::SourceDatabase,
    span: tondo_compiler::source::Span,
) -> Result<Value, String> {
    let file = sources
        .get(span.file())
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "source_id": file.source_id().as_str(),
        "module": file.module().as_str(),
        "file": file.path().as_str(),
        "start": span.range().start(),
        "end": span.range().end()
    }))
}
