//! Stable diagnostics for compile-time providers and generators.
//!
//! The execution layers return typed errors. This module is the single boundary
//! that assigns the normative `E2101`-`E2109` identities, source locations and
//! related provider input locations before ordinary diagnostic rendering.

use crate::diagnostics::{
    Diagnostic, DiagnosticBag, DiagnosticCode, DiagnosticError, DiagnosticReport, PrimaryLocation,
    Related, Severity,
};
use crate::meta::{MetaDiagnostic, MetaDiagnosticCode};
use crate::meta_derive::DeriveExecutionError;
use crate::meta_generate::GeneratorExecutionError;
use crate::meta_vm::MetaVmError;
use crate::source::{SourceDatabase, SourceId, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaDiagnosticEntry {
    code: MetaDiagnosticCode,
    message: String,
    primary: Option<Span>,
    related: Vec<(String, Span)>,
}

impl MetaDiagnosticEntry {
    pub fn new(
        code: MetaDiagnosticCode,
        message: impl Into<String>,
        primary: Option<Span>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            primary,
            related: Vec::new(),
        }
    }

    pub fn with_related(mut self, message: impl Into<String>, span: Span) -> Self {
        self.related.push((message.into(), span));
        self
    }

    pub fn code(&self) -> MetaDiagnosticCode {
        self.code
    }

    pub fn primary(&self) -> Option<Span> {
        self.primary
    }

    pub fn related(&self) -> &[(String, Span)] {
        &self.related
    }

    fn into_diagnostic(self, target: &SourceId) -> Result<Diagnostic, DiagnosticError> {
        let location = self.primary.map_or_else(
            || PrimaryLocation::Target(target.clone()),
            PrimaryLocation::Source,
        );
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(self.code.as_str())?,
            self.message,
            location,
        )?;
        for (message, span) in self.related {
            diagnostic = diagnostic.with_related(Related::new(message, span)?);
        }
        Ok(diagnostic)
    }
}

/// Render through the ordinary diagnostic protocol so JSON IDs, ordering and
/// null source locations have exactly the same contract as compiler errors.
pub fn render_meta_diagnostics(
    entries: impl IntoIterator<Item = MetaDiagnosticEntry>,
    target: SourceId,
    sources: &SourceDatabase,
) -> Result<DiagnosticReport, DiagnosticError> {
    let mut bag = DiagnosticBag::new();
    for entry in entries {
        bag.push(entry.into_diagnostic(&target)?);
    }
    bag.resolve(crate::LANGUAGE_EDITION, sources)
}

pub fn semantic_entry(diagnostic: &MetaDiagnostic) -> MetaDiagnosticEntry {
    let mut entry =
        MetaDiagnosticEntry::new(diagnostic.code(), diagnostic.message(), diagnostic.span());
    if let (Some(identity), Some(span)) = (diagnostic.trait_identity(), diagnostic.span()) {
        entry = entry.with_related(format!("requested trait `{identity}`"), span);
    }
    entry
}

pub fn derive_execution_entry(
    error: &DeriveExecutionError,
    request_span: Option<Span>,
    related: impl IntoIterator<Item = (String, Span)>,
) -> MetaDiagnosticEntry {
    let mut entry = MetaDiagnosticEntry::new(
        derive_execution_code(error),
        error.to_string(),
        request_span,
    );
    for (message, span) in related {
        entry = entry.with_related(message, span);
    }
    entry
}

pub fn generator_execution_entry(error: &GeneratorExecutionError) -> MetaDiagnosticEntry {
    MetaDiagnosticEntry::new(generator_execution_code(error), error.to_string(), None)
}

pub fn dependency_cycle_entry(
    root: impl Into<String>,
    root_span: Span,
    dependency: impl Into<String>,
    dependency_span: Span,
) -> MetaDiagnosticEntry {
    let root = root.into();
    let dependency = dependency.into();
    MetaDiagnosticEntry::new(
        MetaDiagnosticCode::GenerationDependencyCycle,
        format!("meta root `{root}` depends on current-round output `{dependency}`"),
        Some(root_span),
    )
    .with_related("current-round generated declaration", dependency_span)
}

pub fn derive_execution_code(error: &DeriveExecutionError) -> MetaDiagnosticCode {
    match error {
        DeriveExecutionError::MissingProvider(_) => MetaDiagnosticCode::MissingDeriveProvider,
        DeriveExecutionError::DuplicatePlanEntry { .. } => MetaDiagnosticCode::InvalidDeriveRequest,
        DeriveExecutionError::ProviderFailed { .. } => MetaDiagnosticCode::DeriveExpansionFailed,
        DeriveExecutionError::ProviderVm { source, .. } => vm_code(source),
        DeriveExecutionError::InvalidProviderResult(_)
        | DeriveExecutionError::InvalidProviderBody
        | DeriveExecutionError::Contract(_)
        | DeriveExecutionError::InvalidProviderIdentity(_)
        | DeriveExecutionError::DuplicateProvider(_) => MetaDiagnosticCode::InvalidGeneratedSource,
    }
}

pub fn generator_execution_code(error: &GeneratorExecutionError) -> MetaDiagnosticCode {
    match error {
        GeneratorExecutionError::ProviderVm { source, .. } => vm_code(source),
        GeneratorExecutionError::InvalidGeneratedSource
        | GeneratorExecutionError::InvalidProviderResult(_)
        | GeneratorExecutionError::ProviderFailed { .. } => {
            MetaDiagnosticCode::InvalidGeneratedSource
        }
        GeneratorExecutionError::SnapshotRoots(_) => MetaDiagnosticCode::GenerationDependencyCycle,
        GeneratorExecutionError::InvalidPlan(_)
        | GeneratorExecutionError::InputSet
        | GeneratorExecutionError::InputHash(_)
        | GeneratorExecutionError::UnknownInput { .. }
        | GeneratorExecutionError::SnapshotSet
        | GeneratorExecutionError::OutputCollision(_)
        | GeneratorExecutionError::DuplicateProvider(_)
        | GeneratorExecutionError::MissingProvider(_)
        | GeneratorExecutionError::Model(_)
        | GeneratorExecutionError::Contract(_) => MetaDiagnosticCode::GeneratorContractViolation,
    }
}

fn vm_code(error: &MetaVmError) -> MetaDiagnosticCode {
    match error {
        MetaVmError::OutputLimit { .. } => MetaDiagnosticCode::GeneratorResourceLimit,
        MetaVmError::Vm(error) if error.is_resource_limit() => {
            MetaDiagnosticCode::GeneratorResourceLimit
        }
        MetaVmError::Capability(_)
        | MetaVmError::ForbiddenType(_)
        | MetaVmError::ForbiddenOperation(_) => MetaDiagnosticCode::MetaCapabilityDenied,
        MetaVmError::Vm(tondo_vm::runtime::VmError::UnsupportedHostCall(_)) => {
            MetaDiagnosticCode::MetaCapabilityDenied
        }
        MetaVmError::WrongTarget { .. }
        | MetaVmError::InvalidLimit(_)
        | MetaVmError::UnknownEntry(_)
        | MetaVmError::HostValue
        | MetaVmError::OutputSizeOverflow
        | MetaVmError::StructuredOutput(_)
        | MetaVmError::Vm(_) => MetaDiagnosticCode::InvalidGeneratedSource,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use tondo_vm::runtime::VmError;

    use super::*;
    use crate::source::{LogicalPath, ModulePath, SourceInput, SourceOrigin, TextRange};

    fn sources() -> (SourceDatabase, Span, Span) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::new(
                SourceId::new("src:app").unwrap(),
                ModulePath::new("app").unwrap(),
                LogicalPath::new("src/app.to").unwrap(),
                SourceOrigin::Physical,
                b"derive Display\nrecord User { name: String }\n".to_vec(),
            ))
            .unwrap();
        let derive = sources.span(file, TextRange::new(0, 14).unwrap()).unwrap();
        let field = sources.span(file, TextRange::new(29, 41).unwrap()).unwrap();
        (sources, derive, field)
    }

    #[test]
    fn every_meta_code_has_the_normative_identity_and_stable_name() {
        let cases = [
            (
                MetaDiagnosticCode::InvalidDeriveTarget,
                "E2101",
                "invalid-derive-target",
            ),
            (
                MetaDiagnosticCode::MissingDeriveProvider,
                "E2102",
                "missing-derive-provider",
            ),
            (
                MetaDiagnosticCode::InvalidDeriveRequest,
                "E2103",
                "invalid-derive-request",
            ),
            (
                MetaDiagnosticCode::DeriveExpansionFailed,
                "E2104",
                "derive-expansion-failed",
            ),
            (
                MetaDiagnosticCode::InvalidGeneratedSource,
                "E2105",
                "invalid-generated-source",
            ),
            (
                MetaDiagnosticCode::GeneratorContractViolation,
                "E2106",
                "generator-contract-violation",
            ),
            (
                MetaDiagnosticCode::GeneratorResourceLimit,
                "E2107",
                "generator-resource-limit",
            ),
            (
                MetaDiagnosticCode::MetaCapabilityDenied,
                "E2108",
                "meta-capability-denied",
            ),
            (
                MetaDiagnosticCode::GenerationDependencyCycle,
                "E2109",
                "generation-dependency-cycle",
            ),
        ];
        for (code, identity, name) in cases {
            assert_eq!(code.as_str(), identity);
            assert_eq!(code.stable_name(), name);
        }
    }

    #[test]
    fn source_and_target_locations_have_stable_json_shapes() {
        let (sources, derive, _) = sources();
        let target = SourceId::new("target:meta").unwrap();
        let entries = [
            MetaDiagnosticEntry::new(
                MetaDiagnosticCode::DeriveExpansionFailed,
                "rejected",
                Some(derive),
            ),
            MetaDiagnosticEntry::new(
                MetaDiagnosticCode::GeneratorContractViolation,
                "missing output",
                None,
            ),
        ];
        let first = render_meta_diagnostics(entries.clone(), target.clone(), &sources)
            .unwrap()
            .json_lines()
            .unwrap();
        let second = render_meta_diagnostics(entries, target, &sources)
            .unwrap()
            .json_lines()
            .unwrap();
        assert_eq!(first, second);
        let values = first
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            values
                .iter()
                .any(|value| value["code"] == "E2104" && !value["range"].is_null())
        );
        assert!(
            values
                .iter()
                .any(|value| value["code"] == "E2106" && value["range"].is_null())
        );
    }

    #[test]
    fn provider_diagnostics_retain_field_locations() {
        let (sources, derive, field) = sources();
        let entry = derive_execution_entry(
            &DeriveExecutionError::ProviderFailed {
                provider: "std.derive.Display".into(),
                message: "field cannot be displayed".into(),
            },
            Some(derive),
            [("unsupported field `name`".into(), field)],
        );
        let json =
            render_meta_diagnostics([entry], SourceId::new("target:meta").unwrap(), &sources)
                .unwrap()
                .json_lines()
                .unwrap();
        let value: Value = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(value["code"], "E2104");
        assert_eq!(value["related"][0]["message"], "unsupported field `name`");
        assert_eq!(value["related"][0]["range"]["start"]["byte"], 29);
    }

    #[test]
    fn specific_vm_failures_take_precedence_over_invalid_output() {
        let limit = DeriveExecutionError::ProviderVm {
            provider: "p".into(),
            source: MetaVmError::Vm(VmError::ResourceLimit {
                resource: "steps",
                limit: 1,
            }),
        };
        let capability = GeneratorExecutionError::ProviderVm {
            generator: "g".into(),
            source: MetaVmError::Vm(VmError::UnsupportedHostCall("clock".into())),
        };
        assert_eq!(
            derive_execution_code(&limit),
            MetaDiagnosticCode::GeneratorResourceLimit
        );
        assert_eq!(
            generator_execution_code(&capability),
            MetaDiagnosticCode::MetaCapabilityDenied
        );
        assert_eq!(
            generator_execution_code(&GeneratorExecutionError::InvalidGeneratedSource),
            MetaDiagnosticCode::InvalidGeneratedSource
        );
    }

    #[test]
    fn dependency_cycles_point_to_both_ends_of_the_edge() {
        let (sources, root, generated) = sources();
        let entry = dependency_cycle_entry("app.Schema", root, "generated.Codec", generated);
        assert_eq!(entry.code(), MetaDiagnosticCode::GenerationDependencyCycle);
        assert_eq!(entry.primary(), Some(root));
        assert_eq!(entry.related().len(), 1);
        let report =
            render_meta_diagnostics([entry], SourceId::new("target:meta").unwrap(), &sources)
                .unwrap();
        assert!(report.json_lines().unwrap().contains("E2109"));
    }

    #[test]
    fn invalid_messages_fail_before_unstable_json_can_escape() {
        let sources = SourceDatabase::new();
        let result = render_meta_diagnostics(
            [MetaDiagnosticEntry::new(
                MetaDiagnosticCode::GeneratorContractViolation,
                "line one\nline two",
                None,
            )],
            SourceId::new("target:meta").unwrap(),
            &sources,
        );
        assert!(matches!(
            result,
            Err(DiagnosticError::MessageContainsLineFeed)
        ));
    }
}
