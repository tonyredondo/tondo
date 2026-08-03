use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::artifact::{
    ArtifactError, BuildArtifact, BuildProducts, CompiledInterface, DeclaredBuildInputs,
    build_products, validate_dependency_interfaces,
};
use crate::bytecode::{BytecodeError, BytecodeLoweringLimits, lower_to_bytecode};
use crate::diagnostics::{
    Diagnostic, DiagnosticBag, DiagnosticCode, DiagnosticError, DiagnosticReport, PrimaryLocation,
    Related, Severity,
};
use crate::hir::{
    ExpressionCheckLimits, HirCallableId, HirDiscardStatus, HirError, HirProgram,
    TypeLoweringLimits, check_expressions_configured, lower_types,
};
use crate::mir::{MirError, MirLoweringLimits, lower_to_mir};
pub use crate::package::Edition;
use crate::package::{PackageGraph, PackageGraphError};
use crate::process_host::BootstrapHost;
use crate::resolve::{
    ResolveError, ResolvedProgram, SymbolKind, Visibility, is_script_statement, resolve,
};
use crate::semantic::SemanticModel;
use crate::source::{FileId, SourceDatabase, SourceError, SourceId, Span, TextRange};
use crate::syntax::{
    FormatError, LexError, LexLimits, LexMode, ParseError, ParseLimits, ParseMode, Parsed,
    format_parsed, lex_with_limits, parse,
};
use crate::test_backend;
use crate::types::TypeError;
use crate::types::{ScalarType, TypeKind};
use tondo_vm::bytecode::BytecodeSpan;
use tondo_vm::runtime::{RuntimeValue, VmError, VmLimits, VmOutcome, VmPanic, execute_with_limits};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Format,
    Check,
    Run,
    Test,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Format => "fmt",
            Self::Check => "check",
            Self::Run => "run",
            Self::Test => "test",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostProfile {
    Hosted,
    Meta,
}

impl HostProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Meta => "meta",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceForm {
    Module,
    Script,
    Fragment,
}

impl SourceForm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Script => "script",
            Self::Fragment => "fragment",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFormat {
    Human,
    Json,
}

/// Closed warning profiles selected by an invocation.
///
/// Profiles add diagnostics only; they never relax language errors or change
/// runtime semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WarningProfile {
    Core,
}

impl WarningProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapabilityName(String);

impl CapabilityName {
    pub fn new(value: impl Into<String>) -> Result<Self, DriverError> {
        let value = value.into();
        if !matches!(
            value.as_str(),
            "process"
                | "threads"
                | "filesystem"
                | "network"
                | "console"
                | "environment"
                | "clock"
                | "entropy"
                | "dynamic-linking"
        ) {
            return Err(DriverError::InvalidCapability(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTarget {
    name: String,
    diagnostic_source_id: SourceId,
    profiles: BTreeSet<HostProfile>,
    supported_capabilities: BTreeSet<CapabilityName>,
}

impl BuildTarget {
    pub fn vm_hosted() -> Self {
        let supported_capabilities = Self::vm_hosted_capabilities();
        Self {
            name: "tondo-vm-hosted".into(),
            diagnostic_source_id: SourceId::new("target:tondo-vm-hosted")
                .expect("the built-in target source ID is valid"),
            profiles: BTreeSet::from([HostProfile::Hosted]),
            supported_capabilities,
        }
    }

    /// Hermetic target used exclusively by compile-time Tondo programs.
    pub fn tondo_meta() -> Self {
        Self {
            name: "tondo-meta".into(),
            diagnostic_source_id: SourceId::new("target:tondo-meta")
                .expect("the built-in meta target source ID is valid"),
            profiles: BTreeSet::from([HostProfile::Meta]),
            supported_capabilities: BTreeSet::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn diagnostic_source_id(&self) -> &SourceId {
        &self.diagnostic_source_id
    }

    pub fn supports_profile(&self, profile: HostProfile) -> bool {
        self.profiles.contains(&profile)
    }

    pub fn supported_capabilities(&self) -> &BTreeSet<CapabilityName> {
        &self.supported_capabilities
    }

    pub fn vm_hosted_capabilities() -> BTreeSet<CapabilityName> {
        BTreeSet::from([
            CapabilityName::new("console")
                .expect("console is a registered Tondo target capability"),
            CapabilityName::new("process")
                .expect("process is a registered Tondo target capability"),
            CapabilityName::new("clock").expect("clock is a registered Tondo target capability"),
            CapabilityName::new("environment")
                .expect("environment is a registered Tondo target capability"),
            CapabilityName::new("filesystem")
                .expect("filesystem is a registered Tondo target capability"),
        ])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    pub max_source_bytes: u32,
    pub max_files: u32,
    pub max_syntax_tokens: u32,
    pub max_syntax_nodes: u32,
    pub max_syntax_depth: u32,
    pub max_type_nodes: u32,
    pub max_hir_nodes: u32,
    pub max_pattern_analysis_steps: u32,
    pub max_mir_functions: u32,
    pub max_mir_blocks_per_function: u32,
    pub max_mir_locals_per_function: u32,
    pub max_mir_statements_per_function: u32,
    pub max_mir_verification_steps: u64,
    pub max_bytecode_types: u32,
    pub max_bytecode_nominals: u32,
    pub max_bytecode_callables: u32,
    pub max_bytecode_constants: u32,
    pub max_bytecode_functions: u32,
    pub max_bytecode_slots_per_function: u32,
    pub max_bytecode_blocks_per_function: u32,
    pub max_bytecode_instructions_per_function: u32,
    pub max_bytecode_spans_per_function: u32,
    pub max_bytecode_verification_steps: u64,
    pub max_vm_steps: u64,
    pub max_vm_stack_depth: u32,
    pub max_vm_heap_objects: u32,
    pub max_vm_heap_bytes: u64,
    pub initial_vm_gc_threshold: u32,
    pub max_generic_instantiations: u32,
    pub max_trait_obligations: u32,
    pub max_diagnostics: u32,
    pub max_diagnostic_json_bytes: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 64 * 1024 * 1024,
            max_files: 65_536,
            max_syntax_tokens: 2_000_000,
            max_syntax_nodes: 4_000_000,
            max_syntax_depth: 256,
            max_type_nodes: 4_000_000,
            max_hir_nodes: 4_000_000,
            max_pattern_analysis_steps: 4_000_000,
            max_mir_functions: 100_000,
            max_mir_blocks_per_function: 1_000_000,
            max_mir_locals_per_function: 1_000_000,
            max_mir_statements_per_function: 4_000_000,
            max_mir_verification_steps: 32_000_000,
            max_bytecode_types: 4_000_000,
            max_bytecode_nominals: 1_000_000,
            max_bytecode_callables: 1_000_000,
            max_bytecode_constants: 1_000_000,
            max_bytecode_functions: 100_000,
            max_bytecode_slots_per_function: 1_000_000,
            max_bytecode_blocks_per_function: 1_000_000,
            max_bytecode_instructions_per_function: 4_000_000,
            max_bytecode_spans_per_function: 4_000_000,
            max_bytecode_verification_steps: 32_000_000,
            max_vm_steps: 100_000_000,
            max_vm_stack_depth: 65_536,
            max_vm_heap_objects: 1_000_000,
            max_vm_heap_bytes: 1024 * 1024 * 1024,
            initial_vm_gc_threshold: 1024,
            max_generic_instantiations: 1_000_000,
            max_trait_obligations: 1_000_000,
            max_diagnostics: 10_000,
            max_diagnostic_json_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct CompilationRequest {
    operation: Operation,
    edition: Edition,
    target: BuildTarget,
    profile: HostProfile,
    capabilities: BTreeSet<CapabilityName>,
    diagnostic_format: DiagnosticFormat,
    source_form: SourceForm,
    limits: ResourceLimits,
    packages: PackageGraph,
    sources: SourceDatabase,
    root: FileId,
    program_arguments: Vec<String>,
    build_inputs: DeclaredBuildInputs,
    documentation_fixture: bool,
    warning_profiles: BTreeSet<WarningProfile>,
    test_entry: Option<String>,
    test_envelope: Option<crate::test_control::EnvelopeHandle>,
    test_participation_entries: Vec<String>,
    test_participation: Option<crate::test_backend::TestParticipation>,
}

impl CompilationRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        operation: Operation,
        edition: Edition,
        target: BuildTarget,
        profile: HostProfile,
        capabilities: BTreeSet<CapabilityName>,
        diagnostic_format: DiagnosticFormat,
        source_form: SourceForm,
        limits: ResourceLimits,
        mut packages: PackageGraph,
        sources: SourceDatabase,
        root: FileId,
    ) -> Result<Self, DriverError> {
        sources.get(root)?;
        if !target.supports_profile(profile) {
            return Err(DriverError::UnsupportedTargetProfile {
                target: target.name().to_owned(),
                profile: profile.as_str(),
            });
        }
        if let Some(capability) = capabilities
            .iter()
            .find(|capability| !target.supported_capabilities().contains(*capability))
        {
            return Err(DriverError::UnsupportedTargetCapability {
                target: target.name().to_owned(),
                capability: capability.as_str().to_owned(),
            });
        }
        packages.select_bootstrap_standard_modules(|required| {
            capabilities
                .iter()
                .any(|capability| capability.as_str() == required)
        });
        packages.validate_sources(&sources, root)?;
        Ok(Self {
            operation,
            edition,
            target,
            profile,
            capabilities,
            diagnostic_format,
            source_form,
            limits,
            packages,
            sources,
            root,
            program_arguments: Vec::new(),
            build_inputs: DeclaredBuildInputs::default(),
            documentation_fixture: false,
            warning_profiles: BTreeSet::new(),
            test_entry: None,
            test_envelope: None,
            test_participation_entries: Vec::new(),
            test_participation: None,
        })
    }

    /// Enables the isolated Appendix C interfaces for the conformance doc
    /// runner. This surface is absent from ordinary compiler builds.
    #[cfg(feature = "conformance")]
    pub fn with_documentation_fixture(mut self) -> Self {
        self.documentation_fixture = true;
        self
    }

    /// Supplies the values exposed by `std.process.args()` during `run`.
    pub fn with_program_arguments(mut self, arguments: Vec<String>) -> Self {
        self.program_arguments = arguments;
        self
    }

    pub fn with_declared_build_inputs(mut self, inputs: DeclaredBuildInputs) -> Self {
        self.build_inputs = inputs;
        self
    }

    pub fn with_warning_profiles(
        mut self,
        profiles: impl IntoIterator<Item = WarningProfile>,
    ) -> Self {
        self.warning_profiles = profiles.into_iter().collect();
        self
    }

    /// Selects the visible ID (or unique leaf name) of the test entry lowered
    /// by [`Operation::Test`].
    pub fn with_test_entry(mut self, entry: impl Into<String>) -> Self {
        self.test_entry = Some(entry.into());
        self
    }

    /// Installs the private evidence envelope used by `std.testing` host calls.
    /// Ordinary compilation requests never carry this handle.
    pub fn with_test_envelope(mut self, envelope: crate::test_control::EnvelopeHandle) -> Self {
        self.test_envelope = Some(envelope);
        self
    }

    pub fn with_test_participation(
        mut self,
        entries: impl IntoIterator<Item = String>,
        participation: crate::test_backend::TestParticipation,
    ) -> Self {
        self.test_participation_entries = entries.into_iter().collect();
        self.test_participation = Some(participation);
        self
    }

    pub fn operation(&self) -> Operation {
        self.operation
    }

    pub fn edition(&self) -> Edition {
        self.edition
    }

    pub fn target(&self) -> &BuildTarget {
        &self.target
    }

    pub fn profile(&self) -> HostProfile {
        self.profile
    }

    pub fn capabilities(&self) -> &BTreeSet<CapabilityName> {
        &self.capabilities
    }

    pub fn diagnostic_format(&self) -> DiagnosticFormat {
        self.diagnostic_format
    }

    pub fn source_form(&self) -> SourceForm {
        self.source_form
    }

    pub fn limits(&self) -> ResourceLimits {
        self.limits
    }

    pub fn packages(&self) -> &PackageGraph {
        &self.packages
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub fn root(&self) -> FileId {
        self.root
    }

    pub fn program_arguments(&self) -> &[String] {
        &self.program_arguments
    }

    pub fn build_inputs(&self) -> &DeclaredBuildInputs {
        &self.build_inputs
    }

    pub fn warning_profiles(&self) -> &BTreeSet<WarningProfile> {
        &self.warning_profiles
    }

    pub fn test_entry(&self) -> Option<&str> {
        self.test_entry.as_deref()
    }

    /// Creates an isolated test request rooted at the source file containing
    /// `entry`. Source bytes and package identity are copied, never shared
    /// mutably, so retries can compile and execute a fresh VM root.
    pub fn for_test_entry(&self, entry: &test_backend::TestEntry) -> Result<Self, DriverError> {
        let root = entry.file();
        let sources = clone_source_database(&self.sources, None)?;
        let mut packages = self.packages.clone();
        packages.enable_bootstrap_testing()?;
        packages.validate_sources(&sources, root)?;
        let request = CompilationRequest::new(
            Operation::Test,
            self.edition,
            self.target.clone(),
            self.profile,
            self.capabilities.clone(),
            self.diagnostic_format,
            SourceForm::Module,
            self.limits,
            packages,
            sources,
            root,
        )?
        .with_program_arguments(self.program_arguments.clone())
        .with_declared_build_inputs(self.build_inputs.clone())
        .with_warning_profiles(self.warning_profiles.clone())
        .with_test_entry(entry.id().to_owned());
        Ok(request)
    }

    /// Creates one test request for every selected leaf in the same source
    /// file, preserving suite scopes inside a single VM root.
    pub fn for_test_participation(
        &self,
        entries: &[test_backend::TestEntry],
        participation: test_backend::TestParticipation,
    ) -> Result<Self, DriverError> {
        let first = entries
            .first()
            .ok_or_else(|| DriverError::Invariant("test participation cannot be empty".into()))?;
        if entries.iter().any(|entry| entry.file() != first.file()) {
            return Err(DriverError::Invariant(
                "test participation crosses source files".into(),
            ));
        }
        let root = first.file();
        let sources = clone_source_database(&self.sources, None)?;
        let mut packages = self.packages.clone();
        packages.enable_bootstrap_testing()?;
        packages.validate_sources(&sources, root)?;
        CompilationRequest::new(
            Operation::Test,
            self.edition,
            self.target.clone(),
            self.profile,
            self.capabilities.clone(),
            self.diagnostic_format,
            SourceForm::Module,
            self.limits,
            packages,
            sources,
            root,
        )
        .map(|request| {
            request
                .with_program_arguments(self.program_arguments.clone())
                .with_declared_build_inputs(self.build_inputs.clone())
                .with_warning_profiles(self.warning_profiles.clone())
                .with_test_participation(
                    entries.iter().map(|entry| entry.id().to_owned()),
                    participation,
                )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilationStatus {
    Success,
    Rejected,
}

#[derive(Debug)]
pub struct CompilationOutput {
    status: CompilationStatus,
    exit_code: u8,
    diagnostics: DiagnosticReport,
    stdout: Vec<u8>,
    semantic_model: Option<SemanticModel>,
    products: Option<BuildProducts>,
}

impl CompilationOutput {
    pub fn status(&self) -> CompilationStatus {
        self.status
    }

    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }

    pub fn diagnostics(&self) -> &DiagnosticReport {
        &self.diagnostics
    }

    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    pub fn semantic_model(&self) -> Option<&SemanticModel> {
        self.semantic_model.as_ref()
    }

    pub fn into_semantic_model(self) -> Option<SemanticModel> {
        self.semantic_model
    }

    pub fn into_stdout(self) -> Vec<u8> {
        self.stdout
    }

    pub fn interface(&self) -> Option<&CompiledInterface> {
        self.products.as_ref().map(BuildProducts::interface)
    }

    pub fn artifact(&self) -> Option<&BuildArtifact> {
        self.products.as_ref().map(BuildProducts::artifact)
    }

    pub fn into_products(self) -> Option<BuildProducts> {
        self.products
    }
}

#[derive(Debug)]
pub enum DriverError {
    InvalidCapability(String),
    UnsupportedTargetProfile {
        target: String,
        profile: &'static str,
    },
    UnsupportedTargetCapability {
        target: String,
        capability: String,
    },
    Artifact(ArtifactError),
    PackageGraph(PackageGraphError),
    Source(SourceError),
    Diagnostic(DiagnosticError),
    Lex(LexError),
    Parse(ParseError),
    Resolve(ResolveError),
    Hir(HirError),
    Mir(MirError),
    Bytecode(BytecodeError),
    Vm(VmError),
    Format(FormatError),
    Invariant(String),
}

impl fmt::Display for DriverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapability(capability) => {
                write!(formatter, "invalid capability name `{capability}`")
            }
            Self::UnsupportedTargetProfile { target, profile } => {
                write!(
                    formatter,
                    "target `{target}` does not support profile `{profile}`"
                )
            }
            Self::UnsupportedTargetCapability { target, capability } => write!(
                formatter,
                "target `{target}` does not provide capability `{capability}`"
            ),
            Self::Artifact(error) => error.fmt(formatter),
            Self::PackageGraph(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::Lex(error) => error.fmt(formatter),
            Self::Parse(error) => error.fmt(formatter),
            Self::Resolve(error) => error.fmt(formatter),
            Self::Hir(error) => error.fmt(formatter),
            Self::Mir(error) => error.fmt(formatter),
            Self::Bytecode(error) => error.fmt(formatter),
            Self::Vm(error) => error.fmt(formatter),
            Self::Format(error) => error.fmt(formatter),
            Self::Invariant(message) => write!(formatter, "driver invariant failed: {message}"),
        }
    }
}

impl Error for DriverError {}

impl From<PackageGraphError> for DriverError {
    fn from(error: PackageGraphError) -> Self {
        Self::PackageGraph(error)
    }
}

impl From<ArtifactError> for DriverError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

impl From<SourceError> for DriverError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<DiagnosticError> for DriverError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

impl From<LexError> for DriverError {
    fn from(error: LexError) -> Self {
        Self::Lex(error)
    }
}

impl From<ParseError> for DriverError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<ResolveError> for DriverError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

impl From<HirError> for DriverError {
    fn from(error: HirError) -> Self {
        Self::Hir(error)
    }
}

impl From<MirError> for DriverError {
    fn from(error: MirError) -> Self {
        Self::Mir(error)
    }
}

impl From<BytecodeError> for DriverError {
    fn from(error: BytecodeError) -> Self {
        Self::Bytecode(error)
    }
}

impl From<VmError> for DriverError {
    fn from(error: VmError) -> Self {
        Self::Vm(error)
    }
}

impl From<FormatError> for DriverError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

/// Executes the single public compilation pipeline.
///
/// Implemented phases run before the terminal bootstrap diagnostic. A source
/// rejected by an implemented phase therefore reports its normative diagnostic
/// instead of also receiving `T0001`.
pub fn execute(request: CompilationRequest) -> Result<CompilationOutput, DriverError> {
    if request.operation == Operation::Test {
        return execute_test(request);
    }
    validate_dependency_interfaces(
        request.edition.as_str(),
        request.target.name(),
        request.profile.as_str(),
        request
            .capabilities
            .iter()
            .map(|capability| capability.as_str().to_owned()),
        &request.build_inputs,
        &request.packages,
    )?;
    if request.source_form == SourceForm::Fragment && request.operation == Operation::Run {
        let mut bag = DiagnosticBag::new();
        bag.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E0006")?,
            "fragment source form cannot be executed",
            PrimaryLocation::Source(request.sources.span(request.root, TextRange::empty(0))?),
        )?);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }
    if let Some(diagnostic) = resource_limit_diagnostic(&request)? {
        let mut bag = DiagnosticBag::new();
        bag.push(diagnostic);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }

    let mut lexical_diagnostics = DiagnosticBag::new();
    let mut lexed_sources = Vec::with_capacity(request.sources.len());
    let mut remaining_tokens = request.limits.max_syntax_tokens as usize;
    let mut remaining_diagnostics = request.limits.max_diagnostics as usize;
    for index in 0..request.sources.len() {
        let file = FileId::from_index(index)?;
        let (lex_mode, parse_mode) = if file == request.root {
            match request.source_form {
                SourceForm::Module => (LexMode::Module, ParseMode::Module),
                SourceForm::Script => (LexMode::Script, ParseMode::Script),
                SourceForm::Fragment => (LexMode::Fragment, ParseMode::Fragment),
            }
        } else {
            (LexMode::ImportedModule, ParseMode::ImportedModule)
        };
        let lexed = match lex_with_limits(
            &request.sources,
            file,
            lex_mode,
            LexLimits {
                max_tokens: remaining_tokens,
                max_diagnostics: remaining_diagnostics,
                max_nesting_depth: request.limits.max_syntax_depth,
            },
        ) {
            Ok(lexed) => lexed,
            Err(LexError::ResourceLimit { resource, offset }) => {
                return syntax_resource_output(&request, file, resource, offset);
            }
            Err(error) => return Err(error.into()),
        };
        remaining_tokens -= lexed.tokens().len();
        remaining_diagnostics -= lexed.diagnostics().len();
        if lexed.diagnostics().is_empty() {
            lexed_sources.push((file, parse_mode, lexed));
        } else {
            lexical_diagnostics.extend(lexed.into_diagnostics());
        }
    }
    if !lexical_diagnostics.is_empty() {
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: lexical_diagnostics.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }

    let mut syntax_diagnostics = DiagnosticBag::new();
    let mut remaining_nodes = request.limits.max_syntax_nodes;
    let mut parsed_sources = Vec::with_capacity(lexed_sources.len());
    for (file, mode, lexed) in lexed_sources {
        let parsed = match parse(
            &request.sources,
            file,
            lexed,
            mode,
            ParseLimits {
                max_nodes: remaining_nodes,
                max_nesting_depth: request.limits.max_syntax_depth,
                max_diagnostics: u32::try_from(remaining_diagnostics)
                    .unwrap_or(request.limits.max_diagnostics),
            },
        ) {
            Ok(parsed) => parsed,
            Err(ParseError::ResourceLimit { resource, offset }) => {
                return syntax_resource_output(&request, file, resource, offset);
            }
            Err(error) => return Err(error.into()),
        };
        remaining_nodes -= u32::try_from(parsed.cst().nodes().len())
            .expect("the parser enforces the u32 syntax-node budget");
        remaining_diagnostics -= parsed.diagnostics().len();
        syntax_diagnostics.extend(parsed.diagnostics().iter().cloned());
        parsed_sources.push((file, parsed));
    }
    if !syntax_diagnostics.is_empty() {
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: syntax_diagnostics.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }

    if request.operation == Operation::Format {
        let parsed = parsed_sources
            .iter()
            .find_map(|(file, parsed)| (*file == request.root).then_some(parsed))
            .expect("the root source is always parsed");
        let stdout = format_parsed(&request.sources, request.root, parsed)?.into_bytes();
        return Ok(CompilationOutput {
            status: CompilationStatus::Success,
            exit_code: 0,
            diagnostics: DiagnosticBag::new()
                .resolve(request.edition.as_str(), &request.sources)?,
            stdout,
            semantic_model: None,
            products: None,
        });
    }

    let resolved = match resolve(
        &request.packages,
        &request.sources,
        parsed_sources.iter().map(|(file, parsed)| (*file, parsed)),
        remaining_diagnostics,
    ) {
        Ok(resolved) => resolved,
        Err(ResolveError::DiagnosticLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "primary diagnostic count", offset);
        }
        Err(error) => return Err(error.into()),
    };
    let (resolved_program, resolution_diagnostics) = resolved.into_parts();
    if !resolution_diagnostics.is_empty() {
        let mut bag = DiagnosticBag::new();
        bag.extend(resolution_diagnostics);
        let diagnostics = bag.resolve(request.edition.as_str(), &request.sources)?;
        drop(parsed_sources);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics,
            stdout: Vec::new(),
            semantic_model: Some(SemanticModel::after_resolution(
                request.sources,
                resolved_program,
            )),
            products: None,
        });
    }

    let hir = match lower_types(
        &request.packages,
        &request.sources,
        parsed_sources.iter().map(|(file, parsed)| (*file, parsed)),
        &resolved_program,
        TypeLoweringLimits {
            max_type_nodes: request.limits.max_type_nodes,
            max_trait_obligations: request.limits.max_trait_obligations,
            max_diagnostics: remaining_diagnostics,
        },
    ) {
        Ok(hir) => hir,
        Err(HirError::DiagnosticLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "primary diagnostic count", offset);
        }
        Err(HirError::Type(TypeError::ResourceLimit { .. })) => {
            return syntax_resource_output(&request, request.root, "interned type node count", 0);
        }
        Err(HirError::TraitObligationLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "trait obligation", offset);
        }
        Err(error) => return Err(error.into()),
    };
    let (hir_program, type_diagnostics) = hir.into_parts();
    if !type_diagnostics.is_empty() {
        let mut bag = DiagnosticBag::new();
        bag.extend(type_diagnostics);
        let diagnostics = bag.resolve(request.edition.as_str(), &request.sources)?;
        drop(parsed_sources);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics,
            stdout: Vec::new(),
            semantic_model: Some(SemanticModel::with_hir(
                request.sources,
                resolved_program,
                hir_program,
            )),
            products: None,
        });
    }

    let checked = match check_expressions_configured(
        &request.sources,
        parsed_sources.iter().map(|(file, parsed)| (*file, parsed)),
        &resolved_program,
        hir_program,
        ExpressionCheckLimits {
            max_nodes: request.limits.max_hir_nodes,
            max_pattern_steps: request.limits.max_pattern_analysis_steps,
            max_trait_obligations: request.limits.max_trait_obligations,
            max_diagnostics: remaining_diagnostics,
        },
        request.documentation_fixture,
    ) {
        Ok(checked) => checked,
        Err(HirError::DiagnosticLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "primary diagnostic count", offset);
        }
        Err(HirError::NodeLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "typed HIR node count", offset);
        }
        Err(HirError::PatternAnalysisLimit { file, offset }) => {
            return syntax_resource_output(
                &request,
                file,
                "pattern exhaustiveness analysis",
                offset,
            );
        }
        Err(HirError::TraitObligationLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "trait obligation", offset);
        }
        Err(HirError::Type(TypeError::ResourceLimit { .. })) => {
            return syntax_resource_output(&request, request.root, "interned type node count", 0);
        }
        Err(error) => return Err(error.into()),
    };
    let (hir_program, mut expression_diagnostics, expression_check_complete) = checked.into_parts();
    let core_warnings = request.warning_profiles.contains(&WarningProfile::Core);
    if !core_warnings {
        expression_diagnostics.retain(|diagnostic| diagnostic.severity() != Severity::Warning);
    }
    if expression_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == Severity::Error)
    {
        let mut bag = DiagnosticBag::new();
        bag.extend(expression_diagnostics);
        let diagnostics = bag.resolve(request.edition.as_str(), &request.sources)?;
        drop(parsed_sources);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics,
            stdout: Vec::new(),
            semantic_model: Some(SemanticModel::with_hir(
                request.sources,
                resolved_program,
                hir_program,
            )),
            products: None,
        });
    }
    if core_warnings {
        let available = remaining_diagnostics.saturating_sub(expression_diagnostics.len());
        match crate::resolve::lint_core(&request.sources, &resolved_program, available) {
            Ok(warnings) => expression_diagnostics.extend(warnings),
            Err(ResolveError::DiagnosticLimit { file, offset }) => {
                return syntax_resource_output(&request, file, "primary diagnostic count", offset);
            }
            Err(error) => return Err(error.into()),
        }
    }

    if request.operation == Operation::Check && expression_check_complete {
        if request.source_form == SourceForm::Script {
            let diagnostic = match select_hosted_main(
                &request,
                &parsed_sources,
                &resolved_program,
                &hir_program,
            )? {
                MainSelection::Rejected(diagnostic) => Some(diagnostic),
                MainSelection::Sync(_) | MainSelection::Async(_) => None,
            };
            let exit_code = u8::from(diagnostic.is_some());
            drop(parsed_sources);
            return semantic_output(
                request,
                resolved_program,
                hir_program,
                expression_diagnostics,
                diagnostic,
                exit_code,
                Vec::new(),
            );
        }
        if request.source_form != SourceForm::Module {
            // Fragment checks deliberately remain outside the hosted program pipeline.
        } else {
            drop(parsed_sources);
            return semantic_output(
                request,
                resolved_program,
                hir_program,
                expression_diagnostics,
                None,
                0,
                Vec::new(),
            );
        }
    }

    if request.operation == Operation::Run {
        match select_hosted_main(&request, &parsed_sources, &resolved_program, &hir_program)? {
            MainSelection::Rejected(diagnostic) => {
                drop(parsed_sources);
                return semantic_output(
                    request,
                    resolved_program,
                    hir_program,
                    expression_diagnostics,
                    Some(diagnostic),
                    1,
                    Vec::new(),
                );
            }
            MainSelection::Sync(_) | MainSelection::Async(_) if !expression_check_complete => {}
            MainSelection::Sync(entry) | MainSelection::Async(entry) => {
                let mir = match lower_to_mir(
                    &resolved_program,
                    &hir_program,
                    MirLoweringLimits {
                        max_functions: request.limits.max_mir_functions,
                        max_blocks_per_function: request.limits.max_mir_blocks_per_function,
                        max_locals_per_function: request.limits.max_mir_locals_per_function,
                        max_statements_per_function: request.limits.max_mir_statements_per_function,
                        max_verification_steps: request.limits.max_mir_verification_steps,
                    },
                ) {
                    Ok(mir) => mir,
                    Err(MirError::NodeLimit { span, resource }) => {
                        return syntax_resource_output(
                            &request,
                            span.file(),
                            format!("MIR {resource}"),
                            span.range().start(),
                        );
                    }
                    Err(MirError::VerificationLimit { resource }) => {
                        return syntax_resource_output(
                            &request,
                            request.root,
                            format!("MIR {resource}"),
                            0,
                        );
                    }
                    Err(error) => return Err(error.into()),
                };
                let bytecode = match lower_to_bytecode(
                    &resolved_program,
                    &hir_program,
                    &mir,
                    BytecodeLoweringLimits {
                        max_types: request.limits.max_bytecode_types,
                        max_nominals: request.limits.max_bytecode_nominals,
                        max_callables: request.limits.max_bytecode_callables,
                        max_constants: request.limits.max_bytecode_constants,
                        max_functions: request.limits.max_bytecode_functions,
                        max_slots_per_function: request.limits.max_bytecode_slots_per_function,
                        max_blocks_per_function: request.limits.max_bytecode_blocks_per_function,
                        max_instructions_per_function: request
                            .limits
                            .max_bytecode_instructions_per_function,
                        max_spans_per_function: request.limits.max_bytecode_spans_per_function,
                        max_generic_instantiations: request.limits.max_generic_instantiations,
                        max_verification_steps: request.limits.max_bytecode_verification_steps,
                    },
                ) {
                    Ok(bytecode) => bytecode,
                    Err(BytecodeError::NodeLimit { span, resource }) => {
                        let (file, offset) = span
                            .map(|span| (span.file(), span.range().start()))
                            .unwrap_or((request.root, 0));
                        return syntax_resource_output(
                            &request,
                            file,
                            format!("bytecode {resource}"),
                            offset,
                        );
                    }
                    Err(BytecodeError::VerificationLimit { resource }) => {
                        return syntax_resource_output(
                            &request,
                            request.root,
                            format!("bytecode {resource}"),
                            0,
                        );
                    }
                    Err(error) => return Err(error.into()),
                };
                let function = bytecode
                    .callables
                    .iter()
                    .find(|callable| callable.name == entry.canonical_name)
                    .and_then(|callable| callable.implementation)
                    .ok_or_else(|| {
                        DriverError::Invariant(
                            "selected main has no lowered bytecode implementation".into(),
                        )
                    })?;
                let mut host = BootstrapHost::with_max_bytes(
                    request.program_arguments.clone(),
                    request.limits.max_vm_heap_bytes,
                );
                if let Some(envelope) = request.test_envelope.clone() {
                    host.install_testing_envelope(envelope);
                }
                if let Some(participation) = request.test_participation.clone() {
                    host.install_testing_participation(participation);
                }
                let execution = match execute_with_limits(
                    &bytecode,
                    function,
                    &mut host,
                    vm_limits(request.limits),
                ) {
                    Ok(execution) => execution,
                    Err(VmError::InvalidLimits(resource)) => {
                        return syntax_resource_output(
                            &request,
                            request.root,
                            format!("VM {resource}"),
                            0,
                        );
                    }
                    Err(error) if error.is_resource_limit() => {
                        return syntax_resource_output(
                            &request,
                            request.root,
                            "VM execution resource",
                            0,
                        );
                    }
                    Err(error) => return Err(error.into()),
                };

                let (diagnostic, exit_code) = match execution.outcome {
                    VmOutcome::Returned(RuntimeValue::Unit) => (None, 0),
                    VmOutcome::Returned(RuntimeValue::ResultOk(value))
                        if matches!(value.as_ref(), RuntimeValue::Unit) =>
                    {
                        (None, 0)
                    }
                    VmOutcome::Returned(RuntimeValue::ResultErr(error)) => (
                        Some(unhandled_main_error_diagnostic(&entry, error.as_ref())?),
                        1,
                    ),
                    VmOutcome::Panicked(panic) => {
                        (Some(panic_diagnostic(&request.sources, &panic)?), 101)
                    }
                    VmOutcome::Returned(value) => {
                        return Err(DriverError::Invariant(format!(
                            "main returned a value incompatible with its admitted outcome: {value:?}"
                        )));
                    }
                };
                drop(parsed_sources);
                return semantic_output(
                    request,
                    resolved_program,
                    hir_program,
                    expression_diagnostics,
                    diagnostic,
                    exit_code,
                    host.take_stdout(),
                );
            }
        }
    }

    let location = request.sources.span(request.root, TextRange::empty(0))?;
    let diagnostic = Diagnostic::new(
        Severity::Error,
        DiagnosticCode::new("T0001")?,
        format!(
            "the `{}` pipeline is not implemented in the bootstrap compiler",
            request.operation.as_str()
        ),
        PrimaryLocation::Source(location),
    )?;
    let mut bag = DiagnosticBag::new();
    bag.extend(expression_diagnostics);
    bag.push(diagnostic);
    let report = bag.resolve(request.edition.as_str(), &request.sources)?;
    drop(parsed_sources);

    Ok(CompilationOutput {
        status: CompilationStatus::Rejected,
        exit_code: 1,
        diagnostics: report,
        stdout: Vec::new(),
        semantic_model: Some(SemanticModel::with_hir(
            request.sources,
            resolved_program,
            hir_program,
        )),
        products: None,
    })
}

/// Discovers executable test leaves in the request's root package. Parsing is
/// repeated here deliberately: discovery is a read-only planning operation;
/// the selected request is parsed and checked again by [`execute`].
pub fn discover_tests(
    request: &CompilationRequest,
) -> Result<Vec<test_backend::TestEntry>, DriverError> {
    let root_package = request.packages.root().clone();
    let mut entries = Vec::new();
    for (file, source) in request.sources.iter() {
        let Some(package) = request.packages.package_for_source(source.source_id()) else {
            return Err(DriverError::Invariant(
                "test discovery encountered an unowned source".into(),
            ));
        };
        if package.id() != &root_package {
            continue;
        }
        let (lex_mode, parse_mode) = if file == request.root {
            (LexMode::Module, ParseMode::Module)
        } else {
            (LexMode::ImportedModule, ParseMode::ImportedModule)
        };
        let lexed = lex_with_limits(
            &request.sources,
            file,
            lex_mode,
            LexLimits {
                max_tokens: request.limits.max_syntax_tokens as usize,
                max_diagnostics: request.limits.max_diagnostics as usize,
                max_nesting_depth: request.limits.max_syntax_depth,
            },
        )
        .map_err(|error| match error {
            LexError::ResourceLimit { resource, offset } => DriverError::Invariant(format!(
                "test discovery hit lexical {resource} limit at {offset}"
            )),
            other => DriverError::Lex(other),
        })?;
        if !lexed.diagnostics().is_empty() {
            continue;
        }
        let parsed = parse(
            &request.sources,
            file,
            lexed,
            parse_mode,
            ParseLimits {
                max_nodes: request.limits.max_syntax_nodes,
                max_nesting_depth: request.limits.max_syntax_depth,
                max_diagnostics: request.limits.max_diagnostics,
            },
        )
        .map_err(|error| match error {
            ParseError::ResourceLimit { resource, offset } => DriverError::Invariant(format!(
                "test discovery hit syntax {resource} limit at {offset}"
            )),
            other => DriverError::Parse(other),
        })?;
        if !parsed.diagnostics().is_empty() {
            continue;
        }
        let package_name = package.local_name().as_str();
        entries.extend(
            test_backend::discover(
                &request.sources,
                file,
                parsed.cst(),
                package.id(),
                package_name,
            )
            .map_err(|error| DriverError::Invariant(error.to_string()))?,
        );
    }
    entries.sort_by(|left, right| left.id().cmp(right.id()));
    Ok(entries)
}

/// Lowers one source-level test declaration to an ordinary `main` and runs it
/// through the regular compiler/VM pipeline.  Keeping this adapter outside the
/// normal `Run` branch prevents test declarations from being treated as a
/// script and makes the backend boundary explicit.
fn execute_test(request: CompilationRequest) -> Result<CompilationOutput, DriverError> {
    if request.source_form != SourceForm::Module {
        return backend_diagnostic_output(
            &request,
            "E2012",
            "test execution requires module source form",
        );
    }
    request.sources.get(request.root)?;
    let lexed = match lex_with_limits(
        &request.sources,
        request.root,
        LexMode::Module,
        LexLimits {
            max_tokens: request.limits.max_syntax_tokens as usize,
            max_diagnostics: request.limits.max_diagnostics as usize,
            max_nesting_depth: request.limits.max_syntax_depth,
        },
    ) {
        Ok(lexed) => lexed,
        Err(LexError::ResourceLimit { resource, offset }) => {
            return syntax_resource_output(&request, request.root, resource, offset);
        }
        Err(error) => return Err(error.into()),
    };
    if !lexed.diagnostics().is_empty() {
        let mut bag = DiagnosticBag::new();
        bag.extend(lexed.into_diagnostics());
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }
    let parsed = match parse(
        &request.sources,
        request.root,
        lexed,
        ParseMode::Module,
        ParseLimits {
            max_nodes: request.limits.max_syntax_nodes,
            max_nesting_depth: request.limits.max_syntax_depth,
            max_diagnostics: request.limits.max_diagnostics,
        },
    ) {
        Ok(parsed) => parsed,
        Err(ParseError::ResourceLimit { resource, offset }) => {
            return syntax_resource_output(&request, request.root, resource, offset);
        }
        Err(error) => return Err(error.into()),
    };
    if !parsed.diagnostics().is_empty() {
        let mut bag = DiagnosticBag::new();
        bag.extend(parsed.diagnostics().iter().cloned());
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
            products: None,
        });
    }

    let root_module = request
        .packages
        .module_for_file(&request.sources, request.root)?;
    let package_name = request
        .packages
        .package(root_module.package())
        .map(|package| package.local_name().as_str())
        .unwrap_or("main");
    let lowered_result = if request.test_participation_entries.is_empty() {
        test_backend::lower_selected(
            &request.sources,
            request.root,
            parsed.cst(),
            root_module.package(),
            package_name,
            request.test_entry(),
        )
    } else {
        test_backend::lower_participation(
            &request.sources,
            request.root,
            parsed.cst(),
            root_module.package(),
            package_name,
            request
                .test_participation_entries
                .iter()
                .map(String::as_str),
        )
    };
    let lowered = match lowered_result {
        Ok(lowered) => lowered,
        Err(test_backend::TestBackendError::Source(error)) => return Err(error.into()),
        Err(test_backend::TestBackendError::ProductionMain) => {
            return backend_diagnostic_output(
                &request,
                "E2011",
                "a test target cannot declare a `main` entry point",
            );
        }
        Err(error) => {
            return backend_diagnostic_output(&request, "E2012", error.to_string());
        }
    };
    let sources = clone_source_database(
        &request.sources,
        Some((
            request.root,
            std::sync::Arc::from(lowered),
            crate::source::SourceOrigin::GeneratedTesting,
        )),
    )?;
    let root = request.root;
    let test_envelope = request.test_envelope.clone();
    let test_participation = request.test_participation.clone();
    let mut nested = CompilationRequest::new(
        Operation::Run,
        request.edition,
        request.target.clone(),
        request.profile,
        request.capabilities.clone(),
        request.diagnostic_format,
        SourceForm::Module,
        request.limits,
        request.packages.clone(),
        sources,
        root,
    )?
    .with_program_arguments(request.program_arguments.clone())
    .with_declared_build_inputs(request.build_inputs.clone())
    .with_warning_profiles(request.warning_profiles.clone());
    if let Some(envelope) = test_envelope {
        nested = nested.with_test_envelope(envelope);
    }
    if let Some(participation) = test_participation {
        nested.test_participation = Some(participation);
    }
    nested.documentation_fixture = request.documentation_fixture;
    execute(nested)
}

fn clone_source_database(
    original: &SourceDatabase,
    replacement: Option<(FileId, std::sync::Arc<[u8]>, crate::source::SourceOrigin)>,
) -> Result<SourceDatabase, DriverError> {
    let mut sources = SourceDatabase::new();
    for (file, source) in original.iter() {
        let bytes = replacement
            .as_ref()
            .filter(|(replacement_file, _, _)| *replacement_file == file)
            .map(|(_, bytes, _)| bytes.clone())
            .unwrap_or_else(|| std::sync::Arc::from(source.bytes()));
        let origin = replacement
            .as_ref()
            .filter(|(replacement_file, _, _)| *replacement_file == file)
            .map_or(source.origin(), |(_, _, origin)| *origin);
        let actual = sources.add(crate::source::SourceInput::new(
            source.source_id().clone(),
            source.module().clone(),
            source.path().clone(),
            origin,
            bytes,
        ))?;
        if actual != file {
            return Err(DriverError::Invariant(
                "source clone changed file identity ordering".into(),
            ));
        }
    }
    Ok(sources)
}

fn backend_diagnostic_output(
    request: &CompilationRequest,
    code: &'static str,
    message: impl Into<String>,
) -> Result<CompilationOutput, DriverError> {
    let mut bag = DiagnosticBag::new();
    bag.push(Diagnostic::new(
        Severity::Error,
        DiagnosticCode::new(code)?,
        message,
        PrimaryLocation::Source(request.sources.span(request.root, TextRange::empty(0))?),
    )?);
    Ok(CompilationOutput {
        status: CompilationStatus::Rejected,
        exit_code: 1,
        diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
        stdout: Vec::new(),
        semantic_model: None,
        products: None,
    })
}

#[derive(Debug)]
struct MainEntry {
    canonical_name: String,
    span: Span,
    error_type: Option<String>,
}

enum MainSelection {
    Sync(MainEntry),
    Async(MainEntry),
    Rejected(Diagnostic),
}

fn select_hosted_main(
    request: &CompilationRequest,
    parsed: &[(FileId, Parsed)],
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<MainSelection, DriverError> {
    let root_module = request
        .packages
        .module_for_file(&request.sources, request.root)?;
    let script_statement = if request.source_form == SourceForm::Script {
        parsed
            .iter()
            .find(|(file, _)| *file == request.root)
            .and_then(|(_, parsed)| {
                parsed
                    .cst()
                    .root_node()
                    .child_nodes()
                    .find(|node| is_script_statement(node.kind()))
            })
            .map(|node| request.sources.span(request.root, node.range()))
            .transpose()?
    } else {
        None
    };
    let candidates = resolved
        .symbols()
        .filter(|symbol| {
            symbol.kind() == SymbolKind::Function
                && symbol.name().as_str() == "main"
                && symbol.identity().package() == root_module.package()
                && symbol.identity().module() == root_module.path()
        })
        .collect::<Vec<_>>();

    if candidates.len() > 1 {
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1802")?,
            "the hosted target has more than one `main` entry point",
            PrimaryLocation::Source(candidates[0].span()),
        )?;
        for candidate in candidates.iter().skip(1) {
            diagnostic = diagnostic.with_related(Related::new(
                "additional `main` entry point",
                candidate.span(),
            )?);
        }
        return Ok(MainSelection::Rejected(diagnostic));
    }

    let explicit = candidates.first().copied();
    if let Some(statement) = script_statement
        && let Some(symbol) = explicit
    {
        return Ok(MainSelection::Rejected(
            Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1802")?,
                "an explicit `main` cannot coexist with top-level script statements",
                PrimaryLocation::Source(symbol.span()),
            )?
            .with_related(Related::new("script entry also begins here", statement)?),
        ));
    }
    let symbol = if let Some(symbol) = explicit {
        symbol
    } else if script_statement.is_some() {
        resolved
            .symbols()
            .find(|symbol| {
                symbol.is_synthetic()
                    && symbol.kind() == SymbolKind::Function
                    && symbol.identity().package() == root_module.package()
                    && symbol.identity().module() == root_module.path()
            })
            .ok_or_else(|| {
                DriverError::Invariant(
                    "a script with top-level statements has no synthetic entry point".into(),
                )
            })?
    } else {
        return Ok(MainSelection::Rejected(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1806")?,
            "the hosted target has no explicit `main` and no script entry",
            PrimaryLocation::Target(request.target.diagnostic_source_id().clone()),
        )?));
    };

    let id = HirCallableId::Symbol(symbol.id());
    let callable = hir.callable(id).ok_or_else(|| {
        DriverError::Invariant("resolved main has no typed callable signature".into())
    })?;
    let function = match hir
        .interner()
        .kind(callable.function_type())
        .map_err(HirError::from)?
    {
        TypeKind::Function(function) => function,
        _ => {
            return Err(DriverError::Invariant(
                "typed main does not have a function type".into(),
            ));
        }
    };
    let mut violations = Vec::new();
    if symbol.visibility() != Visibility::Private {
        violations.push("be private");
    }
    if !callable.parameters().is_empty() {
        violations.push("take no parameters");
    }
    if !callable.generics().is_empty() {
        violations.push("be non-generic");
    }
    if callable.body_source().is_none() {
        violations.push("have a body");
    }
    if function.is_unsafe() {
        violations.push("not be unsafe");
    }
    let error_type = match hir
        .interner()
        .kind(callable.outcome())
        .map_err(HirError::from)?
    {
        TypeKind::Scalar(ScalarType::Unit) => None,
        TypeKind::Result { success, error }
            if matches!(
                hir.interner().kind(*success).map_err(HirError::from)?,
                TypeKind::Scalar(ScalarType::Unit)
            ) =>
        {
            if hir.discard_status(*error) != Some(HirDiscardStatus::Satisfied) {
                violations.push("declare an error type that satisfies Discard");
            }
            Some(hir.interner().canonical(*error).map_err(HirError::from)?)
        }
        _ => {
            violations.push("return Unit or `Unit ! E`");
            None
        }
    };
    if !violations.is_empty() {
        let actual = hir
            .interner()
            .canonical(callable.function_type())
            .map_err(HirError::from)?;
        return Ok(MainSelection::Rejected(
            Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1803")?,
                format!("invalid `main`: it must {}", violations.join(", ")),
                PrimaryLocation::Source(symbol.span()),
            )?
            .with_expected_actual(Some("fn(): Unit or fn(): Unit ! E".into()), Some(actual)),
        ));
    }

    let entry = MainEntry {
        canonical_name: symbol.identity().canonical_name(),
        span: symbol.span(),
        error_type,
    };
    if function.is_async() {
        Ok(MainSelection::Async(entry))
    } else {
        Ok(MainSelection::Sync(entry))
    }
}

fn vm_limits(limits: ResourceLimits) -> VmLimits {
    VmLimits {
        max_verification_steps: limits.max_bytecode_verification_steps,
        max_steps: limits.max_vm_steps,
        max_stack_depth: limits.max_vm_stack_depth,
        max_heap_objects: limits.max_vm_heap_objects,
        max_heap_bytes: limits.max_vm_heap_bytes,
        initial_gc_threshold: limits.initial_vm_gc_threshold,
    }
}

fn semantic_output(
    request: CompilationRequest,
    resolved: ResolvedProgram,
    hir: HirProgram,
    diagnostics: Vec<Diagnostic>,
    runtime_diagnostic: Option<Diagnostic>,
    exit_code: u8,
    stdout: Vec<u8>,
) -> Result<CompilationOutput, DriverError> {
    let mut bag = DiagnosticBag::new();
    bag.extend(diagnostics);
    if let Some(diagnostic) = runtime_diagnostic {
        bag.push(diagnostic);
    }
    let products = build_products(
        request.edition.as_str(),
        request.source_form.as_str(),
        request.target.name(),
        request.profile.as_str(),
        request
            .capabilities
            .iter()
            .map(|capability| capability.as_str().to_owned()),
        &request.build_inputs,
        &request.packages,
        &request.sources,
        &resolved,
        &hir,
    )?;
    let diagnostics = bag.resolve(request.edition.as_str(), &request.sources)?;
    Ok(CompilationOutput {
        status: if exit_code == 0 {
            CompilationStatus::Success
        } else {
            CompilationStatus::Rejected
        },
        exit_code,
        diagnostics,
        stdout,
        semantic_model: Some(SemanticModel::with_hir(request.sources, resolved, hir)),
        products: Some(products),
    })
}

fn unhandled_main_error_diagnostic(
    entry: &MainEntry,
    error: &RuntimeValue,
) -> Result<Diagnostic, DriverError> {
    let error_type = entry
        .error_type
        .as_deref()
        .ok_or_else(|| DriverError::Invariant("infallible main returned a Result error".into()))?;
    let detail = match error {
        RuntimeValue::Variant { variant, .. } => format!(" variant#{variant}"),
        RuntimeValue::Union { member, .. } => format!(" union-member#{member}"),
        RuntimeValue::OptionNone => " none".into(),
        RuntimeValue::OptionSome(_) => " some".into(),
        RuntimeValue::ResultOk(_) => " ok".into(),
        RuntimeValue::ResultErr(_) => " err".into(),
        _ => String::new(),
    };
    Ok(Diagnostic::new(
        Severity::Error,
        DiagnosticCode::new("R0001")?,
        format!("unhandled-main-error: `{error_type}`{detail}"),
        PrimaryLocation::Source(entry.span),
    )?)
}

fn panic_diagnostic(sources: &SourceDatabase, panic: &VmPanic) -> Result<Diagnostic, DriverError> {
    let primary = source_span_from_bytecode(sources, panic.span)?;
    let message = panic.message.replace('\r', "\\r").replace('\n', "\\n");
    let mut diagnostic = Diagnostic::new(
        Severity::Error,
        DiagnosticCode::new(panic.code.code())?,
        format!("{}: {message}", panic.code.name()),
        PrimaryLocation::Source(primary),
    )?;
    for frame in panic.stack.iter().skip(1) {
        diagnostic = diagnostic.with_related(Related::new(
            format!("called from {}", frame.function),
            source_span_from_bytecode(sources, frame.span)?,
        )?);
    }
    diagnostic = attach_suppressed_panics(sources, diagnostic, &panic.suppressed)?;
    Ok(diagnostic)
}

fn attach_suppressed_panics(
    sources: &SourceDatabase,
    mut diagnostic: Diagnostic,
    suppressed: &[VmPanic],
) -> Result<Diagnostic, DriverError> {
    let mut pending = suppressed.iter().rev().collect::<Vec<_>>();
    while let Some(panic) = pending.pop() {
        let message = panic.message.replace('\r', "\\r").replace('\n', "\\n");
        diagnostic = diagnostic.with_related(Related::new(
            format!("suppressed {}: {message}", panic.code.name()),
            source_span_from_bytecode(sources, panic.span)?,
        )?);
        pending.extend(panic.suppressed.iter().rev());
    }
    Ok(diagnostic)
}

fn source_span_from_bytecode(
    sources: &SourceDatabase,
    span: BytecodeSpan,
) -> Result<Span, DriverError> {
    let file = FileId::from_index(span.file as usize)?;
    Ok(sources.span(file, TextRange::new(span.start, span.end)?)?)
}

fn syntax_resource_output(
    request: &CompilationRequest,
    file: FileId,
    resource: impl fmt::Display,
    offset: u32,
) -> Result<CompilationOutput, DriverError> {
    let mut bag = DiagnosticBag::new();
    bag.push(Diagnostic::new(
        Severity::Error,
        DiagnosticCode::new("T0002")?,
        format!("{resource} limit exceeded"),
        PrimaryLocation::Source(request.sources.span(file, TextRange::empty(offset))?),
    )?);
    Ok(CompilationOutput {
        status: CompilationStatus::Rejected,
        exit_code: 1,
        diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
        stdout: Vec::new(),
        semantic_model: None,
        products: None,
    })
}

fn resource_limit_diagnostic(
    request: &CompilationRequest,
) -> Result<Option<Diagnostic>, DriverError> {
    if request.sources.len() > request.limits.max_files as usize {
        return Ok(Some(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("T0002")?,
            format!(
                "source file limit exceeded: {} > {}",
                request.sources.len(),
                request.limits.max_files
            ),
            PrimaryLocation::Target(request.target.diagnostic_source_id().clone()),
        )?));
    }
    for index in 0..request.sources.len() {
        let file_id = FileId::from_index(index)?;
        let file = request.sources.get(file_id)?;
        if file.length() > request.limits.max_source_bytes {
            return Ok(Some(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("T0002")?,
                format!(
                    "source byte limit exceeded: {} > {}",
                    file.length(),
                    request.limits.max_source_bytes
                ),
                PrimaryLocation::Source(request.sources.span(file_id, TextRange::empty(0))?),
            )?));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::package::{PackageAlias, PackageId, PackageNode};
    use crate::source::{LogicalPath, ModulePath, SourceInput};

    fn request(format: DiagnosticFormat) -> CompilationRequest {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"fn main() {}\n"[..]),
            ))
            .unwrap();
        CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BuildTarget::vm_hosted_capabilities(),
            format,
            SourceForm::Module,
            ResourceLimits::default(),
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap()
    }

    fn source_request(
        bytes: &'static [u8],
        source_form: SourceForm,
        limits: ResourceLimits,
    ) -> CompilationRequest {
        operation_request(Operation::Check, bytes, source_form, limits)
    }

    #[test]
    fn test_operation_executes_a_real_assertion_through_the_vm() {
        let request = operation_request(
            Operation::Test,
            b"test smoke { assert(true) }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().is_empty());
    }

    #[test]
    fn test_operation_executes_std_testing_value_helpers_through_the_vm() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\ntest helpers {\n let tolerance = match testing.FloatTolerance.from(0.01, 0.1) {\n  ok(value) => value\n  err(_) => testing.failNow(\"invalid tolerance\")\n }\n testing.assertTextEqual(\"same\", \"same\")\n testing.assertFloatNear(10.0, 10.5, ref tolerance)\n testing.assertFloat32Near(10.0, 10.5, ref tolerance)\n let diff = testing.diffText(\"old\\n\", \"new\\n\")\n testing.assertTextEqual(diff.render(), \"--- expected\\n+++ actual\\n-old\\n+new\\n\")\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let envelope = crate::test_control::EnvelopeHandle::new(
            "helpers",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let request = base
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone());
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(envelope.report().unwrap().terminal().is_none());
    }

    #[test]
    fn test_operation_executes_generic_testing_assertions_and_wrapper_consumers() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\n\
             test helpers {\n\
              let value = testing.assertSome(some(42))\n\
              let expectedValue = 42\n\
              let differentValue = 41\n\
              testing.assertEqual(ref value, ref expectedValue)\n\
              testing.assertNotEqual(ref value, ref differentValue)\n\
              let absent: Int? = none\n\
              testing.assertNone(absent)\n\
              let success: Int ! String = ok(7)\n\
              let failure: Int ! String = err(\"bad\")\n\
              let successValue = testing.assertOk(success)\n\
              let failureValue = testing.assertErr(failure)\n\
              let expectedSuccess = 7\n\
              let expectedFailure = \"bad\"\n\
              testing.assertEqual(ref successValue, ref expectedSuccess)\n\
              testing.assertEqual(ref failureValue, ref expectedFailure)\n\
             }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let envelope = crate::test_control::EnvelopeHandle::new(
            "helpers",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        envelope
            .set_phase(crate::test_control::ExecutionPhase::Body)
            .unwrap();
        let output = execute(
            base.for_test_entry(&entries[0])
                .unwrap()
                .with_test_envelope(envelope.clone()),
        )
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(envelope.report().unwrap().terminal().is_none());
    }

    #[test]
    fn test_operation_virtualizes_the_production_monotonic_clock() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\nimport std.time\ntest virtual_clock {\n match await testing.withVirtualTime(async (clock) {\n  let before = time.now()?\n  await clock.advance(time.Duration.fromNanoseconds(100))\n  let after = time.now()?\n  assert(after.durationSince(before)?.toNanoseconds() == 100)\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual clock failed\")\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let envelope = crate::test_control::EnvelopeHandle::new(
            "virtual_clock",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        let request = base
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone());
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0);
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time().len(), 1);
        assert_eq!(report.virtual_time()[0].index(), 1);
        assert_eq!(report.virtual_time()[0].elapsed_ns(), 100);
        assert_eq!(report.virtual_time()[0].automatic_advances(), 0);
        assert_eq!(report.virtual_time()[0].advances(), 1);
    }

    #[test]
    fn test_operation_accepts_affine_virtual_time_body_and_settles_spawned_timers() {
        let base = operation_request(
            Operation::Check,
            b"import std.bytes\nimport std.testing\nimport std.time\nasync fn exerciseVirtualTime(): Unit ! (bytes.BytesError | time.ClockError) {\n var affine = bytes.builder()?\n await testing.withVirtualTime(async (clock) {\n  _ = affine.finish()?\n  let before = time.now()?\n  scope {\n   let sleeper = spawn time.sleep(time.Duration.fromNanoseconds(40))\n   await clock.settle()\n   await sleeper?\n  }\n  let after = time.now()?\n  assert(after.durationSince(before)?.toNanoseconds() == 40)\n })?\n await testing.withVirtualTime(async (clock) {\n  _ = time.now()?\n  await clock.advance(time.Duration.fromNanoseconds(5))\n })?\n}\ntest virtual_settle {\n match await exerciseVirtualTime() {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual time failed\")\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let envelope = crate::test_control::EnvelopeHandle::new(
            "virtual_settle",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        let request = base
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone());
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time().len(), 2);
        assert_eq!(report.virtual_time()[0].index(), 1);
        assert_eq!(report.virtual_time()[0].elapsed_ns(), 40);
        assert_eq!(report.virtual_time()[0].automatic_advances(), 1);
        assert_eq!(report.virtual_time()[0].settles(), 1);
        assert_eq!(report.virtual_time()[1].index(), 2);
        assert_eq!(report.virtual_time()[1].elapsed_ns(), 5);
        assert_eq!(report.virtual_time()[1].advances(), 1);
    }

    #[test]
    fn test_operation_rejects_spawning_the_virtual_time_boundary() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\nimport std.time\ntest invalid_spawn {\n scope {\n  let task = spawn testing.withVirtualTime(async (clock) {\n   _ = time.now()?\n   await clock.settle()\n  })\n  _ = await task\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let request = base.for_test_entry(&entries[0]).unwrap();
        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "E1601"),
            "{}",
            output.diagnostics().human()
        );
        assert!(
            output.diagnostics().diagnostics().iter().any(|diagnostic| {
                diagnostic.code() == "E1601"
                    && diagnostic
                        .message()
                        .contains("must be awaited directly and cannot be spawned")
            }),
            "{}",
            output.diagnostics().human()
        );
    }

    #[test]
    fn test_operation_rejects_non_send_virtual_time_body() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\nfn consumeUnit(value: Unit) {\n match value {\n  () => ()\n }\n}\nasync fn ready(): Unit {}\ntest invalid_capture {\n scope {\n  let task = spawn ready()\n  match await testing.withVirtualTime(async (clock) {\n   _ = clock\n   _ = await task\n  }) {\n   ok(_) => ()\n   err(_) => ()\n  }\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let request = base.for_test_entry(&entries[0]).unwrap();
        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(
            output.diagnostics().diagnostics().iter().any(|diagnostic| {
                diagnostic.code() == "E1108" && diagnostic.message().contains("Send")
            }),
            "{}",
            output.diagnostics().human()
        );
    }

    #[test]
    fn test_operation_closes_virtual_time_when_the_body_panics() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\nimport std.time\ntest virtual_panic {\n match await testing.withVirtualTime(async (clock) {\n  _ = time.now()?\n  await clock.advance(time.Duration.fromNanoseconds(7))\n  panic(\"inside virtual time\")\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"unexpected clock error\")\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let envelope = crate::test_control::EnvelopeHandle::new(
            "virtual_panic",
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
        );
        let request = base
            .for_test_entry(&entries[0])
            .unwrap()
            .with_test_envelope(envelope.clone());
        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "P0008"),
            "{}",
            output.diagnostics().human()
        );
        let report = envelope.report().unwrap();
        assert_eq!(report.virtual_time().len(), 1);
        assert_eq!(report.virtual_time()[0].elapsed_ns(), 7);
        assert_eq!(report.virtual_time()[0].advances(), 1);
    }

    #[test]
    fn test_operation_reports_a_runtime_assertion_failure() {
        let request = operation_request(
            Operation::Test,
            b"test smoke { assert(false) }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 101);
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "P0007")
        );
    }

    #[test]
    fn test_participation_preserves_suite_scope_and_isolates_leaf_panics() {
        let base = operation_request(
            Operation::Check,
            b"import std.testing\nsuite shared {\n testing.log(\"setup once\")\n let value = 42\n test failing { assert(false) }\n test passing { assert(value == 42) }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&base).unwrap();
        let participation = test_backend::TestParticipation::new(
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
            BTreeMap::new(),
            false,
        );
        let request = base
            .for_test_participation(&entries, participation.clone())
            .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let executions = participation.executions().unwrap();
        assert_eq!(
            executions
                .iter()
                .filter(|execution| execution.kind == test_backend::TestExecutionKind::Leaf)
                .count(),
            2
        );
        assert!(
            executions
                .iter()
                .any(|execution| execution.id.ends_with("::failing") && execution.panic.is_some())
        );
        let suite = executions
            .iter()
            .find(|execution| execution.kind == test_backend::TestExecutionKind::Suite)
            .unwrap();
        assert_eq!(suite.report.logs().len(), 1);
        assert_eq!(suite.report.logs()[0].message(), "setup once");
        assert!(
            executions
                .iter()
                .any(|execution| execution.id.ends_with("::passing") && execution.panic.is_none())
        );
    }

    #[test]
    fn test_operation_rejects_script_source_form_with_a_typed_diagnostic() {
        let request = operation_request(
            Operation::Test,
            b"test smoke { assert(true) }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        );
        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E2012");
    }

    #[test]
    fn test_discovery_and_fork_execute_one_of_multiple_leaves() {
        let request = operation_request(
            Operation::Check,
            b"test first { assert(true) }\ntest second { assert(false) }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&request).unwrap();
        assert_eq!(entries.len(), 2);
        let fork = request.for_test_entry(&entries[1]).unwrap();
        let output = execute(fork).unwrap();
        assert_eq!(output.exit_code(), 101);
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "P0007")
        );
    }

    fn operation_request(
        operation: Operation,
        bytes: &[u8],
        source_form: SourceForm,
        limits: ResourceLimits,
    ) -> CompilationRequest {
        operation_request_with_capabilities(
            operation,
            bytes,
            source_form,
            limits,
            BuildTarget::vm_hosted_capabilities(),
        )
    }

    fn operation_request_with_capabilities(
        operation: Operation,
        bytes: &[u8],
        source_form: SourceForm,
        limits: ResourceLimits,
        capabilities: BTreeSet<CapabilityName>,
    ) -> CompilationRequest {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:driver-test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(bytes),
            ))
            .unwrap();
        CompilationRequest::new(
            operation,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            capabilities,
            DiagnosticFormat::Json,
            source_form,
            limits,
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap()
    }

    fn multimodule_request(
        operation: Operation,
        main_source: &[u8],
        api_source: &[u8],
    ) -> CompilationRequest {
        let mut sources = SourceDatabase::new();
        let source_id = SourceId::new("source:driver-multimodule").unwrap();
        let root = sources
            .add(SourceInput::virtual_file(
                source_id.clone(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(main_source),
            ))
            .unwrap();
        sources
            .add(SourceInput::virtual_file(
                source_id.clone(),
                ModulePath::new("api").unwrap(),
                LogicalPath::new("api.to").unwrap(),
                Arc::<[u8]>::from(api_source),
            ))
            .unwrap();
        let app = PackageId::new("pkg:driver-multimodule").unwrap();
        let standard = PackageId::new("pkg:std").unwrap();
        let graph = PackageGraph::new(
            app.clone(),
            standard.clone(),
            [
                PackageNode::new(
                    app,
                    source_id,
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    [
                        ModulePath::new("api").unwrap(),
                        ModulePath::new("main").unwrap(),
                    ],
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    standard,
                    SourceId::new("source:std").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        CompilationRequest::new(
            operation,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BuildTarget::vm_hosted_capabilities(),
            DiagnosticFormat::Json,
            SourceForm::Script,
            ResourceLimits::default(),
            graph,
            sources,
            root,
        )
        .unwrap()
    }

    #[test]
    fn bootstrap_standard_modules_follow_the_closed_target_capabilities() {
        let source = b"import std.console\nfn main() { console.print(\"ready\") }\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `console` is missing")
        );

        let accepted = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(accepted.status(), CompilationStatus::Success);
        assert!(accepted.diagnostics().diagnostics().is_empty());

        assert!(CapabilityName::new("console").is_ok());
        let process_source =
            b"import std.process\nfn main() {\n    let command = process.cmd(\"true\")\n}\n";
        let process_rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            process_source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::from([CapabilityName::new("console").unwrap()]),
        ))
        .unwrap();
        assert_eq!(process_rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &process_rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `process` is missing")
        );

        let process_accepted = execute(operation_request(
            Operation::Check,
            process_source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(process_accepted.status(), CompilationStatus::Success);
        assert!(CapabilityName::new("process").is_ok());
        assert!(matches!(
            CapabilityName::new("made-up-capability"),
            Err(DriverError::InvalidCapability(_))
        ));

        let time_source =
            b"import std.time\nfn main(): !time.ClockError {\n    let instant = time.now()?\n}\n";
        let time_rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            time_source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(time_rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &time_rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `clock` is missing")
        );

        let time_accepted = execute(operation_request(
            Operation::Check,
            time_source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(time_accepted.status(), CompilationStatus::Success);
        assert!(time_accepted.diagnostics().diagnostics().is_empty());

        let env_source =
            b"import std.env\nfn main(): !env.EnvError {\n    let snapshot = env.snapshot()?\n}\n";
        let env_rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            env_source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(env_rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &env_rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `environment` is missing")
        );
        let env_accepted = execute(operation_request(
            Operation::Check,
            env_source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(env_accepted.status(), CompilationStatus::Success);
        assert!(env_accepted.diagnostics().diagnostics().is_empty());

        let baseline = request(DiagnosticFormat::Json);
        let root = baseline.root;
        assert!(matches!(
            CompilationRequest::new(
                Operation::Check,
                Edition::V0_1,
                BuildTarget::vm_hosted(),
                HostProfile::Hosted,
                BTreeSet::from([CapabilityName::new("network").unwrap()]),
                DiagnosticFormat::Json,
                SourceForm::Module,
                ResourceLimits::default(),
                baseline.packages,
                baseline.sources,
                root,
            ),
            Err(DriverError::UnsupportedTargetCapability { target, capability })
                if target == "tondo-vm-hosted" && capability == "network"
        ));
    }

    #[test]
    fn environment_module_requires_the_explicit_target_capability() {
        let source =
            b"import std.env\nfn main(): !env.EnvError {\n    let snapshot = env.snapshot()?\n}\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `environment` is missing")
        );

        let accepted = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(accepted.status(), CompilationStatus::Success);
        assert!(accepted.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn time_module_requires_the_explicit_clock_capability() {
        let source =
            b"import std.time\nfn main(): !time.ClockError {\n    let instant = time.now()?\n}\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        let diagnostic = &rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `clock` is missing")
        );

        let accepted = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(accepted.status(), CompilationStatus::Success);
        assert!(accepted.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn program_arguments_reach_process_args_without_cli_options() {
        let source = br#"
import std.console
import std.process

fn main() {
    assert(process.args() == ["--flag", "two words", "*", "$HOME"])
    console.print("args-ok\n")
}
"#;
        let output = execute(
            operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            )
            .with_program_arguments(vec![
                "--flag".into(),
                "two words".into(),
                "*".into(),
                "$HOME".into(),
            ]),
        )
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.stdout(), b"args-ok\n");
    }

    #[test]
    fn formatter_operation_returns_canonical_stdout_and_is_idempotent() {
        let output = execute(operation_request(
            Operation::Format,
            b"fn main(){let values=[1,2]\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();

        assert_eq!(output.status(), CompilationStatus::Success);
        assert!(output.diagnostics().diagnostics().is_empty());
        assert_eq!(
            output.stdout(),
            b"fn main() {\n    let values = [1, 2]\n}\n"
        );

        let second = execute(operation_request(
            Operation::Format,
            output.stdout(),
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(second.status(), CompilationStatus::Success);
        assert_eq!(second.stdout(), output.stdout());
    }

    #[test]
    fn compilation_request_rejects_sources_outside_the_closed_package_graph() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("source:app").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"fn main() {}\n"[..]),
            ))
            .unwrap();
        let root_package = PackageId::new("pkg:app").unwrap();
        let standard_package = PackageId::new("pkg:std").unwrap();
        let packages = PackageGraph::new(
            root_package.clone(),
            standard_package.clone(),
            [
                PackageNode::new(
                    root_package,
                    SourceId::new("source:app").unwrap(),
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    [ModulePath::new("different").unwrap()],
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    standard_package,
                    SourceId::new("source:std").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap();

        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            packages,
            sources,
            root,
        );

        assert!(matches!(request, Err(DriverError::PackageGraph(_))));
    }

    #[test]
    fn formatter_operation_honors_script_and_fragment_source_forms() {
        let script = execute(operation_request(
            Operation::Format,
            b"#!/usr/bin/env tondo\nlet value=1\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(script.status(), CompilationStatus::Success);
        assert_eq!(script.stdout(), b"#!/usr/bin/env tondo\n\nlet value = 1\n");

        let fragment = execute(operation_request(
            Operation::Format,
            b"let value=[1,2]\n",
            SourceForm::Fragment,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(fragment.status(), CompilationStatus::Success);
        assert_eq!(fragment.into_stdout(), b"let value = [1, 2]\n");
    }

    #[test]
    fn formatter_operation_rejects_invalid_syntax_without_stdout() {
        let output = execute(operation_request(
            Operation::Format,
            b"enum Empty {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();

        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E0004");
        assert!(output.stdout().is_empty());
    }

    #[test]
    fn formatter_resource_rejection_never_emits_partial_stdout() {
        let output = execute(operation_request(
            Operation::Format,
            b"fn main() {}\n",
            SourceForm::Module,
            ResourceLimits {
                max_syntax_nodes: 1,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();

        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(output.stdout().is_empty());
    }

    #[test]
    fn completed_check_returns_a_semantic_snapshot_without_diagnostics() {
        let output = execute(request(DiagnosticFormat::Json)).unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert!(output.diagnostics().diagnostics().is_empty());
        assert_eq!(output.diagnostics().json_lines().unwrap(), "");
        assert!(output.semantic_model().is_some());
    }

    #[test]
    fn driver_reports_source_byte_budget() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"four"[..]),
            ))
            .unwrap();
        let limits = ResourceLimits {
            max_source_bytes: 3,
            ..ResourceLimits::default()
        };

        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            limits,
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap();
        let output = execute(request).unwrap();

        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
    }

    #[test]
    fn lexical_diagnostic_preempts_the_unimplemented_pipeline_marker() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"let value = 01\n"[..]),
            ))
            .unwrap();
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            ResourceLimits::default(),
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap();

        let output = execute(request).unwrap();
        let diagnostics = output.diagnostics().diagnostics();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code(), "E0003");
    }

    #[test]
    fn non_root_files_never_inherit_script_shebang_permission() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"#!/usr/bin/env tondo\nlet value = 1\n"[..]),
            ))
            .unwrap();
        sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("dependency").unwrap(),
                LogicalPath::new("dependency.to").unwrap(),
                Arc::<[u8]>::from(&b"#!/usr/bin/env tondo\nconst Value = 1\n"[..]),
            ))
            .unwrap();
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Script,
            ResourceLimits::default(),
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap();

        let output = execute(request).unwrap();
        assert_eq!(output.diagnostics().diagnostics().len(), 1);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1801");
    }

    #[test]
    fn syntax_resource_limit_is_a_rejection_not_an_internal_error() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"value"[..]),
            ))
            .unwrap();
        let limits = ResourceLimits {
            max_syntax_tokens: 2,
            ..ResourceLimits::default()
        };
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            limits,
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap();

        let output = execute(request).unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
    }

    #[test]
    fn parser_diagnostics_preempt_the_unimplemented_pipeline_marker() {
        for (source, expected) in [
            (&b"enum Empty {}\n"[..], "E0004"),
            (
                &b"fn chained(value: Int): Bool {\n    0 < value < 10\n}\n"[..],
                "E0005",
            ),
            (&b"let value = 1\n"[..], "E1804"),
        ] {
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            let diagnostics = output.diagnostics().diagnostics();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(diagnostics.len(), 1, "{source:?}");
            assert_eq!(diagnostics[0].code(), expected, "{source:?}");
        }
    }

    #[test]
    fn resolution_diagnostics_preempt_the_unimplemented_pipeline_marker() {
        for (source, expected) in [
            (&b"fn duplicate() {}\nfn duplicate() {}\n"[..], "E1002"),
            (&b"fn String() {}\n"[..], "E1005"),
            (&b"fn first() {}\nimport main.missing\n"[..], "E1007"),
            (&b"import main.missing\nfn main() {}\n"[..], "E1008"),
        ] {
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            let diagnostics = output.diagnostics().diagnostics();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert!(
                !diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code() == "T0001")
            );
            assert_eq!(diagnostics[0].code(), expected, "{source:?}");
        }
    }

    #[test]
    fn type_lowering_diagnostics_preempt_the_unimplemented_pipeline_marker() {
        for (source, expected) in [
            (&b"fn invalid(value: Array[Int, String]) {}\n"[..], "E1104"),
            (
                &b"alias First = Second\nalias Second = First\n"[..],
                "E1106",
            ),
            (&b"type Invalid = { next: Invalid }\n"[..], "E1107"),
            (
                &b"trait Summary {}\nfn consume(value: Summary) {}\n"[..],
                "E1110",
            ),
            (&b"pub const Missing = 1\n"[..], "E1115"),
        ] {
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            let diagnostics = output.diagnostics().diagnostics();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(diagnostics.len(), 1, "{source:?}");
            assert_eq!(diagnostics[0].code(), expected, "{source:?}");
            assert!(
                !diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code() == "T0001")
            );
        }
    }

    #[test]
    fn expression_type_diagnostics_preempt_the_unimplemented_pipeline_marker() {
        for (source, expected) in [
            (&b"fn invalid(): Int { \"text\" }\n"[..], "E1102"),
            (&b"fn invalid() {\n    let value = none\n}\n"[..], "E1304"),
            (&b"fn invalid() {\n    var value: Int\n}\n"[..], "E1109"),
            (
                &b"const First: Int = Second\nconst Second: Int = First\n"[..],
                "E1902",
            ),
            (
                &b"fn runtime(): Int { 1 }\nconst Invalid: Int = runtime()\n"[..],
                "E1901",
            ),
            (&b"const Invalid: Int = 1 / 0\n"[..], "E1903"),
            (
                &b"const Entries: Map[String, Int] = [\"a\": 1, \"a\": 2]\n"[..],
                "E1116",
            ),
            (&b"fn invalid(): Int {\n    return\n}\n"[..], "E1205"),
            (
                &b"fn invalid() {\n    for value in 42 { () }\n}\n"[..],
                "E1206",
            ),
            (&b"fn invalid() { 1\n() }\n"[..], "E1303"),
            (
                &b"fn inspect(value: ref Int) {}\nfn invalid() { let value = 1\ninspect(value) }\n"
                    [..],
                "E1407",
            ),
            (
                &b"fn invalid[T: Discard](value: T) {\n    _ = value\n    _ = value\n}\n"[..],
                "E1401",
            ),
            (
                &b"fn source(): Int ! String { 1 }\nfn invalid(): Int { source()? }\n"[..],
                "E1301",
            ),
            (&b"fn invalid() {\n    fail \"bad\"\n}\n"[..], "E1302"),
            (&b"fn invalid(): Int ! Bool { err(\"bad\") }\n"[..], "E1304"),
            (
                &b"fn invalid(value: Int?) {\n    let some(number) = value\n}\n"[..],
                "E1201",
            ),
            (
                &b"fn invalid(value: Bool): Int {\n    match value {\n        some(_) => 1\n        _ => 0\n    }\n}\n"[..],
                "E1202",
            ),
            (
                &b"fn invalid(value: Bool): Int {\n    match value {\n        _ => 0\n        true => 1\n    }\n}\n"[..],
                "E1203",
            ),
            (
                &b"fn invalid(value: Bool): Int {\n    match value {\n        true => 1\n    }\n}\n"[..],
                "E1204",
            ),
            (
                &b"fn invalid() {\n    let value = 1\n    value = 2\n}\n"[..],
                "E1411",
            ),
            (
                &b"fn invalid() {\n    var value = 0\n    (value, value) = (1, 2)\n}\n"[..],
                "E1405",
            ),
            (
                &b"fn invalid(task: Join[Int, Never]) {\n    _ = task\n}\n"[..],
                "E1105",
            ),
        ] {
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            let diagnostics = output.diagnostics().diagnostics();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(diagnostics.len(), 1, "{source:?}");
            assert_eq!(diagnostics[0].code(), expected, "{source:?}");
            assert!(
                !diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.code() == "T0001")
            );
        }
    }

    #[test]
    fn expression_warnings_do_not_reject_a_completed_check() {
        let output = execute(
            source_request(
                b"fn main() {\n    return\n    let unreachable = 1\n}\n",
                SourceForm::Module,
                ResourceLimits::default(),
            )
            .with_warning_profiles([WarningProfile::Core]),
        )
        .unwrap();
        let diagnostics = output.diagnostics().diagnostics();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "W1006" && diagnostic.severity() == Severity::Warning
        }));
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity() == Severity::Warning)
        );
        assert!(
            !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code() == "T0001")
        );
    }

    #[test]
    fn constant_warnings_remain_visible_on_a_successful_check() {
        for (source, warning) in [
            (
                &b"fn values(): Set[String] { Set[\"a\", \"a\"] }\n"[..],
                "W1011",
            ),
            (
                &b"const Zero: Float = 0.0\nconst Nan: Float = Zero / Zero\nconst Known: Bool = Nan == Nan\n"[..],
                "W1008",
            ),
        ] {
            let output = execute(
                source_request(source, SourceForm::Module, ResourceLimits::default())
                    .with_warning_profiles([WarningProfile::Core]),
            )
            .unwrap();
            let diagnostics = output.diagnostics().diagnostics();
            assert_eq!(output.status(), CompilationStatus::Success);
            assert_eq!(diagnostics.len(), 1);
            assert!(diagnostics.iter().any(|diagnostic| {
                diagnostic.code() == warning && diagnostic.severity() == Severity::Warning
            }));
            assert!(!diagnostics.iter().any(|diagnostic| diagnostic.code() == "T0001"));
        }
    }

    #[test]
    fn warning_profiles_are_closed_and_opt_in() {
        let source = b"fn main() {\n    return\n    let unreachable = 1\n}\n";
        let without_profile = execute(source_request(
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert!(without_profile.diagnostics().is_empty());

        let with_core = execute(
            source_request(source, SourceForm::Module, ResourceLimits::default())
                .with_warning_profiles([WarningProfile::Core]),
        )
        .unwrap();
        assert_eq!(with_core.diagnostics().diagnostics()[0].code(), "W1006");
    }

    #[test]
    fn warning_profiles_never_relax_language_errors() {
        let source = b"fn invalid(): Int { \"wrong\" }\n";
        let baseline = execute(source_request(
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        let with_core = execute(
            source_request(source, SourceForm::Module, ResourceLimits::default())
                .with_warning_profiles([WarningProfile::Core]),
        )
        .unwrap();
        for output in [&baseline, &with_core] {
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1102");
        }
    }

    #[test]
    fn type_node_and_hir_diagnostic_budgets_are_enforced_through_the_driver() {
        let type_limits = ResourceLimits {
            max_type_nodes: 16,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn main() {}\n",
            SourceForm::Module,
            type_limits,
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("type node")
        );

        let diagnostic_limits = ResourceLimits {
            max_diagnostics: 0,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn invalid(value: Array[Int, String]) {}\n",
            SourceForm::Module,
            diagnostic_limits,
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");

        let hir_limits = ResourceLimits {
            max_hir_nodes: 0,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn main() {}\n",
            SourceForm::Module,
            hir_limits,
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("typed HIR node count")
        );

        let pattern_limits = ResourceLimits {
            max_pattern_analysis_steps: 0,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn inspect(value: Bool) {\n    match value {\n        true => ()\n        false => ()\n    }\n}\n",
            SourceForm::Module,
            pattern_limits,
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("pattern exhaustiveness analysis")
        );
    }

    #[test]
    fn resolver_diagnostic_budget_is_enforced_through_the_driver() {
        let limits = ResourceLimits {
            max_diagnostics: 0,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn String() {}\n",
            SourceForm::Module,
            limits,
        ))
        .unwrap();

        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
    }

    #[test]
    fn script_source_form_accepts_top_level_statements() {
        let output = execute(source_request(
            b"let value = 1\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn script_entry_executes_sync_and_async_top_level_work() {
        let sync = execute(operation_request(
            Operation::Run,
            b"#!/usr/bin/env tondo\n\
              import std.console\n\
              let answer = 6 * 7\n\
              console.print(\"{answer}\")\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(sync.status(), CompilationStatus::Success);
        assert_eq!(sync.exit_code(), 0);
        assert_eq!(sync.stdout(), b"42");

        let asynchronous = execute(operation_request(
            Operation::Run,
            b"async fn tick(): Int { 42 }\n\
              let answer = await tick()\n\
              scope {\n\
                  let job = spawn tick()\n\
                  assert(await job == answer)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(asynchronous.status(), CompilationStatus::Success);
        assert_eq!(asynchronous.exit_code(), 0);
        assert!(asynchronous.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn named_declarations_cannot_capture_script_locals() {
        let output = execute(operation_request(
            Operation::Check,
            b"fn read(): Int { answer }\nlet answer = 42\n_ = read()\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1001");
    }

    #[test]
    fn script_entry_infers_one_closed_error_union() {
        let output = execute(operation_request(
            Operation::Run,
            b"enum ReadError { Missing }\n\
              enum WriteError { Denied }\n\
              fn read(): Int ! ReadError {\n\
                  fail ReadError.Missing\n\
              }\n\
              fn write(): Int ! WriteError {\n\
                  fail WriteError.Denied\n\
              }\n\
              let first = read()?\n\
              let second = write()?\n\
              _ = (first, second)\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 1);
        let diagnostic = &output.diagnostics().diagnostics()[0];
        assert_eq!(
            diagnostic.code(),
            "R0001",
            "{:?}",
            output.diagnostics().diagnostics()
        );
        assert!(diagnostic.message().contains("ReadError"));
        assert!(diagnostic.message().contains("WriteError"));
    }

    #[test]
    fn run_pipeline_executes_sync_main_after_mir_and_bytecode_verification() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn main() {\n    let value = if true { 1 } else { 2 }\n    _ = value\n}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());

        let reinitialized = execute(operation_request(
            Operation::Run,
            b"fn replace[T: Discard](first: T, second: T): T {\n\
                  var value = first\n\
                  _ = value\n\
                  value = second\n\
                  value\n\
              }\n\
              fn main() {\n\
                  _ = replace(1, 42)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            reinitialized.status(),
            CompilationStatus::Success,
            "{:#?}",
            reinitialized.diagnostics().diagnostics()
        );
        assert_eq!(reinitialized.exit_code(), 0);

        for limits in [
            ResourceLimits {
                max_mir_blocks_per_function: 1,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_mir_verification_steps: 0,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_bytecode_types: 1,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_bytecode_verification_steps: 0,
                ..ResourceLimits::default()
            },
        ] {
            let output = execute(operation_request(
                Operation::Run,
                b"fn main() {\n    let value = if true { 1 } else { 2 }\n    _ = value\n}\n",
                SourceForm::Script,
                limits,
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
            let message = output.diagnostics().diagnostics()[0].message();
            assert!(message.contains("MIR") || message.contains("bytecode"));
        }
    }

    #[test]
    fn closure_protocols_and_invocation_cross_the_public_run_pipeline() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn keep[T: Copy + Discard](value: T): T {\n\
                  let closure = (): T { value }\n\
                  closure()\n\
              }\n\
              fn main() {\n\
                  let seed = 40\n\
                  let add = (value: Int): Int { seed + value }\n\
                  let copied = add\n\
                  assert(add(2) == 42)\n\
                  assert(copied(2) == 42)\n\
                  var count = 0\n\
                  var next = (): Int {\n\
                      count += 1\n\
                      count\n\
                  }\n\
                  assert(next() == 1)\n\
                  assert(next() == 2)\n\
                  assert(count == 0)\n\
                  assert(keep(42) == 42)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn closure_effects_cross_the_public_pipeline_without_premature_invocation() {
        let source = b"fn main() {\n\
              let sync: fn(): Int = () { 1 }\n\
              let raw: unsafe fn(): Int = unsafe () { 2 }\n\
              let later: async fn(): Int = async () { 3 }\n\
              let both: async unsafe fn(): Int = async unsafe () { 4 }\n\
              _ = sync\n\
              _ = raw\n\
              _ = later\n\
              _ = both\n\
          }\n";
        let checked = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(checked.status(), CompilationStatus::Success);
        let model = checked.semantic_model().unwrap();
        assert!(model.expression_check_complete());
        let closures = model.hir().unwrap().closures().collect::<Vec<_>>();
        assert_eq!(closures.len(), 4);
        assert_eq!(
            closures
                .iter()
                .map(|closure| (closure.is_async(), closure.is_unsafe()))
                .collect::<Vec<_>>(),
            vec![(false, false), (false, true), (true, false), (true, true)]
        );

        let run = execute(operation_request(
            Operation::Run,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            run.status(),
            CompilationStatus::Success,
            "{:#?}",
            run.diagnostics().diagnostics()
        );

        for (source, code) in [
            (
                &b"fn main() {\n    let operation = async (): Int { 1 }\n    _ = operation()\n}\n"
                    [..],
                "E1601",
            ),
            (
                &b"fn main() {\n    let operation = unsafe (): Int { 1 }\n    _ = operation()\n}\n"
                    [..],
                "E1701",
            ),
            (
                &b"async fn operation(): Int { 1 }\nfn main() {\n    _ = operation()\n}\n"[..],
                "E1601",
            ),
            (
                &b"unsafe fn operation(): Int { 1 }\nfn main() {\n    _ = operation()\n}\n"[..],
                "E1701",
            ),
        ] {
            let output = execute(operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), code);
        }
    }

    #[test]
    fn unsafe_calls_execute_only_through_explicit_regions() {
        let source = b"unsafe fn raw(value: Int): Int { value + 1 }\n\
            fn main() {\n\
                let direct = unsafe { raw(40) }\n\
                let operation = unsafe (value: Int): Int { raw(value) }\n\
                let indirect = unsafe { operation(1) }\n\
                assert(direct + indirect == 43)\n\
            }\n";
        let output = execute(operation_request(
            Operation::Run,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
    }

    #[test]
    fn async_unsafe_calls_keep_both_effects_visible() {
        let source = b"async unsafe fn raw(value: Int): Int { value + 1 }\n\
            async fn main() {\n\
                let result = unsafe { await raw(41) }\n\
                assert(result == 42)\n\
            }\n";
        let output = execute(operation_request(
            Operation::Run,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );

        let invalid = b"async unsafe fn raw(): Int { 1 }\n\
            async fn main() {\n\
                _ = await raw()\n\
            }\n";
        let output = execute(operation_request(
            Operation::Run,
            invalid,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1701");
    }

    #[test]
    fn raw_pointer_operations_lower_through_verified_bytecode() {
        let source = b"fn main() {\n\
            if false {\n\
                let pointer = unsafe { 1u64.toPointer[Int]() }\n\
                unsafe {\n\
                    let value = pointer.read()\n\
                    pointer.write(value)\n\
                    let advanced = pointer.offset(1)\n\
                    let bytes = advanced.cast[Byte]()\n\
                    _ = bytes.address()\n\
                    let qualifiedValue = Pointer.read(pointer)\n\
                    Pointer.write(pointer, qualifiedValue)\n\
                    let qualifiedAdvanced = Pointer.offset(pointer, 1)\n\
                    let qualifiedBytes = Pointer.cast[Byte](qualifiedAdvanced)\n\
                    let qualifiedPointer = UInt64.toPointer[Int](1u64)\n\
                    _ = qualifiedPointer\n\
                    _ = Pointer.address(qualifiedBytes)\n\
                }\n\
            }\n\
        }\n";
        let output = execute(operation_request(
            Operation::Run,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
    }

    #[test]
    fn unbounded_generics_infer_invariant_arguments_and_execute() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn identity[T](value: T): T { value }\n\
              fn main() {\n\
                  let inferred: Int = identity(42)\n\
                  let expected: String = identity(\"Tondo\")\n\
                  let explicit = identity[Bool](true)\n\
                  assert(inferred == 42)\n\
                  assert(expected == \"Tondo\")\n\
                  assert(explicit)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn trait_defaults_cross_the_public_pipeline_without_becoming_runtime_roots() {
        let output = execute(operation_request(
            Operation::Run,
            b"trait Empty[T: Discard] {\n\
                  fn length(self): Int\n\
                  fn isEmpty(self): Bool { self.length() == 0 }\n\
                  fn identity[U](self, value: U): U { value }\n\
              }\n\
              fn main() {\n\
                  assert(true)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());

        let invalid = execute(operation_request(
            Operation::Check,
            b"trait Invalid {\n\
                  fn value(self): Int { \"wrong\" }\n\
              }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(invalid.status(), CompilationStatus::Rejected);
        assert_eq!(invalid.diagnostics().diagnostics()[0].code(), "E1102");
    }

    #[test]
    fn exact_implementations_cross_the_public_pipeline_and_reject_drift() {
        let valid = execute(operation_request(
            Operation::Run,
            b"trait Value {\n\
                  fn value(self): Int\n\
                  fn valid(self): Bool { true }\n\
              }\n\
              type Item = Int\n\
              impl Value for Item {\n\
                  fn value(self): Int { 7 }\n\
              }\n\
              fn main() {\n\
                  assert(true)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            valid.status(),
            CompilationStatus::Success,
            "{:#?}",
            valid.diagnostics().diagnostics()
        );
        assert_eq!(valid.exit_code(), 0);

        let missing = execute(operation_request(
            Operation::Check,
            b"trait Value {\n\
                  fn value(self): Int\n\
              }\n\
              type Item = Int\n\
              impl Value for Item {\n\
              }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(missing.status(), CompilationStatus::Rejected);
        assert_eq!(missing.diagnostics().diagnostics()[0].code(), "E1114");

        let invalid_body = execute(operation_request(
            Operation::Check,
            b"trait Value {\n\
                  fn value(self): Int\n\
              }\n\
              type Item = Int\n\
              impl Value for Item {\n\
                  fn value(self): Int { \"wrong\" }\n\
              }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(invalid_body.status(), CompilationStatus::Rejected);
        assert_eq!(invalid_body.diagnostics().diagnostics()[0].code(), "E1102");
    }

    #[test]
    fn implementation_coherence_uses_public_diagnostics_before_constraints() {
        let overlap = execute(operation_request(
            Operation::Check,
            b"trait Marker {}\n\
              type Box[T] = { value: T }\n\
              impl[T] Marker for Box[T] {}\n\
              impl[U] Marker for Box[Array[U]] {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(overlap.status(), CompilationStatus::Rejected);
        assert_eq!(overlap.diagnostics().diagnostics().len(), 1);
        assert_eq!(overlap.diagnostics().diagnostics()[0].code(), "E1111");
        assert!(
            overlap
                .diagnostics()
                .json_lines()
                .unwrap()
                .contains("earlier overlapping implementation")
        );

        let iterator = execute(operation_request(
            Operation::Check,
            b"type Cursor = { value: Int }\n\
              impl Iterator[Int] for Cursor {\n\
                  fn next(mut self): Int? { none }\n\
              }\n\
              impl Iterator[String] for Cursor {\n\
                  fn next(mut self): String? { none }\n\
              }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(iterator.status(), CompilationStatus::Rejected);
        assert_eq!(iterator.diagnostics().diagnostics().len(), 1);
        assert_eq!(iterator.diagnostics().diagnostics()[0].code(), "E1113");
        assert!(
            iterator
                .diagnostics()
                .json_lines()
                .unwrap()
                .contains("earlier Iterator implementation")
        );
    }

    #[test]
    fn trait_termination_reports_witnesses_and_obeys_the_public_budget() {
        let cycle = execute(operation_request(
            Operation::Check,
            b"trait Left {}\n\
              trait Right {}\n\
              impl[T: Right] Left for T {}\n\
              impl[T: Left] Right for T {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(cycle.status(), CompilationStatus::Rejected);
        assert_eq!(cycle.diagnostics().diagnostics().len(), 1);
        assert_eq!(cycle.diagnostics().diagnostics()[0].code(), "E1112");
        let json = cycle.diagnostics().json_lines().unwrap();
        assert!(json.contains("idempotent size-change matrix"));
        assert!(json.contains("[[=]]"));
        assert!(json.contains("cycle obligation introduced here"));

        let limited = execute(operation_request(
            Operation::Check,
            b"trait Summary {}\n\
              trait Render {}\n\
              impl[T: Summary] Render for T {}\n",
            SourceForm::Module,
            ResourceLimits {
                max_trait_obligations: 0,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();
        assert_eq!(limited.status(), CompilationStatus::Rejected);
        assert_eq!(limited.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            limited.diagnostics().diagnostics()[0]
                .message()
                .contains("trait obligation")
        );
    }

    #[test]
    fn generic_constraint_obligations_execute_and_obey_the_request_budget() {
        let source = b"fn consume[T: Discard](value: T) {\n\
                           _ = value\n\
                       }\n\
                       fn main() {\n\
                           consume(42)\n\
                       }\n";
        let output = execute(operation_request(
            Operation::Run,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);

        let limited = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits {
                max_trait_obligations: 0,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();
        assert_eq!(limited.status(), CompilationStatus::Rejected);
        assert_eq!(limited.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            limited.diagnostics().diagnostics()[0]
                .message()
                .contains("trait obligation")
        );

        let expanding = execute(operation_request(
            Operation::Run,
            b"fn expand[T: Discard](value: T) {\n\
                  let wrapped = some(value)\n\
                  expand(wrapped)\n\
              }\n\
              fn main() {\n\
                  expand(1)\n\
              }\n",
            SourceForm::Script,
            ResourceLimits {
                max_generic_instantiations: 3,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();
        assert_eq!(expanding.status(), CompilationStatus::Rejected);
        assert_eq!(expanding.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            expanding.diagnostics().diagnostics()[0]
                .message()
                .contains("generic instantiations")
        );
    }

    #[test]
    fn hosted_main_validation_reports_missing_invalid_and_duplicate_entries() {
        let missing = execute(operation_request(
            Operation::Run,
            b"fn helper() {}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(missing.status(), CompilationStatus::Rejected);
        assert_eq!(missing.diagnostics().diagnostics()[0].code(), "E1806");

        for source in [
            &b"pub fn main() {}\n"[..],
            &b"fn main(value: Int) {}\n"[..],
            &b"fn main[T]() {}\n"[..],
            &b"fn main(): Int { 1 }\n"[..],
            &b"unsafe fn main() {}\n"[..],
            &b"fn main(): !Join[Int, Never] { panic(\"unreachable\") }\n"[..],
        ] {
            let output = execute(operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected, "{source:?}");
            assert_eq!(
                output.diagnostics().diagnostics()[0].code(),
                "E1803",
                "{source:?}"
            );
            if source
                .windows(b"Join".len())
                .any(|window| window == b"Join")
            {
                assert!(
                    output.diagnostics().diagnostics()[0]
                        .message()
                        .contains("Discard")
                );
            }
        }

        let duplicate = execute(operation_request(
            Operation::Run,
            b"fn main() {}\nlet value = 1\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(duplicate.status(), CompilationStatus::Rejected);
        assert_eq!(duplicate.diagnostics().diagnostics()[0].code(), "E1802");
    }

    #[test]
    fn async_main_executes_in_the_runtime_root_scope() {
        let output = execute(operation_request(
            Operation::Run,
            b"async fn main() {}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn fallible_main_maps_success_and_unhandled_error_to_hosted_exit_status() {
        let success = execute(operation_request(
            Operation::Run,
            b"enum AppError { Failed }\nfn main(): !AppError { () }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(success.status(), CompilationStatus::Success);
        assert_eq!(success.exit_code(), 0);

        let failure = execute(operation_request(
            Operation::Run,
            b"enum AppError { Failed }\nfn main(): !AppError {\n    fail AppError.Failed\n}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(failure.status(), CompilationStatus::Rejected);
        assert_eq!(failure.exit_code(), 1);
        assert_eq!(failure.diagnostics().diagnostics()[0].code(), "R0001");
        assert!(
            failure.diagnostics().diagnostics()[0]
                .message()
                .contains("AppError")
        );
    }

    #[test]
    fn root_panic_has_normative_diagnostic_and_distinct_exit_status() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn inner(): Never { panic(\"boom\") }\nfn main() { inner() }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 101);
        let diagnostic = &output.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "P0008");
        assert!(diagnostic.message().contains("explicit-panic"));
        assert!(
            output
                .diagnostics()
                .json_lines()
                .unwrap()
                .contains("called from")
        );
    }

    #[test]
    fn cleanup_panics_are_reported_as_suppressed_without_replacing_the_primary() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn first() {\n    panic(\"first\")\n}\n\
              fn second() {\n    panic(\"second\")\n}\n\
              fn main() {\n\
                  defer first()\n\
                  defer second()\n\
                  panic(\"primary\")\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 101);
        let diagnostic = &output.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "P0008");
        assert!(diagnostic.message().ends_with("primary"));
        let json = output.diagnostics().json_lines().unwrap();
        assert!(json.contains("suppressed explicit-panic: second"));
        assert!(json.contains("suppressed explicit-panic: first"));
    }

    #[test]
    fn structured_child_panics_use_creation_order_after_sibling_cleanup() {
        let output = execute(operation_request(
            Operation::Run,
            b"async fn tick() {}\n\
              async fn first() {\n\
                  await tick()\n\
                  await tick()\n\
                  await tick()\n\
                  panic(\"first child\")\n\
              }\n\
              async fn second() {\n\
                  await tick()\n\
                  await tick()\n\
                  await tick()\n\
                  panic(\"second child\")\n\
              }\n\
              async fn main() {\n\
                  scope {\n\
                      let firstJob = spawn first()\n\
                      let secondJob = spawn second()\n\
                      let _ = await firstJob\n\
                      let _ = await secondJob\n\
                  }\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 101);
        let diagnostic = &output.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "P0008");
        assert!(diagnostic.message().ends_with("first child"));
        assert!(
            output
                .diagnostics()
                .json_lines()
                .unwrap()
                .contains("suppressed explicit-panic: second child")
        );
    }

    #[test]
    fn g2_002_hello_world_is_captured_as_exact_program_stdout() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.console\n\nfn main() {\n    console.print(\"Hello, world\")\n}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
        assert_eq!(output.stdout(), b"Hello, world");
    }

    #[test]
    fn vm_execution_budget_is_a_resource_diagnostic() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn main() {\n    for {}\n}\n",
            SourceForm::Script,
            ResourceLimits {
                max_vm_steps: 8,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("VM")
        );
    }

    #[test]
    fn imported_sources_are_always_parsed_as_modules() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:driver-test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"let root_value = 1\n"[..]),
            ))
            .unwrap();
        sources
            .add(SourceInput::virtual_file(
                SourceId::new("module:dependency").unwrap(),
                ModulePath::new("dependency").unwrap(),
                LogicalPath::new("dependency.to").unwrap(),
                Arc::<[u8]>::from(&b"let dependency_value = 2\n"[..]),
            ))
            .unwrap();
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BTreeSet::new(),
            DiagnosticFormat::Json,
            SourceForm::Script,
            ResourceLimits::default(),
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap();

        let output = execute(request).unwrap();
        assert_eq!(output.diagnostics().diagnostics().len(), 1);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1801");
    }

    #[test]
    fn g2_005_multimodule_program_executes_with_visibility_and_nominal_identity() {
        let api = b"pub type Answer = {\n    value: Int\n    priv secret: Int\n}\n\
                    pub fn answer(): Answer { Answer { value: 42, secret: 7 } }\n\
                    pub fn value(input: Answer): Int { input.value }\n";
        let output = execute(multimodule_request(
            Operation::Run,
            b"import app.api\n\
              fn main() { assert(api.value(api.answer()) == 42) }\n",
            api,
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
        assert!(output.semantic_model().is_some());

        let nominal_mismatch = execute(multimodule_request(
            Operation::Check,
            b"import app.api\n\
              type Answer = { value: Int, secret: Int }\n\
              fn main() { api.value(Answer { value: 42, secret: 7 }) }\n",
            api,
        ))
        .unwrap();
        assert_eq!(nominal_mismatch.status(), CompilationStatus::Rejected);
        assert_eq!(
            nominal_mismatch.diagnostics().diagnostics()[0].code(),
            "E1102"
        );

        let private_access = execute(multimodule_request(
            Operation::Check,
            b"import app.api\n\
              fn main() { let answer = api.answer()\n    _ = answer.secret\n}\n",
            api,
        ))
        .unwrap();
        assert_eq!(private_access.status(), CompilationStatus::Rejected);
        assert_eq!(
            private_access.diagnostics().diagnostics()[0].code(),
            "E1501",
            "{:#?}",
            private_access.diagnostics().diagnostics()
        );
    }

    #[test]
    fn parser_node_budget_is_enforced_through_the_driver() {
        let limits = ResourceLimits {
            max_syntax_nodes: 1,
            ..ResourceLimits::default()
        };
        let output = execute(source_request(
            b"fn main() {}\n",
            SourceForm::Module,
            limits,
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("syntax node count")
        );
    }
}
