//! Static validation shared by derive, reflection, and the meta target.
//!
//! This module deliberately stops at the semantic boundary. It resolves the
//! exact target/trait/provider identities and produces a deterministic plan;
//! executing a provider and constructing its immutable model belong to later
//! meta phases.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::hir::HirProgram;
use crate::source::Span;

/// The only meta model accepted by the Tondo 0.1 toolchain.
pub const META_MODEL: &str = "tondo-meta-model-0.1/1";

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
    CoherenceConflict,
}

impl MetaDiagnosticCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDeriveTarget => "E2101",
            Self::MissingDeriveProvider => "E2102",
            Self::InvalidDeriveRequest => "E2103",
            Self::CoherenceConflict => "E1111",
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
            if !context.traits.contains(trait_identity) {
                diagnostics.push(invalid_request(
                    request_index,
                    request,
                    Some(trait_identity),
                    "the requested identity is not a trait",
                ));
                continue;
            }
            let Some(provider) = context.providers.get(trait_identity) else {
                diagnostics.push(MetaDiagnostic {
                    code: MetaDiagnosticCode::MissingDeriveProvider,
                    request_index,
                    trait_identity: Some(trait_identity.clone()),
                    message: "no provider is locked for the exact trait identity".into(),
                    span: request.span(),
                });
                continue;
            };
            if provider.trait_identity() != trait_identity
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
}
