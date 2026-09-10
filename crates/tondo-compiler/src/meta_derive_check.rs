//! Recheck each added derive bound against the same sealed compilation inputs.

use super::*;
use crate::meta_frontend::GeneratedDeriveSource;
use crate::source::SourceInput;

pub(super) fn validate(
    request: &CompilationRequest,
    generated: &[GeneratedDeriveSource],
) -> Result<Option<CompilationOutput>, DriverError> {
    let mut probes = Vec::new();
    for (index, source) in generated.iter().enumerate() {
        for bound in crate::meta_derive_bounds::introduced(&source.bytes, &source.baseline_header)
            .map_err(|error| DriverError::Invariant(error.to_string()))?
        {
            probes.push((index, bound));
        }
    }
    if probes.is_empty() {
        return Ok(None);
    }
    if probes.len() > request.limits.max_trait_obligations as usize {
        return Ok(Some(syntax_resource_output(
            request,
            request.root,
            "derive bound rechecks",
            0,
        )?));
    }
    // A rejected baseline cannot establish necessity. Check never executes main,
    // invokes a provider or publishes a compilation artifact.
    let baseline = execute_with_derives(checking_request(request, generated, None)?, false)?;
    if baseline.status() != CompilationStatus::Success {
        return Ok(Some(baseline));
    }
    for (index, bound) in probes {
        let output = execute_with_derives(
            checking_request(request, generated, Some((index, bound.removal)))?,
            false,
        )?;
        if output.status() == CompilationStatus::Success {
            let mut bag = DiagnosticBag::new();
            bag.push(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E2105")?,
                format!(
                    "derive provider introduced redundant bound `{}`",
                    bound.identity
                ),
                PrimaryLocation::Source(generated[index].request_span),
            )?);
            return Ok(Some(CompilationOutput {
                status: CompilationStatus::Rejected,
                exit_code: 1,
                diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
                stdout: Vec::new(),
                diagnostic_trace: None,
                mir_summary: None,
                bytecode: None,
                semantic_model: None,
                products: None,
            }));
        }
        if output
            .diagnostics()
            .diagnostics()
            .iter()
            .any(|diagnostic| matches!(diagnostic.code(), "T0002" | "E2105"))
        {
            // Exhaustion or incomplete checking cannot establish necessity.
            return Ok(Some(output));
        }
    }
    Ok(None)
}

fn checking_request(
    original: &CompilationRequest,
    generated: &[GeneratedDeriveSource],
    removal: Option<(usize, TextRange)>,
) -> Result<CompilationRequest, DriverError> {
    let mut sources = clone_source_database(&original.sources, None)?;
    let mut packages = original.packages.clone();
    for (index, source) in generated.iter().enumerate() {
        let mut bytes = source.bytes.clone();
        if let Some((_, range)) = removal.filter(|(candidate, _)| *candidate == index) {
            // Preserve offsets and source maps for every unchanged token.
            for byte in &mut bytes[range.start() as usize..range.end() as usize] {
                if !matches!(*byte, b'\n' | b'\r') {
                    *byte = b' ';
                }
            }
        }
        packages.register_generated_source(
            &source.owner,
            source.source_id.clone(),
            source.module.clone(),
        )?;
        sources.add(
            SourceInput::new(
                source.source_id.clone(),
                source.module.clone(),
                crate::source::LogicalPath::new(&source.path)?,
                crate::source::SourceOrigin::GeneratedMeta,
                bytes,
            )
            .with_diagnostic_mappings(&source.diagnostic_mappings),
        )?;
    }
    let mut request = CompilationRequest::new(
        Operation::Check,
        original.edition,
        original.target.clone(),
        original.profile,
        original.capabilities.clone(),
        original.diagnostic_format,
        original.source_form,
        original.limits,
        packages,
        sources,
        original.root,
    )?;
    request.build_inputs = original.build_inputs.clone();
    request.documentation_fixture = original.documentation_fixture;
    request.warning_profiles = original.warning_profiles.clone();
    request.test_source_classes = original.test_source_classes.clone();
    request.test_package_names = original.test_package_names.clone();
    request.sealed_production = original.sealed_production.clone();
    request.derive_bound_probe = true;
    Ok(request)
}
