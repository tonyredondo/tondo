use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::bytecode::{BytecodeError, BytecodeLoweringLimits, lower_to_bytecode};
use crate::diagnostics::{
    Diagnostic, DiagnosticBag, DiagnosticCode, DiagnosticError, DiagnosticReport, PrimaryLocation,
    Related, Severity,
};
use crate::hir::{
    ExpressionCheckLimits, HirCallableId, HirDiscardStatus, HirError, HirProgram,
    TypeLoweringLimits, check_expressions, lower_types,
};
use crate::mir::{MirError, MirLoweringLimits, lower_to_mir};
pub use crate::package::Edition;
use crate::package::{PackageGraph, PackageGraphError};
use crate::resolve::{ResolveError, ResolvedProgram, SymbolKind, Visibility, resolve};
use crate::semantic::SemanticModel;
use crate::source::{FileId, SourceDatabase, SourceError, SourceId, Span, TextRange};
use crate::syntax::{
    FormatError, LexError, LexLimits, LexMode, ParseError, ParseLimits, ParseMode, Parsed,
    SyntaxKind, format_parsed, lex_with_limits, parse,
};
use crate::types::TypeError;
use crate::types::{ScalarType, TypeKind};
use tondo_vm::bytecode::BytecodeSpan;
use tondo_vm::runtime::{
    RuntimeValue, VmError, VmHost, VmLimits, VmOutcome, VmPanic, execute_with_limits,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Format,
    Check,
    Run,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Format => "fmt",
            Self::Check => "check",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProfile {
    Hosted,
}

impl HostProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceForm {
    Module,
    Script,
    Fragment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticFormat {
    Human,
    Json,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTarget {
    name: String,
    diagnostic_source_id: SourceId,
}

impl BuildTarget {
    pub fn vm_hosted() -> Self {
        Self {
            name: "tondo-vm-hosted".into(),
            diagnostic_source_id: SourceId::new("target:tondo-vm-hosted")
                .expect("the built-in target source ID is valid"),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn diagnostic_source_id(&self) -> &SourceId {
        &self.diagnostic_source_id
    }

    pub fn vm_hosted_capabilities() -> BTreeSet<CapabilityName> {
        BTreeSet::from([CapabilityName::new("console")
            .expect("console is a registered Tondo target capability")])
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
        })
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
}

#[derive(Debug)]
pub enum DriverError {
    InvalidCapability(String),
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
    if let Some(diagnostic) = resource_limit_diagnostic(&request)? {
        let mut bag = DiagnosticBag::new();
        bag.push(diagnostic);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            semantic_model: None,
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
            (LexMode::Module, ParseMode::Module)
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
        });
    }

    let hir = match lower_types(
        &request.packages,
        &request.sources,
        parsed_sources.iter().map(|(file, parsed)| (*file, parsed)),
        &resolved_program,
        TypeLoweringLimits {
            max_type_nodes: request.limits.max_type_nodes,
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
        });
    }

    let checked = match check_expressions(
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
    let (hir_program, expression_diagnostics, expression_check_complete) = checked.into_parts();
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
        });
    }

    if request.operation == Operation::Check
        && request.source_form == SourceForm::Module
        && expression_check_complete
    {
        let mut bag = DiagnosticBag::new();
        bag.extend(expression_diagnostics);
        let diagnostics = bag.resolve(request.edition.as_str(), &request.sources)?;
        drop(parsed_sources);
        return Ok(CompilationOutput {
            status: CompilationStatus::Success,
            exit_code: 0,
            diagnostics,
            stdout: Vec::new(),
            semantic_model: Some(SemanticModel::with_hir(
                request.sources,
                resolved_program,
                hir_program,
            )),
        });
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
            MainSelection::DeferredScript | MainSelection::Async => {}
            MainSelection::Sync(_) if !expression_check_complete => {}
            MainSelection::Sync(entry) => {
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
                let mut host = BootstrapHost::default();
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
                    host.stdout,
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
    Async,
    DeferredScript,
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
                parsed.cst().root_node().child_nodes().find(|node| {
                    matches!(
                        node.kind(),
                        SyntaxKind::BindingDecl
                            | SyntaxKind::Assignment
                            | SyntaxKind::ReturnStmt
                            | SyntaxKind::FailStmt
                            | SyntaxKind::BreakStmt
                            | SyntaxKind::ContinueStmt
                            | SyntaxKind::DeferStmt
                            | SyntaxKind::ForStmt
                            | SyntaxKind::ExpressionStmt
                            | SyntaxKind::TailExpression
                    )
                })
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

    let Some(symbol) = candidates.first().copied() else {
        if script_statement.is_some() {
            return Ok(MainSelection::DeferredScript);
        }
        return Ok(MainSelection::Rejected(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1806")?,
            "the hosted target has no explicit `main` and no script entry",
            PrimaryLocation::Target(request.target.diagnostic_source_id().clone()),
        )?));
    };

    if let Some(statement) = script_statement {
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
        Ok(MainSelection::Async)
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
    Ok(CompilationOutput {
        status: if exit_code == 0 {
            CompilationStatus::Success
        } else {
            CompilationStatus::Rejected
        },
        exit_code,
        diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
        stdout,
        semantic_model: Some(SemanticModel::with_hir(request.sources, resolved, hir)),
    })
}

#[derive(Default)]
struct BootstrapHost {
    stdout: Vec<u8>,
}

impl VmHost for BootstrapHost {
    fn invoke(&mut self, name: &str, arguments: &[RuntimeValue]) -> Result<RuntimeValue, VmError> {
        match (name, arguments) {
            ("std.console.print", [RuntimeValue::String(text)]) => {
                self.stdout.extend_from_slice(text.as_bytes());
                Ok(RuntimeValue::Unit)
            }
            ("std.console.print", _) => Err(VmError::Host(
                "std.console.print received an invalid bootstrap argument list".into(),
            )),
            _ => Err(VmError::UnsupportedHostCall(name.to_owned())),
        }
    }
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
        assert!(matches!(
            CapabilityName::new("made-up-capability"),
            Err(DriverError::InvalidCapability(_))
        ));
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
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E0002");
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
            (&b"let value = 1\n"[..], "E0006"),
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
        let output = execute(source_request(
            b"fn main() {\n    return\n    let unreachable = 1\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        let diagnostics = output.diagnostics().diagnostics();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code() == "W1006" && diagnostic.severity() == Severity::Warning
        }));
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
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
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
        assert_eq!(output.diagnostics().diagnostics().len(), 1);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0001");
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
    fn async_main_remains_an_honest_later_milestone() {
        let output = execute(operation_request(
            Operation::Run,
            b"async fn main() {}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0001");
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
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E0006");
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
