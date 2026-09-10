//! One shared pre-generation semantic base for ordinary project producers.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{DiagnosticBag, DiagnosticReport, Severity};
use crate::driver::{CompilationRequest, DriverError};
use crate::hir::{TypeLoweringLimits, lower_types};
use crate::meta::{MetaDiagnosticCode, MetaDiagnosticSeverity, MetaLimits, MetaRoot, MetaSnapshot};
use crate::meta_atomic::{
    AcceptedMetaResult, MetaBuildContext, MetaInvocation, MetaProducerKind, MetaProviderIdentity,
};
use crate::meta_diagnostics::{
    MetaDiagnosticEntry, generator_execution_entry, render_meta_diagnostics,
};
use crate::meta_frontend::{DeriveExpansion, DeriveFrontendError, expand_derives, provider_origin};
use crate::meta_generate::execute_generator_plan;
use crate::package::PackageId;
use crate::project_meta::ClosedSourceMeta;
use crate::resolve::{Visibility, resolve};
use crate::source::{
    FileId, LogicalPath, ModulePath, SourceDiagnosticMapping, SourceId, SourceInput, SourceOrigin,
    TextRange,
};
use crate::syntax::Parsed;

#[path = "meta_projection.rs"]
mod projection;

pub(crate) struct GeneratedInput {
    pub owner: PackageId,
    pub source: SourceInput,
}

pub(crate) struct GenerationExpansion {
    pub sources: Vec<GeneratedInput>,
    pub derives: DeriveExpansion,
    pub accepted: Vec<AcceptedMetaResult>,
    pub diagnostics: DiagnosticReport,
}

pub(crate) enum GenerationError {
    Diagnostics(DiagnosticReport),
    Driver(DriverError),
}

impl From<DriverError> for GenerationError {
    fn from(error: DriverError) -> Self {
        Self::Driver(error)
    }
}

/// Resolve only authored modules needed by generator roots and derive requests.
/// Consumers of generated code participate in the final compilation, after the
/// complete output batch has been accepted. No producer sees another's output.
pub(crate) fn expand(
    request: &CompilationRequest,
    parsed: &[(FileId, Parsed)],
    meta: &ClosedSourceMeta,
    derive_providers: &crate::meta_provider::SourceDeriveProviders,
) -> Result<GenerationExpansion, GenerationError> {
    let packages = request.packages();
    let sources = request.sources();
    let mut roots = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for generator in &meta.plan.lock.generators {
        for output in &generator.outputs {
            outputs.insert((generator.owner_package.clone(), output.module.clone()));
            if sources.iter().any(|(_, source)| {
                source.path().as_str() == output.logical_path
                    && packages
                        .package_for_source(source.source_id())
                        .is_some_and(|owner| owner.id().as_str() == generator.owner_package)
            }) {
                return Err(meta_failure(
                    request,
                    MetaDiagnosticCode::InvalidGeneratedSource,
                    format!(
                        "generator `{}` replaces authored path `{}`",
                        generator.id, output.logical_path
                    ),
                )?);
            }
        }
        for root in &generator.model_roots {
            let owner = PackageId::new(&root.package).map_err(DriverError::from)?;
            let path = ModulePath::new(&root.module).map_err(DriverError::from)?;
            let module = packages.module(&owner, &path).ok_or_else(|| {
                DriverError::Invariant(format!("validated meta root `{owner}::{path}` is absent"))
            })?;
            roots.insert(module);
        }
    }
    let projected = projection::project(request, parsed, &roots, &outputs)?;
    let selected_parsed = || projected.iter().map(|(file, parsed)| (*file, parsed));
    let (resolved, diagnostics) = resolve(
        packages,
        sources,
        selected_parsed(),
        request.limits().max_diagnostics as usize,
    )
    .map_err(DriverError::from)?
    .into_parts();
    reject_diagnostics(request, diagnostics)?;
    let (mut hir, diagnostics) = lower_types(
        packages,
        sources,
        selected_parsed(),
        &resolved,
        TypeLoweringLimits {
            max_type_nodes: request.limits().max_type_nodes,
            max_trait_obligations: request.limits().max_trait_obligations,
            max_diagnostics: request.limits().max_diagnostics as usize,
        },
    )
    .map_err(DriverError::from)?
    .into_parts();
    reject_diagnostics(request, diagnostics)?;
    hir.restore_meta_declaration_spans(&projection::authored_spans(sources, parsed, &projected)?);
    let limits = request.limits();
    let environment = crate::meta_snapshot::environment(request);
    let derives = expand_derives(
        packages,
        sources,
        parsed,
        &resolved,
        &hir,
        crate::meta_frontend::DeriveSettings {
            limits: MetaLimits::new(
                limits.max_vm_steps.max(1),
                limits.max_vm_heap_bytes.max(1),
                u64::from(limits.max_source_bytes.max(1)),
            )
            .map_err(invariant)?,
            source_providers: derive_providers,
            environment: &environment,
        },
    )
    .map_err(|error| derive_error(request, error))?;
    let mut snapshots = BTreeMap::new();
    for generator in &meta.plan.lock.generators {
        let roots = generator
            .model_roots
            .iter()
            .map(|root| MetaRoot::new(&root.package, &root.module))
            .collect::<Result<Vec<_>, _>>()
            .map_err(invariant)?;
        let seeds = resolved
            .symbols()
            .filter(|symbol| {
                symbol.visibility() == Visibility::Public
                    && generator.model_roots.iter().any(|root| {
                        root.package == symbol.identity().package().as_str()
                            && root.module == symbol.identity().module().as_str()
                    })
                    && (hir.declaration(symbol.id()).is_some()
                        || hir
                            .callable(crate::hir::HirCallableId::Symbol(symbol.id()))
                            .is_some()
                        || hir.constant(symbol.id()).is_some())
            })
            .map(|symbol| symbol.id())
            .collect::<Vec<_>>();
        let snapshot = if roots.is_empty() {
            MetaSnapshot::new(environment.clone(), [], [], []).map_err(invariant)?
        } else {
            crate::meta_snapshot::SnapshotInput {
                packages,
                owner: &PackageId::new(&generator.owner_package).map_err(DriverError::from)?,
                sources,
                parsed,
                resolved: &resolved,
                hir: &hir,
                environment: &environment,
            }
            .build(roots, &seeds, &[], &[], None)
            .map_err(|error| derive_error(request, error))?
        };
        snapshots.insert(generator.id.clone(), snapshot);
    }
    // All snapshots are sealed before any generator executes. Source provider
    // compilation consumes only its separately locked meta dependency graph.
    let registry = match meta.compile_generators(limits) {
        Ok(registry) => registry,
        Err(error) => {
            return Err(meta_failure(
                request,
                MetaDiagnosticCode::InvalidGeneratedSource,
                error.to_string(),
            )?);
        }
    };
    let inputs = meta
        .plan
        .manifest
        .generator_inputs
        .iter()
        .map(|input| (input.name.clone(), meta.supplied[&input.path].to_vec()))
        .collect();
    let mut effective_generators = meta.plan.lock.generators.clone();
    for generator in &mut effective_generators {
        generator.limits.steps = generator.limits.steps.min(limits.max_vm_steps.max(1));
        generator.limits.memory_bytes = generator
            .limits
            .memory_bytes
            .min(limits.max_vm_heap_bytes.max(1));
        generator.limits.output_bytes = generator
            .limits
            .output_bytes
            .min(u64::from(limits.max_source_bytes.max(1)));
    }
    let execution = execute_generator_plan(
        &effective_generators,
        &meta.plan.lock.generator_inputs,
        &inputs,
        &snapshots,
        &registry,
    )
    .map_err(|error| generator_error(request, error))?;
    let capabilities = request
        .capabilities()
        .iter()
        .map(|value| value.as_str().to_owned())
        .collect::<Vec<_>>();
    let features = request
        .build_inputs()
        .features()
        .iter()
        .map(|value| value.as_str().to_owned())
        .collect::<Vec<_>>();
    let compiler = crate::project::bootstrap_standard_hash();
    let mut accepted = Vec::new();
    let mut generated = Vec::new();
    let mut messages = Vec::new();
    for result in execution.results() {
        let locked = meta
            .plan
            .lock
            .generators
            .iter()
            .find(|generator| generator.id == result.generator_id())
            .expect("execution retains exact generator identities");
        let invocation = MetaInvocation::new(
            MetaBuildContext {
                compiler: &compiler,
                edition: request.edition().as_str(),
                target: request.target().name(),
                profile: request.profile().as_str(),
                capabilities: &capabilities,
                features: &features,
            },
            MetaProviderIdentity {
                kind: MetaProducerKind::Generator,
                id: &locked.id,
                package: &locked.provider_package,
                hash: &locked.provider_hash,
                entry: &locked.entry,
            },
            locked.model_roots.clone(),
            result.request(),
        )
        .map_err(invariant)?;
        let result = invocation
            .accept(result.response().clone())
            .map_err(invariant)?;
        for diagnostic in result.response().diagnostics() {
            let origin = diagnostic
                .origin
                .map(|origin| {
                    provider_origin(
                        sources,
                        origin,
                        sources
                            .span(request.root(), TextRange::empty(0))
                            .expect("admitted root"),
                    )
                })
                .transpose()
                .map_err(|error| derive_error(request, error))?;
            let severity = match diagnostic.severity {
                MetaDiagnosticSeverity::Note => Severity::Note,
                MetaDiagnosticSeverity::Warning => Severity::Warning,
                MetaDiagnosticSeverity::Error => Severity::Error,
            };
            messages.push(
                MetaDiagnosticEntry::new(
                    MetaDiagnosticCode::InvalidGeneratedSource,
                    &diagnostic.message,
                    origin,
                )
                .with_severity(severity),
            );
        }
        for output in &result.record().outputs {
            let source = result
                .response()
                .output(&output.path)
                .expect("accepted output set");
            let mappings = source
                .mappings()
                .iter()
                .map(|mapping| {
                    Ok(SourceDiagnosticMapping {
                        generated: TextRange::new(
                            mapping.generated_start(),
                            mapping.generated_end(),
                        )
                        .map_err(invariant)?,
                        origin: provider_origin(
                            sources,
                            mapping.origin(),
                            sources
                                .span(request.root(), TextRange::empty(0))
                                .expect("admitted root"),
                        )
                        .map_err(|error| derive_error(request, error))?,
                    })
                })
                .collect::<Result<Vec<_>, GenerationError>>()?;
            generated.push(GeneratedInput {
                owner: PackageId::new(&locked.owner_package).map_err(DriverError::from)?,
                source: SourceInput::new(
                    SourceId::new(&output.source_id).map_err(DriverError::from)?,
                    ModulePath::new(&output.module).map_err(DriverError::from)?,
                    LogicalPath::new(&output.path).map_err(DriverError::from)?,
                    SourceOrigin::GeneratedMeta,
                    source.bytes().to_vec(),
                )
                .with_diagnostic_mappings(&mappings),
            });
        }
        accepted.push(result);
    }
    let diagnostics = render_meta_diagnostics(
        messages,
        request.target().diagnostic_source_id().clone(),
        sources,
    )
    .map_err(DriverError::from)?;
    Ok(GenerationExpansion {
        sources: generated,
        derives,
        accepted,
        diagnostics,
    })
}

fn generator_error(
    request: &CompilationRequest,
    error: crate::meta_generate::GeneratorExecutionError,
) -> GenerationError {
    let mut entries = Vec::new();
    if let crate::meta_generate::GeneratorExecutionError::ProviderVm {
        source: crate::meta_vm::MetaVmError::ProviderDiagnostics(messages),
        ..
    } = &error
    {
        for message in messages {
            let primary = match message
                .origin
                .map(|origin| {
                    provider_origin(
                        request.sources(),
                        origin,
                        request
                            .sources()
                            .span(request.root(), TextRange::empty(0))
                            .expect("admitted root"),
                    )
                })
                .transpose()
            {
                Ok(primary) => primary,
                Err(error) => return derive_error(request, error),
            };
            let severity = match message.severity {
                MetaDiagnosticSeverity::Error => Severity::Error,
                MetaDiagnosticSeverity::Warning => Severity::Warning,
                MetaDiagnosticSeverity::Note => Severity::Note,
            };
            entries.push(
                MetaDiagnosticEntry::new(
                    MetaDiagnosticCode::InvalidGeneratedSource,
                    &message.message,
                    primary,
                )
                .with_severity(severity),
            );
        }
    } else {
        entries.push(generator_execution_entry(&error));
    }
    match render_meta_diagnostics(
        entries,
        request.target().diagnostic_source_id().clone(),
        request.sources(),
    ) {
        Ok(report) => GenerationError::Diagnostics(report),
        Err(error) => GenerationError::Driver(error.into()),
    }
}

fn reject_diagnostics(
    request: &CompilationRequest,
    diagnostics: Vec<crate::diagnostics::Diagnostic>,
) -> Result<(), GenerationError> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let mut bag = DiagnosticBag::new();
    bag.extend(diagnostics);
    Err(GenerationError::Diagnostics(
        bag.resolve(request.edition().as_str(), request.sources())
            .map_err(DriverError::from)?,
    ))
}

fn meta_failure(
    request: &CompilationRequest,
    code: MetaDiagnosticCode,
    message: String,
) -> Result<GenerationError, DriverError> {
    Ok(GenerationError::Diagnostics(render_meta_diagnostics(
        [MetaDiagnosticEntry::new(code, message, None)],
        request.target().diagnostic_source_id().clone(),
        request.sources(),
    )?))
}

fn derive_error(request: &CompilationRequest, error: DeriveFrontendError) -> GenerationError {
    match error {
        DeriveFrontendError::Invariant(message) => invariant(message),
        DeriveFrontendError::Diagnostics(entries) => match render_meta_diagnostics(
            entries,
            request.target().diagnostic_source_id().clone(),
            request.sources(),
        ) {
            Ok(report) => GenerationError::Diagnostics(report),
            Err(error) => GenerationError::Driver(error.into()),
        },
    }
}

fn invariant(error: impl std::fmt::Display) -> GenerationError {
    GenerationError::Driver(DriverError::Invariant(error.to_string()))
}
