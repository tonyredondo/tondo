//! Atomic execution boundary for locked derive providers.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use tondo_vm::runtime::{RuntimeValue, VmOutcome};

use crate::meta::{
    MetaContractError, MetaLimits, MetaOutputSpec, MetaRequest, MetaResponse, MetaSnapshot,
    MetaSourceMapEntry, ValidatedDerive,
};
use crate::meta_vm::{MetaEntryKind, MetaVmArtifact, MetaVmError, MetaVmLimits};
use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use crate::syntax::{
    LexLimits, LexMode, MappedFormattedSource, ParseLimits, ParseMode, SyntaxKind, TokenKind,
    format_parsed_with_mappings, lex_with_limits, parse,
};

#[derive(Debug, Clone)]
pub struct DeriveProviderRequest<'a> {
    target: &'a str,
    module: &'a str,
    trait_identity: &'a str,
    provider_identity: &'a str,
    introduced_bounds: &'a [String],
    snapshot: &'a MetaSnapshot,
}

impl<'a> DeriveProviderRequest<'a> {
    pub fn target(&self) -> &'a str {
        self.target
    }

    pub fn module(&self) -> &'a str {
        self.module
    }

    pub fn trait_identity(&self) -> &'a str {
        self.trait_identity
    }

    pub fn provider_identity(&self) -> &'a str {
        self.provider_identity
    }

    pub fn introduced_bounds(&self) -> &'a [String] {
        self.introduced_bounds
    }

    pub fn snapshot(&self) -> &'a MetaSnapshot {
        self.snapshot
    }
}

/// Trusted compiler boundary that specializes a locked provider for one request.
///
/// The compiler receives immutable typed input, but provider code itself runs
/// only through the returned hermetic `tondo-meta` program.
pub trait DeriveProviderCompiler: Send + Sync {
    fn compile(&self, request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String>;
}

#[derive(Default)]
pub struct DeriveProviderRegistry {
    providers: BTreeMap<String, Arc<dyn DeriveProviderCompiler>>,
}

impl DeriveProviderRegistry {
    pub fn insert(
        &mut self,
        identity: impl Into<String>,
        provider: Arc<dyn DeriveProviderCompiler>,
    ) -> Result<(), DeriveExecutionError> {
        let identity = identity.into();
        if identity.is_empty() || identity.chars().any(char::is_control) {
            return Err(DeriveExecutionError::InvalidProviderIdentity(identity));
        }
        match self.providers.entry(identity.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(provider);
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(DeriveExecutionError::DuplicateProvider(identity));
            }
        }
        Ok(())
    }

    fn get(&self, identity: &str) -> Option<&Arc<dyn DeriveProviderCompiler>> {
        self.providers.get(identity)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeriveExecution {
    response: MetaResponse,
    providers: Vec<String>,
    baseline_headers: Vec<String>,
}

impl DeriveExecution {
    pub(crate) fn baseline_headers(&self) -> &[String] {
        &self.baseline_headers
    }

    pub fn response(&self) -> &MetaResponse {
        &self.response
    }

    pub fn providers(&self) -> &[String] {
        &self.providers
    }

    pub fn into_response(self) -> MetaResponse {
        self.response
    }
}

/// Execute a fully validated plan and publish only one complete response.
pub fn execute_derive_plan(
    plan: &[ValidatedDerive],
    snapshot: MetaSnapshot,
    limits: MetaLimits,
    registry: &DeriveProviderRegistry,
) -> Result<DeriveExecution, DeriveExecutionError> {
    execute_derive_plan_with_limits(plan, snapshot, limits, registry, &BTreeMap::new())
}

/// Project providers retain their individual locked budgets. The outer limit
/// also bounds the total source admitted by the surrounding compilation.
pub(crate) fn execute_derive_plan_with_limits(
    plan: &[ValidatedDerive],
    snapshot: MetaSnapshot,
    limits: MetaLimits,
    registry: &DeriveProviderRegistry,
    provider_limits: &BTreeMap<String, MetaLimits>,
) -> Result<DeriveExecution, DeriveExecutionError> {
    let mut keys = BTreeSet::new();
    let mut outputs = Vec::new();
    for derive in plan {
        for (trait_index, validated_trait) in derive.traits().iter().enumerate() {
            let key = (
                derive.request_index(),
                validated_trait.identity().to_owned(),
            );
            if !keys.insert(key) {
                return Err(DeriveExecutionError::DuplicatePlanEntry {
                    request: derive.request_index(),
                    trait_identity: validated_trait.identity().to_owned(),
                });
            }
            outputs.push((
                derive,
                validated_trait,
                output_path(derive.request_index(), trait_index),
            ));
        }
    }
    let output_specs = outputs.iter().map(|(derive, _, path)| {
        MetaOutputSpec::new(path.clone(), derive.target().module().to_owned())
    });
    let request = MetaRequest::new(
        snapshot,
        [],
        output_specs.collect::<Result<Vec<_>, _>>()?,
        limits,
    )?;
    let snapshot = request.snapshot().clone();
    let mut builder = request.into_source_builder();
    let mut providers = Vec::with_capacity(outputs.len());
    let mut baseline_headers = Vec::with_capacity(outputs.len());
    let mut provider_diagnostics = Vec::new();

    for (derive, validated_trait, path) in outputs {
        let limits = provider_limits
            .get(validated_trait.provider())
            .copied()
            .unwrap_or(limits);
        let provider = registry.get(validated_trait.provider()).ok_or_else(|| {
            DeriveExecutionError::MissingProvider(validated_trait.provider().into())
        })?;
        let provider_request = DeriveProviderRequest {
            target: derive.target().identity(),
            module: derive.target().module(),
            trait_identity: validated_trait.identity(),
            provider_identity: validated_trait.provider(),
            introduced_bounds: validated_trait.introduced_bounds(),
            snapshot: &snapshot,
        };
        let program = catch_unwind(AssertUnwindSafe(|| provider.compile(provider_request)))
            .map_err(|_| DeriveExecutionError::ProviderPanicked {
                provider: validated_trait.provider().into(),
            })?
            .map_err(|message| DeriveExecutionError::ProviderFailed {
                provider: validated_trait.provider().into(),
                message,
            })?;
        let target_type = derive_target_type(derive.target());
        let generic_header = derive_generic_header(&snapshot, derive, Some(validated_trait));
        let baseline_header = format!(
            "impl{} {} for {}",
            derive_generic_header(&snapshot, derive, None),
            validated_trait.identity(),
            target_type
        );
        let header = format!(
            "impl{} {} for {}",
            generic_header,
            validated_trait.identity(),
            target_type,
        );
        let vm_error = |source| DeriveExecutionError::ProviderVm {
            provider: validated_trait.provider().into(),
            source,
        };
        let entry_kind = program.entry_kind();
        let program = program
            .load(MetaVmLimits::for_request(limits))
            .map_err(vm_error)?;
        let (source, mappings) = match entry_kind {
            Some(MetaEntryKind::Derive) => {
                use crate::std_meta::source_api;
                let request_span = derive
                    .request_span()
                    .ok_or(DeriveExecutionError::InvalidProviderBody)?
                    .into();
                let request = source_api::derive_request(
                    &snapshot,
                    derive.target().identity(),
                    derive.target().module(),
                    validated_trait.identity(),
                    &derive.written_bounds(),
                    request_span,
                    limits,
                );
                let execution = program
                    .run_with_request(request, |outcome| {
                        source_api::measure_derive_output(outcome, &snapshot, limits, request_span)
                    })
                    .map_err(vm_error)?;
                let response = source_api::derive_response(
                    &execution.outcome,
                    &snapshot,
                    limits,
                    request_span,
                )
                .map_err(vm_error)?;
                let imports = snapshot
                    .declarations()
                    .iter()
                    .flat_map(|declaration| declaration.type_references())
                    .filter_map(|ty| {
                        crate::meta_type::render_type(ty, derive.target().module()).ok()
                    })
                    .flat_map(|(_, imports)| imports)
                    .collect::<Vec<_>>();
                let formatted =
                    format_impl(response.source.as_bytes(), Some(&baseline_header), &imports)?;
                let mappings = crate::meta_source_maps::compose(
                    response.source.as_bytes(),
                    &formatted,
                    &response.mappings,
                )
                .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
                provider_diagnostics.extend(response.diagnostics);
                (formatted.source().bytes().to_vec(), mappings)
            }
            Some(MetaEntryKind::Generate) => {
                return Err(DeriveExecutionError::InvalidProviderResult(
                    validated_trait.provider().into(),
                ));
            }
            None => {
                let execution = program.run().map_err(vm_error)?;
                let VmOutcome::Returned(RuntimeValue::String(body)) = execution.outcome else {
                    return Err(DeriveExecutionError::InvalidProviderResult(
                        validated_trait.provider().into(),
                    ));
                };
                let source = format!("{header} {}\n", body.trim());
                let source = format_impl(source.as_bytes(), None, &[])?
                    .source()
                    .bytes()
                    .to_vec();
                let mappings = generated_source_mappings(
                    &snapshot,
                    derive.target().module(),
                    derive.target().identity(),
                    source.len(),
                )?;
                (source, mappings)
            }
        };
        builder.add_mapped_source(path, derive.target().module(), source, mappings)?;
        providers.push(validated_trait.provider().to_owned());
        baseline_headers.push(baseline_header);
    }

    Ok(DeriveExecution {
        response: builder.finish()?.with_diagnostics(provider_diagnostics)?,
        providers,
        baseline_headers,
    })
}

fn generated_source_mappings(
    snapshot: &MetaSnapshot,
    module: &str,
    target: &str,
    generated_len: usize,
) -> Result<Vec<MetaSourceMapEntry>, DeriveExecutionError> {
    let Some(declaration) = snapshot
        .declarations()
        .iter()
        .find(|declaration| declaration.module() == module && declaration.identity() == target)
    else {
        return Ok(Vec::new());
    };
    let generated_end =
        u32::try_from(generated_len).map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
    let origin = declaration
        .origin()
        .source_span()
        .ok_or(DeriveExecutionError::InvalidProviderBody)?;
    Ok(vec![MetaSourceMapEntry::new(0, generated_end, origin)?])
}

fn derive_target_type(target: &crate::meta::DeriveTarget) -> String {
    if target.generic_parameters().is_empty() {
        target.identity().to_owned()
    } else {
        format!(
            "{}[{}]",
            target.identity(),
            target.generic_parameters().join(", ")
        )
    }
}

fn derive_generic_header(
    snapshot: &MetaSnapshot,
    derive: &ValidatedDerive,
    validated_trait: Option<&crate::meta::ValidatedTrait>,
) -> String {
    let parameters = derive.target().generic_parameters();
    if parameters.is_empty() {
        return String::new();
    }
    let declaration = snapshot.declarations().iter().find(|declaration| {
        declaration.module() == derive.target().module()
            && declaration.identity() == derive.target().identity()
    });
    let binders = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            let mut bounds = declaration
                .and_then(|declaration| {
                    declaration
                        .generic_parameters()
                        .iter()
                        .find(|candidate| candidate.name() == parameter)
                })
                .map(|parameter| parameter.source_bounds().to_vec())
                .unwrap_or_default();
            bounds.extend(
                derive
                    .generic_bounds()
                    .get(index)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            if let Some(validated_trait) = validated_trait
                && validated_trait
                    .introduced_bounds()
                    .iter()
                    .any(|bound| bound == parameter)
            {
                bounds.push(validated_trait.identity().to_owned());
                if derive_requires_partial_value_cleanup(validated_trait.identity()) {
                    bounds.push("Discard".into());
                }
            }
            bounds.sort();
            bounds.dedup();
            if bounds.is_empty() {
                parameter.clone()
            } else {
                format!("{parameter}: {}", bounds.join(" + "))
            }
        })
        .collect::<Vec<_>>();
    format!("[{}]", binders.join(", "))
}

fn derive_requires_partial_value_cleanup(trait_identity: &str) -> bool {
    matches!(
        trait_identity
            .split_once('[')
            .map_or(trait_identity, |(base, _)| base),
        "serialization.Encode" | "serialization.Decode"
    )
}

fn output_path(request_index: usize, trait_index: usize) -> String {
    format!("generated/derive/{request_index:08}-{trait_index:04}.to")
}

fn format_impl(
    bytes: &[u8],
    expected_header: Option<&str>,
    allowed_imports: &[String],
) -> Result<MappedFormattedSource, DeriveExecutionError> {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("generated:derive").expect("the generated source identity is valid"),
            ModulePath::new("generated").expect("the generated module is valid"),
            LogicalPath::new("generated/derive.to").expect("the generated path is valid"),
            bytes,
        ))
        .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
    let lexed = lex_with_limits(&sources, file, LexMode::Module, LexLimits::DEFAULT)
        .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
    if !lexed.diagnostics().is_empty() {
        return Err(DeriveExecutionError::InvalidProviderBody);
    }
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
    if !parsed.diagnostics().is_empty() {
        return Err(DeriveExecutionError::InvalidProviderBody);
    }
    let declarations = parsed.cst().root_node().child_nodes().collect::<Vec<_>>();
    let Some(_implementation) = declarations
        .last()
        .filter(|node| node.kind() == SyntaxKind::ImplDecl)
    else {
        return Err(DeriveExecutionError::InvalidProviderBody);
    };
    let normalize = |text: &str| {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    let allowed = allowed_imports
        .iter()
        .map(|text| normalize(text))
        .collect::<std::collections::BTreeSet<_>>();
    let mut seen = std::collections::BTreeSet::new();
    for node in &declarations[..declarations.len() - 1] {
        if node.kind() != SyntaxKind::ImportDecl {
            return Err(DeriveExecutionError::InvalidProviderBody);
        }
        let mut import = String::new();
        for token in node
            .descendant_tokens()
            .filter(|token| !token.kind().is_trivia() && token.kind() != TokenKind::Nl)
        {
            let range = token.range();
            let text = std::str::from_utf8(&bytes[range.start() as usize..range.end() as usize])
                .map_err(|_| DeriveExecutionError::InvalidProviderBody)?;
            import.push_str(token.token().normalized_identifier().unwrap_or(text));
        }
        if !allowed.contains(&import) || !seen.insert(import) {
            return Err(DeriveExecutionError::InvalidProviderBody);
        }
    }
    if let Some(expected) = expected_header {
        crate::meta_derive_bounds::introduced(bytes, expected)?;
    }
    format_parsed_with_mappings(&sources, file, &parsed)
        .map_err(|_| DeriveExecutionError::InvalidProviderBody)
}

#[derive(Debug)]
pub enum DeriveExecutionError {
    InvalidProviderIdentity(String),
    DuplicateProvider(String),
    MissingProvider(String),
    DuplicatePlanEntry {
        request: usize,
        trait_identity: String,
    },
    ProviderFailed {
        provider: String,
        message: String,
    },
    ProviderPanicked {
        provider: String,
    },
    ProviderVm {
        provider: String,
        source: MetaVmError,
    },
    InvalidProviderResult(String),
    InvalidProviderBody,
    Contract(MetaContractError),
}

impl fmt::Display for DeriveExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProviderIdentity(provider) => {
                write!(formatter, "invalid derive provider `{provider}`")
            }
            Self::DuplicateProvider(provider) => {
                write!(formatter, "duplicate derive provider `{provider}`")
            }
            Self::MissingProvider(provider) => {
                write!(formatter, "missing derive provider `{provider}`")
            }
            Self::DuplicatePlanEntry {
                request,
                trait_identity,
            } => write!(
                formatter,
                "duplicate derive plan entry {request} for `{trait_identity}`"
            ),
            Self::ProviderFailed { provider, message } => {
                write!(formatter, "derive provider `{provider}` failed: {message}")
            }
            Self::ProviderPanicked { provider } => {
                write!(formatter, "derive provider `{provider}` panicked")
            }
            Self::ProviderVm { provider, source } => {
                write!(
                    formatter,
                    "derive provider `{provider}` VM failed: {source}"
                )
            }
            Self::InvalidProviderResult(provider) => write!(
                formatter,
                "derive provider `{provider}` did not return its declared result"
            ),
            Self::InvalidProviderBody => {
                formatter.write_str("derive provider returned an invalid or unauthorized impl")
            }
            Self::Contract(error) => error.fmt(formatter),
        }
    }
}

impl Error for DeriveExecutionError {}

impl From<MetaContractError> for DeriveExecutionError {
    fn from(error: MetaContractError) -> Self {
        Self::Contract(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{
        DeriveContext, DeriveProvider, DeriveRequest, DeriveTarget, DeriveTargetKind,
        validate_derive_requests,
    };
    use crate::meta_test_support::string_artifact;

    struct Body(&'static [u8]);

    impl DeriveProviderCompiler for Body {
        fn compile(&self, request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
            assert_eq!(request.target(), "User");
            assert_eq!(request.module(), "app");
            assert_eq!(request.trait_identity(), "Display");
            assert_eq!(request.provider_identity(), "std.derive.Display");
            assert_eq!(request.introduced_bounds(), &["T"]);
            assert_eq!(request.snapshot().format(), crate::meta::META_MODEL);
            Ok(string_artifact(std::str::from_utf8(self.0).unwrap()))
        }
    }

    struct Failure;

    impl DeriveProviderCompiler for Failure {
        fn compile(&self, _request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
            Err("rejected".into())
        }
    }

    struct Panic;

    impl DeriveProviderCompiler for Panic {
        fn compile(&self, _request: DeriveProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
            panic!("hostile derive provider")
        }
    }

    fn plan() -> Vec<ValidatedDerive> {
        let mut context = DeriveContext::new("app");
        context.add_target(DeriveTarget::new(
            "User",
            "app",
            ["T"],
            DeriveTargetKind::Record,
        ));
        context.add_trait("Display");
        context.add_provider(DeriveProvider::new("Display", "std.derive.Display", ["T"]));
        validate_derive_requests(
            &[DeriveRequest::new("app", "User", ["T"], ["Display"])],
            &context,
        )
        .unwrap()
    }

    #[test]
    fn providers_return_only_validated_formatted_impl_bodies() {
        let mut registry = DeriveProviderRegistry::default();
        registry
            .insert(
                "std.derive.Display",
                Arc::new(Body(b"{\nfn display(self): String { \"User\" }\n}\n")),
            )
            .unwrap();
        let execution = execute_derive_plan(
            &plan(),
            MetaSnapshot::new(crate::meta::MetaEnvironment::meta(), [], [], []).unwrap(),
            MetaLimits::new(10_000, 1024, 1024).unwrap(),
            &registry,
        )
        .unwrap();
        assert_eq!(execution.providers(), &["std.derive.Display"]);
        let output = &execution.response().outputs()[0];
        assert_eq!(output.path(), "generated/derive/00000000-0000.to");
        assert_eq!(output.module(), "app");
        assert_eq!(
            std::str::from_utf8(output.bytes()).unwrap(),
            "impl [T: Display]Display for User[T] {\n    fn display(self): String {\n        \"User\"\n    }\n}\n"
        );
        assert_eq!(execution.into_response().outputs().len(), 1);
    }

    #[test]
    fn failure_invalid_body_and_registry_drift_publish_nothing() {
        let snapshot =
            || MetaSnapshot::new(crate::meta::MetaEnvironment::meta(), [], [], []).unwrap();
        let limits = MetaLimits::new(10_000, 1024, 1024).unwrap();
        let empty = DeriveProviderRegistry::default();
        assert!(matches!(
            execute_derive_plan(&plan(), snapshot(), limits, &empty),
            Err(DeriveExecutionError::MissingProvider(_))
        ));

        let mut failed = DeriveProviderRegistry::default();
        failed
            .insert("std.derive.Display", Arc::new(Failure))
            .unwrap();
        assert!(matches!(
            execute_derive_plan(&plan(), snapshot(), limits, &failed),
            Err(DeriveExecutionError::ProviderFailed { .. })
        ));

        let mut panicked = DeriveProviderRegistry::default();
        panicked
            .insert("std.derive.Display", Arc::new(Panic))
            .unwrap();
        assert!(matches!(
            execute_derive_plan(&plan(), snapshot(), limits, &panicked),
            Err(DeriveExecutionError::ProviderPanicked { .. })
        ));

        let mut invalid = DeriveProviderRegistry::default();
        invalid
            .insert("std.derive.Display", Arc::new(Body(b"{ fn broken( }")))
            .unwrap();
        assert!(matches!(
            execute_derive_plan(&plan(), snapshot(), limits, &invalid),
            Err(DeriveExecutionError::InvalidProviderBody)
        ));

        let mut escaped = DeriveProviderRegistry::default();
        escaped
            .insert(
                "std.derive.Display",
                Arc::new(Body(b"{}\nfn leaked(): Unit {}\n")),
            )
            .unwrap();
        assert!(matches!(
            execute_derive_plan(&plan(), snapshot(), limits, &escaped),
            Err(DeriveExecutionError::InvalidProviderBody)
        ));
        assert!(invalid.insert("", Arc::new(Failure)).is_err());
        assert!(
            invalid
                .insert("std.derive.Display", Arc::new(Failure))
                .is_err()
        );
    }
}
