use std::collections::{BTreeMap, BTreeSet};
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
    ExpressionCheckLimits, HirBootstrapHostFunction, HirCallableId, HirDiscardStatus, HirError,
    HirExpressionKind, HirProgram, HirSpawnKind, TypeLoweringLimits, check_expressions_configured,
    lower_types, lower_types_extension,
};
use crate::mir::{MirError, MirLoweringLimits, MirSummary, lower_to_mir};
pub use crate::package::Edition;
use crate::package::{PackageAlias, PackageGraph, PackageGraphError, PackageId, PackageNode};
use crate::process_host::BootstrapHost;
use crate::resolve::{
    ResolveError, ResolvedProgram, SymbolKind, Visibility, is_script_statement, resolve,
    resolve_extension,
};
use crate::semantic::SemanticModel;
use crate::source::{FileId, SourceDatabase, SourceError, SourceId, Span, TextRange};
use crate::syntax::{
    FormatError, LexError, LexLimits, LexMode, ParseError, ParseLimits, ParseMode, Parsed,
    format_parsed, lex_with_limits, parse,
};
use crate::test_backend;
use crate::test_plan::TestSourceClass;
use crate::types::TypeError;
use crate::types::{ScalarType, TypeKind};
use tondo_vm::bytecode::BytecodeSpan;
use tondo_vm::runtime::{
    DiagnosticConfig, DiagnosticTrace, RuntimeValue, ValueCopyStrategy, VmError, VmLimits,
    VmOutcome, VmPanic, execute_with_limits, execute_with_limits_and_copy_strategy_and_diagnostics,
};

#[path = "meta_derive_check.rs"]
mod derive_check;

/// Dynamic diagnostic profiles requested by a tool invocation.  They are
/// deliberately a compiler/CLI concern: no source keyword or stdlib API is
/// introduced by opting into runtime observations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticProfile {
    Race,
    Leaks,
    Crash,
}

impl DiagnosticProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Race => "race",
            Self::Leaks => "leaks",
            Self::Crash => "crash",
        }
    }
}

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
                | "civil-clock"
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
        let mut supported_capabilities = Self::vm_hosted_capabilities();
        supported_capabilities.insert(
            CapabilityName::new("threads")
                .expect("threads is a registered Tondo target capability"),
        );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionMode {
    Compile,
    Run,
}

#[derive(Debug)]
pub struct CompilationRequest {
    operation: Operation,
    execution_mode: ExecutionMode,
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
    diagnostic_profiles: BTreeSet<DiagnosticProfile>,
    retain_bytecode: bool,
    test_entry: Option<String>,
    test_envelope: Option<crate::test_control::EnvelopeHandle>,
    test_temporary_root: Option<std::path::PathBuf>,
    test_participation_entries: Vec<String>,
    test_participation: Option<crate::test_backend::TestParticipation>,
    test_package_names: BTreeMap<PackageId, String>,
    test_source_classes: BTreeMap<FileId, crate::test_plan::TestSourceClass>,
    sealed_production: Option<std::sync::Arc<SemanticModel>>,
    source_derive_providers: crate::meta_provider::SourceDeriveProviders,
    source_meta: Option<std::sync::Arc<crate::project_meta::ClosedSourceMeta>>,
    meta_results: Vec<crate::meta_atomic::AcceptedMetaResult>,
    meta_descriptors: Vec<crate::meta_query::MetaQueryDescriptor>,
    derive_bound_probe: bool,
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
        let test_source_classes = sources
            .iter()
            .map(|(file, _)| {
                (
                    file,
                    if operation == Operation::Test && file == root {
                        TestSourceClass::UnitTest
                    } else {
                        TestSourceClass::Production
                    },
                )
            })
            .collect();
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
            diagnostic_profiles: BTreeSet::new(),
            execution_mode: ExecutionMode::Run,
            retain_bytecode: false,
            test_entry: None,
            test_envelope: None,
            test_temporary_root: None,
            test_participation_entries: Vec::new(),
            test_participation: None,
            test_package_names: BTreeMap::new(),
            test_source_classes,
            sealed_production: None,
            source_derive_providers: crate::meta_provider::SourceDeriveProviders::default(),
            source_meta: None,
            meta_results: Vec::new(),
            meta_descriptors: Vec::new(),
            derive_bound_probe: false,
        })
    }

    /// Enables the isolated Appendix C interfaces for the conformance doc
    /// runner. This surface is absent from ordinary compiler builds.
    #[cfg(feature = "conformance")]
    pub fn with_documentation_fixture(mut self) -> Self {
        self.documentation_fixture = true;
        self
    }

    /// Install the immutable build-only companion for an ordinary meta provider.
    /// Its package identity and bytes are owned by the selected toolchain.
    pub fn with_meta_companion(mut self) -> Result<Self, DriverError> {
        if self.target.name() != "tondo-meta" || self.profile != HostProfile::Meta {
            return Err(DriverError::Invariant(
                "std.meta requires tondo-meta/meta".into(),
            ));
        }
        let companion = crate::std_meta::StdMetaPackage::load_candidate()
            .map_err(|error| DriverError::Invariant(error.to_string()))?;
        let source_id = SourceId::new(crate::std_meta::STD_META_PACKAGE)?;
        let module = crate::source::ModulePath::new("meta")?;
        let node = crate::package::PackageNode::new(
            crate::package::PackageId::new(crate::std_meta::STD_META_PACKAGE)?,
            source_id.clone(),
            crate::package::PackageAlias::new("tondoMeta")?,
            Edition::V0_1,
            [module.clone()],
            [],
        )?;
        self.packages.install_meta_companion(node)?;
        self.sources.add(crate::source::SourceInput::virtual_file(
            source_id,
            module,
            crate::source::LogicalPath::new("src/meta.to")?,
            companion.source(),
        ))?;
        self.packages.validate_sources(&self.sources, self.root)?;
        Ok(self)
    }

    /// Supplies the argument values exposed by `std.env.snapshot()` during `run`.
    pub fn with_program_arguments(mut self, arguments: Vec<String>) -> Self {
        self.program_arguments = arguments;
        self
    }

    pub fn with_declared_build_inputs(mut self, inputs: DeclaredBuildInputs) -> Self {
        self.build_inputs = inputs;
        self
    }

    pub(crate) fn with_source_derive_providers(
        mut self,
        providers: crate::meta_provider::SourceDeriveProviders,
    ) -> Self {
        self.source_derive_providers = providers;
        self
    }

    pub(crate) fn with_source_meta(
        mut self,
        meta: Option<crate::project_meta::ClosedSourceMeta>,
    ) -> Self {
        self.source_meta = meta.map(std::sync::Arc::new);
        self
    }

    pub fn with_warning_profiles(
        mut self,
        profiles: impl IntoIterator<Item = WarningProfile>,
    ) -> Self {
        self.warning_profiles = profiles.into_iter().collect();
        self
    }

    /// Enables bounded hosted runtime observations for the selected profiles.
    /// The VM creates one collector per execution; an empty set keeps the
    /// zero-overhead execution path.
    pub fn with_diagnostic_profiles(
        mut self,
        profiles: impl IntoIterator<Item = DiagnosticProfile>,
    ) -> Self {
        self.diagnostic_profiles = profiles.into_iter().collect();
        self
    }

    /// Retains verified bytecode in a successful run output for compiler-owned
    /// differential tooling. Ordinary callers keep the zero-copy release path.
    pub fn with_bytecode_observation(mut self) -> Self {
        self.retain_bytecode = true;
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

    /// Explicit temporary-root provider for embedded test execution. The
    /// embedding runner owns cleanup; compilation never creates a root.
    pub fn with_test_temporary_root(mut self, root: std::path::PathBuf) -> Self {
        self.test_temporary_root = Some(root);
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

    pub fn runtime_limits(&self) -> VmLimits {
        vm_limits(self.limits)
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

    pub fn diagnostic_profiles(&self) -> &BTreeSet<DiagnosticProfile> {
        &self.diagnostic_profiles
    }

    pub fn test_entry(&self) -> Option<&str> {
        self.test_entry.as_deref()
    }

    fn source_class(&self, file: FileId) -> TestSourceClass {
        self.test_source_classes
            .get(&file)
            .copied()
            .unwrap_or(TestSourceClass::Production)
    }

    /// Materializes integration consumers from explicit test-plan classes.
    /// The caller has already discovered and pinned every source; this method
    /// performs no filesystem discovery or ambient configuration lookup.
    pub fn with_test_project_plan(
        mut self,
        plan: &crate::test_plan::TestProjectPlan,
    ) -> Result<Self, DriverError> {
        if self.sealed_production.is_some() {
            return Err(DriverError::Invariant(
                "test source planning must precede production sealing".into(),
            ));
        }
        let mut sources = SourceDatabase::new();
        let mut nodes = self.packages.packages().cloned().collect::<Vec<_>>();
        let mut names = BTreeMap::new();
        for (file, source) in self.sources.iter() {
            let owner = self
                .packages
                .package_for_source(source.source_id())
                .ok_or_else(|| DriverError::Invariant("test source has no package".into()))?;
            let declared = plan.sources().iter().find(|entry| {
                entry.logical_path() == source.path().as_str()
                    && entry.package() == owner.id().as_str()
            });
            let declared = declared.ok_or_else(|| {
                DriverError::Invariant(format!(
                    "closed test plan omitted source `{}` from package `{}`",
                    source.path(),
                    owner.id()
                ))
            })?;
            self.test_source_classes.insert(file, declared.class());
            let (source_id, module) = match declared {
                entry if entry.class() == crate::test_plan::TestSourceClass::IntegrationTest => {
                    let id = crate::test_integration::synthetic_package(
                        owner.id(),
                        entry.logical_path(),
                    );
                    let source_id = crate::source::SourceId::new(id.as_str())?;
                    let module = crate::source::ModulePath::new(entry.module())?;
                    let mut ordinal = names.len();
                    let local_name = loop {
                        let candidate = PackageAlias::new(format!("tondoIntegration{ordinal}"))?;
                        if nodes.iter().all(|node| node.local_name() != &candidate) {
                            break candidate;
                        }
                        ordinal += 1;
                    };
                    let tested_dependency = plan
                        .sources()
                        .iter()
                        .any(|source| {
                            source.package() == owner.id().as_str()
                                && source.class() == TestSourceClass::Production
                        })
                        .then(|| (owner.local_name().clone(), owner.id().clone()));
                    nodes.push(PackageNode::new(
                        id.clone(),
                        source_id.clone(),
                        local_name,
                        owner.edition(),
                        [module.clone()],
                        tested_dependency,
                    )?);
                    names.insert(id, owner.local_name().as_str().to_owned());
                    (source_id, module)
                }
                _ => {
                    if declared.module() != source.module().as_str() {
                        return Err(DriverError::Invariant(format!(
                            "test plan changed the supplied module for `{}`",
                            source.path()
                        )));
                    }
                    (source.source_id().clone(), source.module().clone())
                }
            };
            let actual = sources.add(
                crate::source::SourceInput::new(
                    source_id,
                    module,
                    source.path().clone(),
                    source.origin(),
                    std::sync::Arc::<[u8]>::from(source.bytes()),
                )
                .with_diagnostic_origin(source.diagnostic_origin())
                .with_diagnostic_mappings(source.diagnostic_mappings())
                .with_testing_calls(source.testing_calls()),
            )?;
            if actual != file {
                return Err(DriverError::Invariant(
                    "test planning changed file identity".into(),
                ));
            }
        }
        let root_source = sources.get(self.root)?.source_id();
        let root = nodes
            .iter()
            .find(|node| node.source_id() == root_source)
            .ok_or_else(|| DriverError::Invariant("test root has no package".into()))?
            .id()
            .clone();
        let mut packages = PackageGraph::new(root, self.packages.standard().clone(), nodes)?;
        packages.retain_generated_owners_from(&self.packages)?;
        packages.enable_bootstrap_testing()?;
        packages.validate_sources(&sources, self.root)?;
        self.sources = sources;
        self.packages = packages;
        self.test_package_names = names;
        Ok(self)
    }

    /// Admits only successful, complete production from the same compilation
    /// environment and pinned sources. Production files keep their original
    /// IDs as a prefix; tests are appended and cannot reopen that prefix.
    pub fn with_production_compilation(
        mut self,
        production: CompilationOutput,
    ) -> Result<Self, DriverError> {
        if self.operation == Operation::Format || self.sealed_production.is_some() {
            return Err(DriverError::Invariant(
                "production sealing requires an unsealed semantic compilation".into(),
            ));
        }
        if production.status != CompilationStatus::Success {
            return Err(DriverError::Invariant(
                "cannot seal rejected production".into(),
            ));
        }
        let interface = production.interface().ok_or_else(|| {
            DriverError::Invariant("production seal requires a compiled interface".into())
        })?;
        if interface.edition() != self.edition.as_str()
            || interface.target() != self.target.name()
            || interface.profile() != self.profile.as_str()
            || interface
                .capabilities()
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != self
                    .capabilities
                    .iter()
                    .map(CapabilityName::as_str)
                    .collect()
            || interface
                .features()
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != self
                    .build_inputs
                    .features()
                    .iter()
                    .map(|feature| feature.as_str())
                    .collect()
            || self
                .packages
                .package(&PackageId::new(interface.package_id())?)
                .is_none()
        {
            return Err(DriverError::Invariant(
                "production seal has a different compilation environment".into(),
            ));
        }
        let generation = interface.generation().to_vec();
        let model = production
            .into_semantic_model()
            .filter(SemanticModel::expression_check_complete)
            .ok_or_else(|| {
                DriverError::Invariant("production seal requires complete semantic checking".into())
            })?;
        let mut sources = clone_source_database(model.sources(), None)?;
        for (_, source) in model
            .sources()
            .iter()
            .filter(|(_, source)| source.origin() == crate::source::SourceOrigin::GeneratedMeta)
        {
            let owner = model
                .packages()
                .package_for_source(source.source_id())
                .ok_or_else(|| {
                    DriverError::Invariant("sealed generated source has no package".into())
                })?;
            match self.packages.package_for_source(source.source_id()) {
                Some(existing) if existing.id() != owner.id() => {
                    return Err(DriverError::Invariant(
                        "test graph changed a generated source owner".into(),
                    ));
                }
                Some(_) => {}
                None => {
                    self.packages.register_generated_source(
                        owner.id(),
                        source.source_id().clone(),
                        source.module().clone(),
                    )?;
                }
            }
        }
        for production_package in model.packages().packages() {
            let compatible =
                self.packages
                    .package(production_package.id())
                    .is_some_and(|package| {
                        package.source_id() == production_package.source_id()
                            && package.local_name() == production_package.local_name()
                            && package.edition() == production_package.edition()
                            && production_package.modules().is_subset(package.modules())
                            && production_package.dependencies().iter().all(
                                |(alias, dependency)| {
                                    package.dependencies().get(alias) == Some(dependency)
                                },
                            )
                    });
            if !compatible {
                return Err(DriverError::Invariant(format!(
                    "test graph changed sealed production package `{}`",
                    production_package.id()
                )));
            }
        }
        let mut mapping = BTreeMap::new();
        let mut appended = Vec::new();
        let mut classes = sources
            .iter()
            .map(|(file, _)| (file, TestSourceClass::Production))
            .collect::<BTreeMap<_, _>>();
        for (file, source) in self.sources.iter() {
            let existing = sources.iter().find(|(_, candidate)| {
                source.source_id() == candidate.source_id()
                    && source.module() == candidate.module()
                    && source.path() == candidate.path()
            });
            if let Some((actual, candidate)) = existing {
                if self.source_class(file) != TestSourceClass::Production
                    || source.bytes() != candidate.bytes()
                    || source.origin() != candidate.origin()
                {
                    return Err(DriverError::Invariant(format!(
                        "test graph changed sealed production source `{}`",
                        source.path()
                    )));
                }
                mapping.insert(file, actual);
            } else {
                if self.source_class(file) == TestSourceClass::Production {
                    return Err(DriverError::Invariant(format!(
                        "production seal omitted source `{}`",
                        source.path()
                    )));
                }
                let actual = FileId::from_index(sources.len() + appended.len())?;
                mapping.insert(file, actual);
                classes.insert(actual, self.source_class(file));
                appended.push((file, source));
            }
        }
        for (file, source) in model.sources().iter() {
            if !matches!(
                source.origin(),
                crate::source::SourceOrigin::GeneratedStandard
                    | crate::source::SourceOrigin::GeneratedMeta
            ) && !mapping.values().any(|mapped| *mapped == file)
            {
                return Err(DriverError::Invariant(format!(
                    "test graph omitted sealed production source `{}`",
                    source.path()
                )));
            }
        }
        for (file, source) in appended {
            let remap_span = |span: crate::source::Span| {
                mapping
                    .get(&span.file())
                    .copied()
                    .map(|file| span.with_file(file))
                    .ok_or_else(|| {
                        DriverError::Invariant(
                            "test source diagnostic origin is outside the pinned graph".into(),
                        )
                    })
            };
            let origin = source.diagnostic_origin().map(remap_span).transpose()?;
            let diagnostic_mappings = source
                .diagnostic_mappings()
                .iter()
                .map(|entry| {
                    Ok(crate::source::SourceDiagnosticMapping {
                        generated: entry.generated,
                        origin: remap_span(entry.origin)?,
                    })
                })
                .collect::<Result<Vec<_>, DriverError>>()?;
            let actual = sources.add(
                crate::source::SourceInput::new(
                    source.source_id().clone(),
                    source.module().clone(),
                    source.path().clone(),
                    source.origin(),
                    std::sync::Arc::<[u8]>::from(source.bytes()),
                )
                .with_diagnostic_origin(origin)
                .with_diagnostic_mappings(&diagnostic_mappings)
                .with_testing_calls(source.testing_calls()),
            )?;
            if actual != mapping[&file] {
                return Err(DriverError::Invariant(
                    "test source remapping changed file order".into(),
                ));
            }
        }
        self.root = mapping[&self.root];
        self.packages.validate_sources(&sources, self.root)?;
        self.sources = sources;
        self.test_source_classes = classes;
        self.build_inputs = self.build_inputs.with_generation(generation)?;
        // Production producers have already completed atomically. Workers
        // consume the sealed sources; none can start another generation round.
        self.source_meta = None;
        self.meta_results = model.meta_results().to_vec();
        self.meta_descriptors = model.meta_descriptors().to_vec();
        self.sealed_production = Some(std::sync::Arc::new(model));
        Ok(self)
    }

    /// Attach an admitted dependency set only to test consumers. Production
    /// has already been checked and sealed; its resolved imports and bodies
    /// are preserved. The new sources are ordinary dependency declarations,
    /// not test overlays, and keep their own package visibility.
    pub fn with_test_dependencies(
        mut self,
        dependencies: &crate::test_dependencies::TestDependencySources,
        supplied: &BTreeMap<String, std::sync::Arc<[u8]>>,
    ) -> Result<Self, DriverError> {
        if self.sealed_production.is_none()
            && self
                .test_source_classes
                .values()
                .any(|class| *class == TestSourceClass::Production)
        {
            return Err(DriverError::Invariant(
                "test dependencies require sealed production".into(),
            ));
        }
        let Some(project) = dependencies.project() else {
            return Ok(self);
        };
        let inputs = dependencies
            .required_inputs()
            .map(|input| {
                supplied
                    .get(input.path())
                    .cloned()
                    .map(|bytes| (input.path().to_owned(), bytes))
                    .ok_or_else(|| {
                        DriverError::Invariant(format!(
                            "missing test dependency input `{}`",
                            input.path()
                        ))
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let dependency = project
            .resolve(&inputs)
            .map_err(|error| DriverError::TestDependency(error.to_string()))?
            .into_compilation_request(Operation::Check, self.diagnostic_format, self.limits)
            .map_err(|error| DriverError::TestDependency(error.to_string()))?;
        let mut consumers = self
            .test_source_classes
            .iter()
            .filter_map(|(file, class)| {
                (*class != TestSourceClass::Production)
                    .then(|| {
                        self.packages
                            .package_for_source(self.sources.get(*file).unwrap().source_id())
                            .map(|package| package.id().clone())
                    })
                    .flatten()
            })
            .collect::<BTreeSet<_>>();
        // The unit-overlay compilation validates the complete dependency set
        // even when selection is empty or there are only integration roots.
        consumers.insert(self.packages.root().clone());
        let mut nodes = Vec::new();
        for package in self.packages.packages() {
            if dependency.packages.package(package.id()).is_some()
                && package.id() != self.packages.standard()
            {
                return Err(DriverError::TestDependency(format!(
                    "test dependency overlaps package `{}`",
                    package.id()
                )));
            }
            let mut edges = package.dependencies().clone();
            if consumers.contains(package.id()) {
                for (alias, target) in dependencies.graph().aliases() {
                    let alias = PackageAlias::new(alias)?;
                    if &alias == package.local_name()
                        || edges.insert(alias.clone(), target.clone()).is_some()
                    {
                        return Err(DriverError::TestDependency(format!(
                            "test dependency alias `{alias}` conflicts with an existing dependency"
                        )));
                    }
                }
            }
            nodes.push(PackageNode::new(
                package.id().clone(),
                package.source_id().clone(),
                package.local_name().clone(),
                package.edition(),
                package.modules().clone(),
                edges,
            )?);
        }
        nodes.extend(
            dependency
                .packages
                .packages()
                .filter(|node| node.id() != self.packages.standard())
                .cloned(),
        );
        for (_, source) in dependency.sources.iter() {
            self.sources.add(crate::source::SourceInput::new(
                source.source_id().clone(),
                source.module().clone(),
                source.path().clone(),
                source.origin(),
                std::sync::Arc::<[u8]>::from(source.bytes()),
            ))?;
        }
        let mut packages = PackageGraph::new(
            self.packages.root().clone(),
            self.packages.standard().clone(),
            nodes,
        )?;
        packages.retain_generated_owners_from(&self.packages)?;
        packages.enable_bootstrap_testing()?;
        packages.validate_sources(&self.sources, self.root)?;
        let mut interfaces = self.build_inputs.dependency_interfaces().clone();
        interfaces.extend(dependency.build_inputs.dependency_interfaces().clone());
        let source_sets = self
            .build_inputs
            .source_sets()
            .union(dependency.build_inputs.source_sets())
            .cloned()
            .collect();
        let mut build_inputs =
            DeclaredBuildInputs::new(self.build_inputs.features().clone(), source_sets)
                .with_generator_inputs(self.build_inputs.generator_inputs().clone())?
                .with_generation(self.build_inputs.generation().to_vec())?
                .with_dependency_interfaces(interfaces, true);
        for (kind, original, extra) in [
            (
                "manifest",
                self.build_inputs.manifest_hash(),
                project.manifest_hash(),
            ),
            (
                "lockfile",
                self.build_inputs.lockfile_hash(),
                project.lockfile_hash(),
            ),
        ] {
            let hash = crate::artifact::sha256(
                format!("test-{kind}:{}:{extra}", original.unwrap_or("")).as_bytes(),
            );
            build_inputs = if kind == "manifest" {
                build_inputs.with_manifest_hash(hash)?
            } else {
                build_inputs.with_lockfile_hash(hash)?
            };
        }
        self.packages = packages;
        self.build_inputs = build_inputs;
        Ok(self)
    }

    /// Produces independent compilation requests for the unit overlay and
    /// each integration root. Other test consumers are never dependencies.
    pub fn test_compilation_requests(&self) -> Result<Vec<Self>, DriverError> {
        let roots = std::iter::once(self.root)
            .chain(self.test_source_classes.iter().filter_map(|(file, class)| {
                (*class == crate::test_plan::TestSourceClass::IntegrationTest).then_some(*file)
            }))
            .collect::<BTreeSet<_>>();
        roots
            .into_iter()
            .map(|root| self.for_test_source(root, Operation::Check))
            .collect()
    }

    fn for_test_source(&self, root: FileId, operation: Operation) -> Result<Self, DriverError> {
        let owner = self
            .packages
            .module_for_file(&self.sources, root)?
            .package()
            .clone();
        let integration = self.test_package_names.contains_key(&owner);
        let mut active = BTreeSet::from([owner.clone(), self.packages.standard().clone()]);
        let mut pending = vec![owner.clone()];
        while let Some(package) = pending.pop() {
            for dependency in self
                .packages
                .package(&package)
                .ok_or_else(|| DriverError::Invariant("test package is missing".into()))?
                .dependencies()
                .values()
            {
                if active.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
        let mut sources = SourceDatabase::new();
        let mut classes = BTreeMap::new();
        let mut selected_root = None;
        let mut source_mapping = BTreeMap::new();
        for (file, source) in self.sources.iter() {
            let package = self
                .packages
                .package_for_source(source.source_id())
                .ok_or_else(|| DriverError::Invariant("test source has no package".into()))?;
            let class = self
                .test_source_classes
                .get(&file)
                .copied()
                .unwrap_or(crate::test_plan::TestSourceClass::Production);
            if !active.contains(package.id())
                || (integration
                    && file != root
                    && class != crate::test_plan::TestSourceClass::Production)
            {
                if self
                    .sealed_production
                    .as_ref()
                    .is_some_and(|model| (file.index() as usize) < model.sources().len())
                {
                    return Err(DriverError::Invariant(
                        "test consumer cannot discard sealed production files".into(),
                    ));
                }
                continue;
            }
            let remap_span = |span: crate::source::Span| {
                source_mapping
                    .get(&span.file())
                    .copied()
                    .map(|file| span.with_file(file))
                    .ok_or_else(|| {
                        DriverError::Invariant("test consumer omitted a diagnostic origin".into())
                    })
            };
            let origin = source.diagnostic_origin().map(remap_span).transpose()?;
            let diagnostic_mappings = source
                .diagnostic_mappings()
                .iter()
                .map(|entry| {
                    Ok(crate::source::SourceDiagnosticMapping {
                        generated: entry.generated,
                        origin: remap_span(entry.origin)?,
                    })
                })
                .collect::<Result<Vec<_>, DriverError>>()?;
            let actual = sources.add(
                crate::source::SourceInput::new(
                    source.source_id().clone(),
                    source.module().clone(),
                    source.path().clone(),
                    source.origin(),
                    std::sync::Arc::<[u8]>::from(source.bytes()),
                )
                .with_diagnostic_origin(origin)
                .with_diagnostic_mappings(&diagnostic_mappings)
                .with_testing_calls(source.testing_calls()),
            )?;
            classes.insert(actual, class);
            source_mapping.insert(file, actual);
            if file == root {
                selected_root = Some(actual);
            }
        }
        let root = selected_root
            .ok_or_else(|| DriverError::Invariant("test root was filtered out".into()))?;
        let nodes = self
            .packages
            .packages()
            .filter(|package| active.contains(package.id()))
            .map(|package| {
                let present = sources
                    .iter()
                    .filter(|(_, source)| {
                        self.packages
                            .package_for_source(source.source_id())
                            .is_some_and(|owner| owner.id() == package.id())
                    })
                    .map(|(_, source)| source.module().clone())
                    .collect::<BTreeSet<_>>();
                let modules = if package.id() == self.packages.standard() || present.is_empty() {
                    package.modules().clone()
                } else {
                    present
                };
                let dependencies = if integration && package.id() != &owner {
                    self.sealed_production
                        .as_ref()
                        .and_then(|model| model.packages().package(package.id()))
                        .map_or_else(|| package.dependencies(), PackageNode::dependencies)
                } else {
                    package.dependencies()
                };
                PackageNode::new(
                    package.id().clone(),
                    package.source_id().clone(),
                    package.local_name().clone(),
                    package.edition(),
                    modules,
                    dependencies
                        .iter()
                        .map(|(alias, package)| (alias.clone(), package.clone())),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut packages =
            PackageGraph::new(owner.clone(), self.packages.standard().clone(), nodes)?;
        packages.retain_generated_owners_from(&self.packages)?;
        packages.enable_bootstrap_testing()?;
        let interfaces = self
            .build_inputs
            .dependency_interfaces()
            .iter()
            .filter(|(package, _)| active.contains(*package) && *package != &owner)
            .map(|(package, interface)| (package.clone(), interface.clone()))
            .collect();
        let inputs = self.build_inputs.clone().with_dependency_interfaces(
            interfaces,
            self.build_inputs.require_dependency_interfaces(),
        );
        let mut request = Self::new(
            operation,
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
        .with_declared_build_inputs(inputs)
        .with_warning_profiles(self.warning_profiles.clone())
        .with_diagnostic_profiles(self.diagnostic_profiles.clone());
        request.test_package_names = self
            .test_package_names
            .iter()
            .filter(|(package, _)| active.contains(*package))
            .map(|(package, name)| (package.clone(), name.clone()))
            .collect();
        request.test_source_classes = classes;
        request.sealed_production = self.sealed_production.clone();
        request.source_derive_providers = self.source_derive_providers.clone();
        request.source_meta = self.source_meta.clone();
        request.meta_results = self.meta_results.clone();
        request.meta_descriptors = self.meta_descriptors.clone();
        Ok(request)
    }

    /// Creates a worker request with only its closed consumer dependency graph.
    pub fn for_test_entry(&self, entry: &test_backend::TestEntry) -> Result<Self, DriverError> {
        Ok(self
            .for_test_source(entry.file(), Operation::Test)?
            .with_test_entry(entry.id().to_owned()))
    }

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
        Ok(self
            .for_test_source(first.file(), Operation::Test)?
            .with_test_participation(
                entries.iter().map(|entry| entry.id().to_owned()),
                participation,
            ))
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
    diagnostic_trace: Option<DiagnosticTrace>,
    mir_summary: Option<MirSummary>,
    bytecode: Option<(
        tondo_vm::bytecode::BytecodeProgram,
        tondo_vm::bytecode::BytecodeFunctionId,
    )>,
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

    /// Returns the bounded runtime trace produced by an opted-in diagnostic
    /// execution.  Normal checks/runs leave this absent.
    pub fn diagnostic_trace(&self) -> Option<&DiagnosticTrace> {
        self.diagnostic_trace.as_ref()
    }

    /// Returns the backend-neutral inventory of the verified MIR when the
    /// request reached the executable lowering boundary.  It contains no
    /// request-local IDs, addresses, layouts or source paths.
    pub fn mir_summary(&self) -> Option<&MirSummary> {
        self.mir_summary.as_ref()
    }

    /// Returns verified VM bytecode after [`compile`] or an observed execution.
    ///
    /// This is an internal toolchain observation surface used by backend
    /// differential probes; it is not a serialized artifact or a public ABI.
    pub fn bytecode(&self) -> Option<&tondo_vm::bytecode::BytecodeProgram> {
        self.bytecode.as_ref().map(|(program, _)| program)
    }

    /// Takes the verified program and the exact entry selected by the compiler.
    pub fn into_compiled_program(
        self,
    ) -> Option<(
        tondo_vm::bytecode::BytecodeProgram,
        tondo_vm::bytecode::BytecodeFunctionId,
    )> {
        self.bytecode
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
    TestDependency(String),
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
            Self::TestDependency(message) => {
                write!(formatter, "invalid test dependency: {message}")
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
    execute_with_derives(request, true)
}

/// Compiles a runnable program or test participation through MIR and bytecode
/// verification without entering the VM or performing any program host calls.
/// A successful output owns the bytecode for this invocation; it is not a
/// persistent artifact or a cross-version bytecode ABI.
pub fn compile(mut request: CompilationRequest) -> Result<CompilationOutput, DriverError> {
    if !matches!(request.operation, Operation::Run | Operation::Test) {
        return Err(DriverError::Invariant(
            "executable compilation requires a run or test request".into(),
        ));
    }
    request.execution_mode = ExecutionMode::Compile;
    request.retain_bytecode = true;
    execute(request)
}

fn execute_with_derives(
    request: CompilationRequest,
    expand_derives: bool,
) -> Result<CompilationOutput, DriverError> {
    let results = request.meta_results.clone();
    let descriptors = request.meta_descriptors.clone();
    let mut output = execute_pipeline(request, expand_derives)?;
    if output.status == CompilationStatus::Success
        && !results.is_empty()
        && let Some(model) = output.semantic_model.as_mut()
        && model.meta_results().is_empty()
    {
        model
            .set_meta_expansions(results, descriptors)
            .map_err(|error| DriverError::Invariant(error.to_string()))?;
    }
    Ok(output)
}

fn execute_pipeline(
    mut request: CompilationRequest,
    expand_derives: bool,
) -> Result<CompilationOutput, DriverError> {
    install_bootstrap_standard_sources(&mut request)?;
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
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
        if request
            .sealed_production
            .as_ref()
            .is_some_and(|model| index < model.sources().len())
        {
            continue;
        }
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: None,
            products: None,
        });
    }

    if expand_derives
        && let Some(meta) = request.source_meta.clone()
        && !meta.plan.lock.generators.is_empty()
    {
        let expansion = match crate::meta_generation::expand(
            &request,
            &parsed_sources,
            &meta,
            &request.source_derive_providers,
        ) {
            Ok(expansion) => expansion,
            Err(crate::meta_generation::GenerationError::Driver(error)) => return Err(error),
            Err(crate::meta_generation::GenerationError::Diagnostics(diagnostics)) => {
                return Ok(CompilationOutput {
                    status: CompilationStatus::Rejected,
                    exit_code: 1,
                    diagnostics,
                    stdout: Vec::new(),
                    diagnostic_trace: None,
                    mir_summary: None,
                    bytecode: None,
                    semantic_model: None,
                    products: None,
                });
            }
        };
        drop(parsed_sources);
        let mut diagnostics = expansion.diagnostics;
        diagnostics.append(crate::meta_diagnostics::render_meta_diagnostics(
            expansion.derives.diagnostics,
            request.target.diagnostic_source_id().clone(),
            &request.sources,
        )?);
        for generated in expansion.sources {
            let file = request.sources.add(generated.source)?;
            let source = request.sources.get(file)?;
            request.packages.register_generated_source(
                &generated.owner,
                source.source_id().clone(),
                source.module().clone(),
            )?;
        }
        if let Some(output) = derive_check::validate(&request, &expansion.derives.sources)? {
            return Ok(output);
        }
        admit_derive_sources(&mut request, expansion.derives.sources)?;
        request.meta_results.extend(expansion.derives.accepted);
        request
            .meta_descriptors
            .extend(expansion.derives.descriptors);
        for result in &expansion.accepted {
            request.meta_descriptors.push(
                crate::meta_query::MetaQueryDescriptor::new(
                    result.identity_hash(),
                    None::<String>,
                    Vec::<String>::new(),
                )
                .map_err(|error| DriverError::Invariant(error.to_string()))?,
            );
        }
        request.meta_results.extend(expansion.accepted);
        request.build_inputs = request.build_inputs.with_generation(
            request
                .meta_results
                .iter()
                .map(|result| result.record().clone())
                .collect(),
        )?;
        request
            .packages
            .validate_sources(&request.sources, request.root)?;
        let mut output = execute_with_derives(request, false)?;
        output.diagnostics.append(diagnostics);
        return Ok(output);
    }

    let test_diagnostics =
        match validate_test_source_contracts(&request, &parsed_sources, remaining_diagnostics) {
            Ok(diagnostics) => diagnostics,
            Err(DriverError::Resolve(ResolveError::DiagnosticLimit { file, offset })) => {
                return syntax_resource_output(&request, file, "primary diagnostic count", offset);
            }
            Err(error) => return Err(error),
        };
    if test_diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity() == Severity::Error)
    {
        let mut bag = DiagnosticBag::new();
        bag.extend(test_diagnostics);
        return Ok(CompilationOutput {
            status: CompilationStatus::Rejected,
            exit_code: 1,
            diagnostics: bag.resolve(request.edition.as_str(), &request.sources)?,
            stdout: Vec::new(),
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: None,
            products: None,
        });
    }
    remaining_diagnostics -= test_diagnostics.len();

    let parsed_input = || parsed_sources.iter().map(|(file, parsed)| (*file, parsed));
    let resolved = match if let Some(production) = &request.sealed_production {
        resolve_extension(
            &request.packages,
            &request.sources,
            parsed_input(),
            production.resolved(),
            remaining_diagnostics,
        )
    } else {
        resolve(
            &request.packages,
            &request.sources,
            parsed_input(),
            remaining_diagnostics,
        )
    } {
        Ok(resolved) => resolved,
        Err(ResolveError::DiagnosticLimit { file, offset }) => {
            return syntax_resource_output(&request, file, "primary diagnostic count", offset);
        }
        Err(error) => return Err(error.into()),
    };
    let (resolved_program, mut resolution_diagnostics) = resolved.into_parts();
    if let Some(production) = &request.sealed_production {
        for member in resolved_program
            .members()
            .skip(production.resolved().members().len())
        {
            let changes_production = match member.owner() {
                crate::resolve::MemberOwner::Type(symbol) => {
                    production.resolved().symbol(symbol).is_some()
                }
                crate::resolve::MemberOwner::Variant(member) => {
                    production.resolved().member(member).is_some()
                }
            };
            if changes_production {
                if resolution_diagnostics.len() >= remaining_diagnostics {
                    return syntax_resource_output(
                        &request,
                        member.span().file(),
                        "primary diagnostic count",
                        member.span().range().start(),
                    );
                }
                resolution_diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    DiagnosticCode::new("E2003")?,
                    "test sources cannot add members to a sealed production type",
                    PrimaryLocation::Source(member.span()),
                )?);
            }
        }
    }
    for symbol in resolved_program.symbols() {
        if symbol.visibility() == Visibility::Public
            && request.source_class(symbol.span().file()) != TestSourceClass::Production
            && !(symbol.is_synthetic()
                && symbol.identity().package() == request.packages.standard())
        {
            if resolution_diagnostics.len() >= remaining_diagnostics {
                return syntax_resource_output(
                    &request,
                    symbol.span().file(),
                    "primary diagnostic count",
                    symbol.span().range().start(),
                );
            }
            resolution_diagnostics.push(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E2003")?,
                "test sources cannot export a public declaration",
                PrimaryLocation::Source(symbol.span()),
            )?);
        }
    }
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: Some(SemanticModel::after_resolution(
                request.packages,
                request.sources,
                resolved_program,
            )),
            products: None,
        });
    }

    let type_limits = TypeLoweringLimits {
        max_type_nodes: request.limits.max_type_nodes,
        max_trait_obligations: request.limits.max_trait_obligations,
        max_diagnostics: remaining_diagnostics,
    };
    let hir = match if let Some(production) = &request.sealed_production {
        lower_types_extension(
            &request.packages,
            &request.sources,
            parsed_input(),
            &resolved_program,
            production
                .hir()
                .expect("admitted production has complete HIR"),
            type_limits,
        )
    } else {
        lower_types(
            &request.packages,
            &request.sources,
            parsed_input(),
            &resolved_program,
            type_limits,
        )
    } {
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: Some(SemanticModel::with_hir(
                request.packages,
                request.sources,
                resolved_program,
                hir_program,
            )),
            products: None,
        });
    }

    if request.target.name() == "tondo-meta"
        && let Some(derive) = hir_program.derive_requests().first()
    {
        let span = derive.span();
        drop(parsed_sources);
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E2109")?,
            "a meta provider cannot contain a derive request",
            PrimaryLocation::Source(span),
        )?;
        return semantic_output(
            request,
            resolved_program,
            hir_program,
            Vec::new(),
            Some(diagnostic),
            1,
            Vec::new(),
        );
    }

    if expand_derives
        && hir_program.derive_requests().iter().any(|derive| {
            parsed_sources
                .iter()
                .any(|(file, _)| *file == derive.span().file())
        })
    {
        let limits = crate::meta::MetaLimits::new(
            request.limits.max_vm_steps.max(1),
            request.limits.max_vm_heap_bytes.max(1),
            u64::from(request.limits.max_source_bytes.max(1)),
        )
        .map_err(|error| DriverError::Invariant(error.to_string()))?;
        match crate::meta_frontend::expand_derives(
            &request.packages,
            &request.sources,
            &parsed_sources,
            &resolved_program,
            &hir_program,
            crate::meta_frontend::DeriveSettings {
                limits,
                source_providers: &request.source_derive_providers,
                environment: &crate::meta_snapshot::environment(&request),
            },
        ) {
            Err(crate::meta_frontend::DeriveFrontendError::Diagnostics(entries)) => {
                let diagnostics = crate::meta_diagnostics::render_meta_diagnostics(
                    entries,
                    request.target.diagnostic_source_id().clone(),
                    &request.sources,
                )?;
                drop(parsed_sources);
                return Ok(CompilationOutput {
                    status: CompilationStatus::Rejected,
                    exit_code: 1,
                    diagnostics,
                    stdout: Vec::new(),
                    diagnostic_trace: None,
                    mir_summary: None,
                    bytecode: None,
                    semantic_model: Some(SemanticModel::with_hir(
                        request.packages,
                        request.sources,
                        resolved_program,
                        hir_program,
                    )),
                    products: None,
                });
            }
            Err(crate::meta_frontend::DeriveFrontendError::Invariant(message)) => {
                return Err(DriverError::Invariant(message));
            }
            Ok(generated) => {
                let provider_diagnostics = crate::meta_diagnostics::render_meta_diagnostics(
                    generated.diagnostics,
                    request.target.diagnostic_source_id().clone(),
                    &request.sources,
                )?;
                drop(parsed_sources);
                drop(resolved_program);
                drop(hir_program);
                if let Some(output) = derive_check::validate(&request, &generated.sources)? {
                    return Ok(output);
                }
                admit_derive_sources(&mut request, generated.sources)?;
                request.meta_results.extend(generated.accepted);
                request.meta_descriptors.extend(generated.descriptors);
                request.build_inputs = request.build_inputs.with_generation(
                    request
                        .meta_results
                        .iter()
                        .map(|result| result.record().clone())
                        .collect(),
                )?;
                request
                    .packages
                    .validate_sources(&request.sources, request.root)?;
                let mut output = execute_with_derives(request, false)?;
                output.diagnostics.append(provider_diagnostics);
                return Ok(output);
            }
        }
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
    expression_diagnostics.extend(test_diagnostics);
    let core_warnings = request.warning_profiles.contains(&WarningProfile::Core);
    if !core_warnings {
        expression_diagnostics.retain(|diagnostic| diagnostic.severity() != Severity::Warning);
    }

    if !request
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == "threads")
    {
        if let Some(expression) = hir_program.expressions().find(|expression| {
            matches!(
                expression.kind(),
                HirExpressionKind::Spawn {
                    kind: HirSpawnKind::Thread,
                    ..
                }
            )
        }) {
            expression_diagnostics.push(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1008")?,
                "capability `threads` is missing for `spawn thread`",
                PrimaryLocation::Source(expression.span()),
            )?);
        }
        if let Some(expression) =
            hir_program
                .expressions()
                .find(|expression| match expression.kind() {
                    HirExpressionKind::BootstrapHostCall { function, .. } => {
                        *function == HirBootstrapHostFunction::ExecutorBlockingPool
                    }
                    HirExpressionKind::Call { callee, .. }
                    | HirExpressionKind::AsyncCall { callee, .. } => {
                        hir_program.expression(*callee).is_some_and(|callee| {
                            matches!(
                                callee.kind(),
                                HirExpressionKind::Function(HirCallableId::Host(
                                    HirBootstrapHostFunction::ExecutorBlockingPool
                                ))
                            )
                        })
                    }
                    _ => false,
                })
        {
            expression_diagnostics.push(Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1008")?,
                "capability `threads` is missing for `executor.blockingPool`",
                PrimaryLocation::Source(expression.span()),
            )?);
        }
    }

    if !request
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == "filesystem")
        && let Some((expression, function)) = hir_program.expressions().find_map(|expression| {
            let function = match expression.kind() {
                HirExpressionKind::Function(HirCallableId::Host(function))
                | HirExpressionKind::SpecializedFunction {
                    callable: HirCallableId::Host(function),
                    ..
                }
                | HirExpressionKind::BootstrapHostCall { function, .. } => *function,
                _ => return None,
            };
            matches!(
                function,
                HirBootstrapHostFunction::TestingTempDirectory
                    | HirBootstrapHostFunction::TestingTempDirectoryCleanup
            )
            .then_some((expression, function))
        })
    {
        // std.testing is a Core module. Its filesystem helpers still require
        // an explicit target capability, including aliases and deferred calls.
        expression_diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new("E1008")?,
            format!(
                "capability `filesystem` is missing for `{}`",
                function.name()
            ),
            PrimaryLocation::Source(expression.span()),
        )?);
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: Some(SemanticModel::with_hir(
                request.packages,
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

    if request.derive_bound_probe && !expression_check_complete {
        return backend_diagnostic_output(
            &request,
            "E2105",
            "derive bound necessity requires complete expression checking",
        );
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
                let mut mir_summary = mir.summary();
                mir_summary.backend = Some(mir.backend_program_with_debug(
                    &resolved_program,
                    &request.sources,
                    hir_program.interner(),
                ));
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
                if request.execution_mode == ExecutionMode::Compile {
                    drop(parsed_sources);
                    let mut output = semantic_output(
                        request,
                        resolved_program,
                        hir_program,
                        expression_diagnostics,
                        None,
                        0,
                        Vec::new(),
                    )?;
                    output.mir_summary = Some(mir_summary);
                    output.bytecode = Some((bytecode, function));
                    return Ok(output);
                }
                let mut host = BootstrapHost::with_max_bytes(
                    request.program_arguments.clone(),
                    request.limits.max_vm_heap_bytes,
                );
                if let Some(envelope) = request.test_envelope.clone() {
                    host.install_testing_envelope(envelope);
                }
                if let Some(root) = request.test_temporary_root.clone() {
                    host.install_testing_temporary_root(root);
                }
                if let Some(participation) = request.test_participation.clone() {
                    host.install_testing_participation(participation);
                }
                let execution = match if request.diagnostic_profiles.is_empty() {
                    execute_with_limits(&bytecode, function, &mut host, vm_limits(request.limits))
                } else {
                    execute_with_limits_and_copy_strategy_and_diagnostics(
                        &bytecode,
                        function,
                        &mut host,
                        vm_limits(request.limits),
                        ValueCopyStrategy::default(),
                        Some(DiagnosticConfig::default()),
                    )
                } {
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

                let runtime_trace = execution.diagnostics.clone();
                let (diagnostic, exit_code) = match execution.outcome {
                    VmOutcome::Interrupted => (None, 4),
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
                let retain_bytecode = request.retain_bytecode;
                let mut output = semantic_output(
                    request,
                    resolved_program,
                    hir_program,
                    expression_diagnostics,
                    diagnostic,
                    exit_code,
                    host.take_stdout(),
                )?;
                output.diagnostic_trace = runtime_trace;
                output.mir_summary = Some(mir_summary);
                if retain_bytecode {
                    output.bytecode = Some((bytecode, function));
                }
                return Ok(output);
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
        diagnostic_trace: None,
        mir_summary: None,
        bytecode: None,
        semantic_model: Some(SemanticModel::with_hir(
            request.packages,
            request.sources,
            resolved_program,
            hir_program,
        )),
        products: None,
    })
}

fn install_bootstrap_standard_sources(request: &mut CompilationRequest) -> Result<(), DriverError> {
    install_selected_standard_sources(&request.packages, &mut request.sources, request.root)
}

fn install_selected_standard_sources(
    packages: &PackageGraph,
    sources: &mut SourceDatabase,
    root: FileId,
) -> Result<(), DriverError> {
    if packages.standard().as_str() != "toolchain:std:0.1-bootstrap" {
        return Ok(());
    }
    let console_available = packages
        .module(
            packages.standard(),
            &crate::source::ModulePath::new("console")?,
        )
        .is_some();
    let filesystem_available = packages
        .module(packages.standard(), &crate::source::ModulePath::new("fs")?)
        .is_some();
    let standard_source = packages
        .package(packages.standard())
        .expect("the package graph contains its selected standard package")
        .source_id()
        .clone();
    let imports = |module: &[u8]| {
        sources
            .iter()
            .any(|(_, source)| imports_bootstrap_module(source.bytes(), module))
    };
    let io_selected = [
        b"std.io".as_slice(),
        b"std.console",
        b"std.fs",
        b"std.encoding",
        b"std.yaml",
        b"std.serialization",
        b"std.messagepack",
        b"std.protobuf",
        b"std.json",
    ]
    .iter()
    .any(|module| {
        (*module != b"std.console" || console_available)
            && (*module != b"std.fs" || filesystem_available)
            && imports(module)
    });
    let console_selected = console_available && imports(b"std.console");
    // An explicitly supplied fs source module (for example a documentation
    // fixture) owns its declarations and need not expose the hosted File API.
    let filesystem_selected = filesystem_available
        && imports(b"std.fs")
        && !sources.iter().any(|(_, source)| {
            source.source_id() == &standard_source && source.module().as_str() == "fs"
        });
    let json_selected = sources
        .iter()
        .any(|(_, source)| imports_bootstrap_json(source.bytes()));

    // Ordinary standard declarations and implementations retain their owning
    // module. Reuse only exact compiler-owned bytes from a sealed compilation.
    for (module, path, bytes, selected) in [
        (
            "io",
            "compiler/io.to",
            include_bytes!("bootstrap/io.to").as_slice(),
            io_selected,
        ),
        (
            "io",
            "compiler/console_io.to",
            include_bytes!("bootstrap/console_io.to").as_slice(),
            console_selected,
        ),
        (
            "io",
            "compiler/fs_io.to",
            include_bytes!("bootstrap/fs_io.to").as_slice(),
            filesystem_selected,
        ),
        (
            "__json_typed",
            "compiler/json_typed.to",
            include_bytes!("bootstrap/json_typed.to").as_slice(),
            json_selected,
        ),
    ] {
        let module = crate::source::ModulePath::new(module)?;
        let path = crate::source::LogicalPath::new(path)?;
        if !selected || packages.module(packages.standard(), &module).is_none() {
            continue;
        }
        if let Some((_, source)) = sources.iter().find(|(_, source)| {
            source.source_id() == &standard_source
                && source.module() == &module
                && source.path() == &path
        }) {
            if source.origin() != crate::source::SourceOrigin::GeneratedStandard
                || source.bytes() != bytes
            {
                return Err(DriverError::Invariant(format!(
                    "generated standard source `{path}` differs from the selected compiler"
                )));
            }
            continue;
        }
        sources.add(crate::source::SourceInput::new(
            standard_source.clone(),
            module,
            path,
            crate::source::SourceOrigin::GeneratedStandard,
            bytes,
        ))?;
    }
    packages.validate_sources(sources, root)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn bootstrap_parsed_for_test(
    packages: &PackageGraph,
    sources: &mut SourceDatabase,
    root: FileId,
    parsed: Parsed,
) -> BTreeMap<FileId, Parsed> {
    let mut files = BTreeMap::from([(root, parsed)]);
    // Layer fixtures can intentionally omit the standard implementation. A
    // console fixture needs its actual IoError declaration and trait sources;
    // otherwise the new public ConsoleError has an unresolved nominal payload.
    if !imports_bootstrap_module(sources.get(root).unwrap().bytes(), b"std.console") {
        return files;
    }
    install_selected_standard_sources(packages, sources, root).unwrap();
    for (file, _) in sources.iter() {
        if file == root {
            continue;
        }
        let lexed = crate::syntax::lex(sources, file, LexMode::Module).unwrap();
        assert!(lexed.diagnostics().is_empty());
        let parsed = parse(
            sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(
            parsed.diagnostics().is_empty(),
            "{:?}",
            parsed.diagnostics()
        );
        files.insert(file, parsed);
    }
    files
}

fn imports_bootstrap_json(bytes: &[u8]) -> bool {
    imports_bootstrap_module(bytes, b"std.json")
}

fn imports_bootstrap_module(bytes: &[u8], module: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        let line = line
            .strip_suffix(b"\r")
            .unwrap_or(line)
            .strip_prefix(b"\xef\xbb\xbf")
            .unwrap_or(line);
        let line = line
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map_or(&[][..], |start| &line[start..]);
        let Some(rest) = line.strip_prefix(b"import") else {
            return false;
        };
        let Some(rest) = rest
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .filter(|_| rest.first().is_some_and(u8::is_ascii_whitespace))
            .map(|start| &rest[start..])
        else {
            return false;
        };
        let Some(rest) = rest.strip_prefix(module) else {
            return false;
        };
        rest.first().is_none_or(|byte| byte.is_ascii_whitespace())
    })
}

fn admit_derive_sources(
    request: &mut CompilationRequest,
    sources: Vec<crate::meta_frontend::GeneratedDeriveSource>,
) -> Result<(), DriverError> {
    for source in sources {
        request.packages.register_generated_source(
            &source.owner,
            source.source_id.clone(),
            source.module.clone(),
        )?;
        request.sources.add(
            crate::source::SourceInput::new(
                source.source_id,
                source.module,
                crate::source::LogicalPath::new(source.path)?,
                crate::source::SourceOrigin::GeneratedMeta,
                source.bytes,
            )
            .with_diagnostic_mappings(&source.diagnostic_mappings),
        )?;
    }
    Ok(())
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
        if request.source_class(file) == TestSourceClass::Production
            || (package.id() != &root_package
                && !request.test_package_names.contains_key(package.id()))
        {
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
        let package_name = request
            .test_package_names
            .get(package.id())
            .map_or_else(|| package.local_name().as_str(), String::as_str);
        entries.extend(
            test_backend::discover(
                &request.sources,
                file,
                parsed.cst(),
                request.source_class(file),
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
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
            diagnostic_trace: None,
            mir_summary: None,
            bytecode: None,
            semantic_model: None,
            products: None,
        });
    }

    let root_module = request
        .packages
        .module_for_file(&request.sources, request.root)?;
    let package_name = request
        .test_package_names
        .get(root_module.package())
        .map(String::as_str)
        .or_else(|| {
            request
                .packages
                .package(root_module.package())
                .map(|package| package.local_name().as_str())
        })
        .unwrap_or("main");
    let lowered_result = if request.test_participation_entries.is_empty() {
        test_backend::lower_selected(
            &request.sources,
            request.root,
            parsed.cst(),
            request.source_class(request.root),
            package_name,
            request.test_entry(),
        )
        .map(|bytes| (bytes, None))
    } else {
        test_backend::lower_participation_mapped(
            &request.sources,
            request.root,
            parsed.cst(),
            request.source_class(request.root),
            package_name,
            request
                .test_participation_entries
                .iter()
                .map(String::as_str),
        )
        .map(|mut lowered| (std::mem::take(&mut lowered.bytes), Some(lowered)))
    };
    let (lowered, source_map) = match lowered_result {
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
    let lowered_bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(lowered);
    let sources = clone_source_database(
        &request.sources,
        Some(TestingSourceReplacement {
            file: request.root,
            bytes: lowered_bytes.clone(),
            calls: source_map
                .as_ref()
                .map_or(&[][..], |map| map.testing_calls.as_slice()),
        }),
    )?;
    let root = request.root;
    let test_envelope = request.test_envelope.clone();
    let test_temporary_root = request.test_temporary_root.clone();
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
    .with_warning_profiles(request.warning_profiles.clone())
    .with_diagnostic_profiles(request.diagnostic_profiles.clone());
    if let Some(envelope) = test_envelope {
        nested = nested.with_test_envelope(envelope);
    }
    nested.test_temporary_root = test_temporary_root;
    if let Some(participation) = test_participation {
        nested.test_participation = Some(participation);
    }
    nested.documentation_fixture = request.documentation_fixture;
    nested.execution_mode = request.execution_mode;
    nested.retain_bytecode = request.retain_bytecode;
    nested.test_source_classes = request.test_source_classes;
    nested.test_package_names = request.test_package_names;
    nested.sealed_production = request.sealed_production;
    nested.source_derive_providers = request.source_derive_providers;
    nested.source_meta = request.source_meta;
    nested.meta_results = request.meta_results;
    nested.meta_descriptors = request.meta_descriptors;
    let mut output = execute(nested)?;
    if let Some(source_map) = &source_map {
        output.diagnostics.remap_source(
            request.edition.as_str(),
            &request.sources,
            root,
            |range| {
                source_map
                    .original_span(
                        tondo_vm::bytecode::BytecodeSpan {
                            file: root.index(),
                            start: range.start(),
                            end: range.end(),
                        },
                        &lowered_bytes,
                    )
                    .and_then(|span| crate::source::TextRange::new(span.start, span.end).ok())
            },
            |range| source_map.copied_range(range),
        )?;
    }
    if let Some(source_map) = source_map
        && let Some((program, _)) = &mut output.bytecode
    {
        let remap = |span: &mut tondo_vm::bytecode::BytecodeSpan| {
            if span.file == root.index() {
                *span = source_map.original_span(*span, &lowered_bytes).unwrap_or(
                    tondo_vm::bytecode::BytecodeSpan {
                        file: span.file,
                        start: 0,
                        end: 0,
                    },
                );
            }
        };
        for function in &mut program.functions {
            if function.source.file != root.index() {
                continue;
            }
            remap(&mut function.source);
            for span in &mut function.spans {
                remap(span);
            }
            // Mapping can reorder or merge locations. Preserve the bytecode
            // span-table contract and rebind every reference to that table.
            let mapped = function.spans.clone();
            function.spans.sort();
            function.spans.dedup();
            let indices = function
                .spans
                .iter()
                .enumerate()
                .map(|(index, span)| (*span, tondo_vm::bytecode::BytecodeSpanId::new(index as u32)))
                .collect::<BTreeMap<_, _>>();
            let rebind = |id: &mut tondo_vm::bytecode::BytecodeSpanId| {
                *id = indices[&mapped[id.index() as usize]];
            };
            for slot in &mut function.slots {
                rebind(&mut slot.span);
            }
            for block in &mut function.blocks {
                rebind(&mut block.terminator.span);
                for instruction in &mut block.instructions {
                    rebind(&mut instruction.span);
                }
            }
        }
        tondo_vm::bytecode::verify_bytecode(program).map_err(|error| {
            DriverError::Invariant(format!(
                "test source mapping produced invalid bytecode: {error}"
            ))
        })?;
    }
    Ok(output)
}

fn validate_test_source_contracts(
    request: &CompilationRequest,
    parsed: &[(FileId, Parsed)],
    max_diagnostics: usize,
) -> Result<Vec<Diagnostic>, DriverError> {
    let mut inputs = Vec::with_capacity(parsed.len());
    let mut diagnostics = Vec::new();
    for (file, parsed) in parsed {
        let source = request.sources.get(*file)?;
        let package = request
            .packages
            .package_for_source(source.source_id())
            .ok_or_else(|| DriverError::Invariant("parsed source has no package".into()))?;
        let class = request.source_class(*file);
        inputs.push(crate::test_tree::TestSourceInput::new(
            package.id(),
            request
                .test_package_names
                .get(package.id())
                .map_or(package.local_name().as_str(), String::as_str),
            class,
            source.module(),
            source.path(),
            *file,
            parsed.cst(),
        ));
        if class == TestSourceClass::UnitTest {
            for declaration in parsed.cst().root_node().child_nodes().filter(|node| {
                matches!(
                    node.kind(),
                    crate::syntax::SyntaxKind::ImplDecl | crate::syntax::SyntaxKind::DeriveDecl
                )
            }) {
                if diagnostics.len() >= max_diagnostics {
                    return Err(ResolveError::DiagnosticLimit {
                        file: *file,
                        offset: declaration.range().start(),
                    }
                    .into());
                }
                diagnostics.push(Diagnostic::new(
                    Severity::Error,
                    DiagnosticCode::new("E2003")?,
                    "unit tests cannot add implementations to sealed production coherence",
                    PrimaryLocation::Source(request.sources.span(*file, declaration.range())?),
                )?);
            }
        }
        if class == TestSourceClass::Production {
            for import in parsed
                .cst()
                .root_node()
                .child_nodes()
                .filter(|node| node.kind() == crate::syntax::SyntaxKind::ImportDecl)
            {
                let path = import
                    .child_nodes()
                    .find(|node| node.kind() == crate::syntax::SyntaxKind::ModulePath);
                let Some(path) = path else {
                    continue;
                };
                let segments = path
                    .descendant_tokens()
                    .filter_map(|token| token.token().normalized_identifier())
                    .collect::<Vec<_>>();
                if segments == ["std", "testing"] {
                    if diagnostics.len() >= max_diagnostics {
                        return Err(ResolveError::DiagnosticLimit {
                            file: *file,
                            offset: import.range().start(),
                        }
                        .into());
                    }
                    diagnostics.push(Diagnostic::new(
                        Severity::Error,
                        DiagnosticCode::new("E2003")?,
                        "production sources cannot import the test-only module `std.testing`",
                        PrimaryLocation::Source(request.sources.span(*file, import.range())?),
                    )?);
                }
            }
        }
    }
    match crate::test_tree::build_with_diagnostic_limit(
        &request.sources,
        inputs,
        max_diagnostics - diagnostics.len(),
    ) {
        Ok(tree) => diagnostics.extend(tree.diagnostics().iter().cloned()),
        Err(crate::test_tree::TestTreeError::Diagnostics(errors)) => diagnostics.extend(errors),
        Err(crate::test_tree::TestTreeError::DiagnosticLimit { file, offset }) => {
            return Err(ResolveError::DiagnosticLimit { file, offset }.into());
        }
        Err(error) => return Err(DriverError::Invariant(error.to_string())),
    }
    Ok(diagnostics)
}

struct TestingSourceReplacement<'a> {
    file: FileId,
    bytes: std::sync::Arc<[u8]>,
    calls: &'a [TextRange],
}

fn clone_source_database(
    original: &SourceDatabase,
    replacement: Option<TestingSourceReplacement<'_>>,
) -> Result<SourceDatabase, DriverError> {
    let mut sources = SourceDatabase::new();
    for (file, source) in original.iter() {
        let bytes = replacement
            .as_ref()
            .filter(|replacement| replacement.file == file)
            .map(|replacement| replacement.bytes.clone())
            .unwrap_or_else(|| std::sync::Arc::from(source.bytes()));
        let origin = replacement
            .as_ref()
            .filter(|replacement| replacement.file == file)
            .map_or(source.origin(), |_| {
                crate::source::SourceOrigin::GeneratedTesting
            });
        let testing_calls = replacement
            .as_ref()
            .filter(|replacement| replacement.file == file)
            .map_or(source.testing_calls(), |replacement| replacement.calls);
        let actual = sources.add(
            crate::source::SourceInput::new(
                source.source_id().clone(),
                source.module().clone(),
                source.path().clone(),
                origin,
                bytes,
            )
            .with_diagnostic_origin(source.diagnostic_origin())
            .with_diagnostic_mappings(source.diagnostic_mappings())
            .with_testing_calls(testing_calls),
        )?;
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
        diagnostic_trace: None,
        mir_summary: None,
        bytecode: None,
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
    let entry_name = if request.sources.get(request.root)?.origin()
        == crate::source::SourceOrigin::GeneratedTesting
    {
        "__tondoTestEntry"
    } else {
        "main"
    };
    let candidates = resolved
        .symbols()
        .filter(|symbol| {
            symbol.kind() == SymbolKind::Function
                && symbol.name().as_str() == entry_name
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
    let products = if request.derive_bound_probe {
        None
    } else {
        Some(build_products(
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
        )?)
    };
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
        diagnostic_trace: None,
        mir_summary: None,
        bytecode: None,
        semantic_model: Some(SemanticModel::with_hir(
            request.packages,
            request.sources,
            resolved,
            hir,
        )),
        products,
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
        diagnostic_trace: None,
        mir_summary: None,
        bytecode: None,
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
    use tondo_vm::runtime::{RejectingHost, execute_with_arguments};

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

    fn unsealed_test_request(
        production: &[u8],
        companion: &[u8],
        limits: ResourceLimits,
    ) -> CompilationRequest {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:driver-test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main_test.to").unwrap(),
                Arc::<[u8]>::from(companion),
            ))
            .unwrap();
        sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:driver-test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(production),
            ))
            .unwrap();
        CompilationRequest::new(
            Operation::Test,
            Edition::V0_1,
            BuildTarget::vm_hosted(),
            HostProfile::Hosted,
            BuildTarget::vm_hosted_capabilities(),
            DiagnosticFormat::Json,
            SourceForm::Module,
            limits,
            PackageGraph::loose(&sources, root).unwrap(),
            sources,
            root,
        )
        .unwrap()
    }

    fn checked_production(bytes: &[u8]) -> CompilationOutput {
        execute(operation_request(
            Operation::Check,
            bytes,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap()
    }

    #[test]
    fn sealed_production_executes_private_helpers_without_reparsing_production() {
        let mut production = "fn secret(): Int { 42 }\n".to_owned();
        for index in 0..200 {
            production.push_str(&format!("const Constant{index} = {index}\n"));
        }
        let companion = b"test sealed { assert(secret() == 42) }\n";
        let limits = ResourceLimits {
            max_syntax_tokens: 512,
            ..ResourceLimits::default()
        };
        let unsealed = execute(unsealed_test_request(
            production.as_bytes(),
            companion,
            limits,
        ))
        .unwrap();
        assert_eq!(unsealed.status(), CompilationStatus::Rejected);
        assert!(unsealed.diagnostics().human().contains("T0002"));
        let checked = checked_production(production.as_bytes());
        assert_eq!(
            checked.status(),
            CompilationStatus::Success,
            "{}",
            checked.diagnostics().human()
        );
        let before = format!("{:?}", checked.semantic_model().unwrap());
        let request = unsealed_test_request(production.as_bytes(), companion, limits)
            .with_production_compilation(checked)
            .unwrap();
        assert_eq!(request.root().index(), 1);
        let sealed = request.sealed_production.clone().unwrap();
        let executed = execute(request).unwrap();
        assert_eq!(
            executed.status(),
            CompilationStatus::Success,
            "{}",
            executed.diagnostics().human()
        );
        assert_eq!(format!("{sealed:?}"), before);
        let old = sealed.hir().unwrap();
        let current = executed.semantic_model().unwrap().hir().unwrap();
        for callable in old.callables() {
            assert_eq!(
                format!("{callable:?}"),
                format!("{:?}", current.callable(callable.id()).unwrap())
            );
            if let Some(body) = old.body(callable.id()) {
                assert_eq!(body.root(), current.body(callable.id()).unwrap().root());
            }
        }
    }

    #[test]
    fn sealed_production_reuses_generated_standard_sources_and_empty_extensions() {
        let production = b"import std.json\nfn secret(): Int { 42 }\n";
        let checked = checked_production(production);
        assert_eq!(
            checked.status(),
            CompilationStatus::Success,
            "{}",
            checked.diagnostics().human()
        );
        let expected_files = checked.semantic_model().unwrap().sources().len();
        assert!(expected_files > 1);
        let request = unsealed_test_request(
            production,
            b"import std.console\ntest sealed { _ = console.print(\"\")\n assert(secret() == 42)\n }\n",
            ResourceLimits::default(),
        )
        .with_production_compilation(checked)
        .unwrap();
        assert_eq!(request.root().index() as usize, expected_files);
        let entries = discover_tests(&request).unwrap();
        let output = execute(request.for_test_entry(&entries[0]).unwrap()).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let checked = checked_production(b"fn secret(): Int { 42 }\n");
        let request = operation_request(
            Operation::Check,
            b"fn secret(): Int { 42 }\n",
            SourceForm::Module,
            ResourceLimits::default(),
        )
        .with_production_compilation(checked)
        .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert!(output.semantic_model().unwrap().expression_check_complete());
    }

    #[test]
    fn sealed_production_rejects_invalid_proof_source_drift_and_coherence_mutations() {
        let production = b"type Value = { number: Int }\nfn secret(): Int { 42 }\n";
        let companion = b"test sealed { assert(secret() == 42) }\n";
        let request = unsealed_test_request(production, companion, ResourceLimits::default());
        assert!(
            request
                .with_production_compilation(checked_production(b"fn broken(): Int { true }\n"))
                .is_err()
        );
        let request = unsealed_test_request(
            b"fn secret(): Int { 43 }\n",
            companion,
            ResourceLimits::default(),
        );
        assert!(
            request
                .with_production_compilation(checked_production(production))
                .unwrap_err()
                .to_string()
                .contains("changed sealed production source")
        );
        let mut request = unsealed_test_request(production, companion, ResourceLimits::default());
        let root_package = request.packages.root().clone();
        let packages = request
            .packages
            .packages()
            .map(|package| {
                PackageNode::new(
                    package.id().clone(),
                    package.source_id().clone(),
                    if package.id() == &root_package {
                        PackageAlias::new("changed").unwrap()
                    } else {
                        package.local_name().clone()
                    },
                    package.edition(),
                    package.modules().clone(),
                    package
                        .dependencies()
                        .iter()
                        .map(|(alias, package)| (alias.clone(), package.clone())),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        request.packages =
            PackageGraph::new(root_package, request.packages.standard().clone(), packages).unwrap();
        assert!(
            request
                .with_production_compilation(checked_production(production))
                .unwrap_err()
                .to_string()
                .contains("changed sealed production package")
        );
        for companion in [
            "fn Value.added(): Int { 1 }\ntest sealed {}\n",
            "trait Custom {}\nimpl Custom for Value {}\ntest sealed {}\n",
        ] {
            let request =
                unsealed_test_request(production, companion.as_bytes(), ResourceLimits::default())
                    .with_production_compilation(checked_production(production))
                    .unwrap();
            let output = execute(request).unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert!(
                output.diagnostics().human().contains("E2003"),
                "{companion}\n{}",
                output.diagnostics().human()
            );
        }
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
    fn standard_function_values_retain_their_callable_signatures() {
        let output = execute(operation_request_with_capabilities(
            Operation::Run,
            b"import std.math\nfn main() {\n let round = math.round\n let away = math.roundTiesAway\n assert(round(2.5) == 2.0)\n assert(away(2.5) == 3.0)\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        )).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
    }

    #[test]
    fn testing_result_assertion_bounds_apply_to_the_displayed_payload() {
        for (body, expected) in [
            (
                "let value: Hidden ! String = ok(Hidden { value: 1 })\n let check = testing.assertOk[Hidden, String]\n _ = check(value)",
                CompilationStatus::Success,
            ),
            (
                "let value: String ! Hidden = err(Hidden { value: 1 })\n let check = testing.assertErr[String, Hidden]\n _ = check(value)",
                CompilationStatus::Success,
            ),
            (
                "let check = testing.assertOk[Int, Hidden]\n _ = check",
                CompilationStatus::Rejected,
            ),
            (
                "let check = testing.assertErr[Hidden, String]\n _ = check",
                CompilationStatus::Rejected,
            ),
            (
                "_ = testing.assertErr(testing.FloatTolerance.from(-1.0, 0.0))",
                CompilationStatus::Rejected,
            ),
        ] {
            let source = format!(
                "import std.testing\ntype Hidden = {{ value: Int }}\ntest bounds {{\n {body}\n}}\n"
            );
            let request = operation_request(
                Operation::Test,
                source.as_bytes(),
                SourceForm::Module,
                ResourceLimits::default(),
            );
            let entries = discover_tests(&request).unwrap();
            let output = compile(request.for_test_entry(&entries[0]).unwrap()).unwrap();
            assert_eq!(
                output.status(),
                expected,
                "{body}: {}",
                output.diagnostics().human()
            );
            if expected == CompilationStatus::Rejected {
                assert!(
                    output.diagnostics().human().contains("Display"),
                    "{}",
                    output.diagnostics().human()
                );
                assert!(output.bytecode().is_none());
            }
        }
    }

    #[test]
    fn testing_assertion_display_metadata_is_closed_and_bound_to_the_callable() {
        use tondo_vm::bytecode::{BytecodeCallableId, BytecodeTypeId, verify_bytecode};
        let production = b"type Label = { value: Int }\nimpl Display for Label {\n fn display(self): String { \"label\" }\n}\n";
        let companion = b"import std.testing\ntest display {\n let value = Label { value: 1 }\n testing.assertEqual(ref value, ref value)\n}\n";
        let request = unsealed_test_request(production, companion, ResourceLimits::default())
            .with_production_compilation(checked_production(production))
            .unwrap();
        let entries = discover_tests(&request).unwrap();
        if entries.is_empty() {
            panic!("{}", compile(request).unwrap().diagnostics().human());
        }
        let output = compile(request.for_test_entry(&entries[0]).unwrap()).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let program = output.bytecode().unwrap();
        verify_bytecode(program).unwrap();
        let index = program
            .callables
            .iter()
            .position(|callable| callable.name.starts_with("std.testing.assertEqual["))
            .unwrap();
        let display = program.callables[index].assertion_display.unwrap();
        assert!(display.callable.is_some());
        for mutation in [
            "missing",
            "type",
            "target",
            "host target",
            "intrinsic",
            "name",
            "receiver",
        ] {
            let mut invalid = program.clone();
            let callable = &mut invalid.callables[index];
            match mutation {
                "missing" => callable.assertion_display = None,
                "type" => {
                    callable.assertion_display.as_mut().unwrap().value_type =
                        BytecodeTypeId::new(u32::MAX)
                }
                "target" => {
                    callable.assertion_display.as_mut().unwrap().callable =
                        Some(BytecodeCallableId::new(u32::MAX))
                }
                "host target" => {
                    callable.assertion_display.as_mut().unwrap().callable =
                        Some(BytecodeCallableId::new(index as u32))
                }
                "intrinsic" => callable.assertion_display.as_mut().unwrap().callable = None,
                "name" => callable.name = "user.assertEqual".into(),
                "receiver" => callable.parameters[0].receiver = true,
                _ => unreachable!(),
            }
            assert!(verify_bytecode(&invalid).is_err(), "{mutation}");
        }
        let bytes = serde_json::to_vec(program).unwrap();
        let decoded = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(*program, decoded);
    }

    #[test]
    fn qualified_calls_do_not_inherit_an_unrelated_free_function_effect() {
        for (call, expected) in [
            (
                "assert(self.text.length() == 5)",
                CompilationStatus::Success,
            ),
            ("self.wait()", CompilationStatus::Rejected),
            ("length()", CompilationStatus::Rejected),
        ] {
            let source = format!(
                "fn length() suspends {{}}\ntype Label = {{ text: String }}\nfn Label.wait(self) suspends {{}}\nimpl Display for Label {{\n fn display(self): String {{\n {call}\n \"label\"\n }}\n}}\nfn main() {{\n let value = Label {{ text: \"label\" }}\n assert(Display.display(value) == \"label\")\n}}\n"
            );
            let output = execute(operation_request(
                Operation::Run,
                source.as_bytes(),
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(
                output.status(),
                expected,
                "{source}\n{}",
                output.diagnostics().human()
            );
            if expected == CompilationStatus::Success {
                assert_eq!(output.exit_code(), 0);
            } else {
                assert!(
                    output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .any(|diagnostic| diagnostic.code() == "E1114")
                );
            }
        }
    }

    #[test]
    fn implementation_effects_are_checked_after_receiver_dispatch() {
        // This includes the pure-body drift case formerly checked before
        // expressions: a body must actually infer (or declare) the effect.
        for (body, annotation, expected) in [
            ("self.work()\n 1", "", CompilationStatus::Success),
            ("1", "", CompilationStatus::Rejected),
            ("1", "@nosuspend\n", CompilationStatus::Rejected),
        ] {
            let source = format!(
                "trait Contract {{\n fn run(self): Int suspends\n}}\ntype Item = {{}}\nfn Item.work(self) suspends {{}}\nimpl Contract for Item {{\n {annotation}fn run(self): Int {{\n {body}\n }}\n}}\nfn main() {{\n let item = Item {{}}\n assert(Contract.run(item) == 1)\n}}\n"
            );
            let output = execute(operation_request(
                Operation::Run,
                source.as_bytes(),
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(
                output.status(),
                expected,
                "{source}\n{}",
                output.diagnostics().human()
            );
            if expected == CompilationStatus::Success {
                assert_eq!(output.exit_code(), 0);
            } else {
                assert!(
                    output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .any(|diagnostic| diagnostic.code() == "E1114")
                );
            }
        }
    }

    #[test]
    fn runtime_unwind_bytecode_entry_is_closed_even_without_an_ordinary_exit() {
        use tondo_vm::bytecode::{
            BytecodeBlockKind, BytecodeInstructionKind, BytecodeTerminatorKind, verify_bytecode,
        };
        let output = compile(operation_request(
            Operation::Run,
            b"fn cleanup() {}\nfn spin(): Never {\n defer cleanup()\n for {}\n}\nfn main() { spin() }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        )).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let program = output.bytecode().unwrap();
        verify_bytecode(program).unwrap();
        let index = program
            .functions
            .iter()
            .position(|function| {
                function.blocks.iter().any(|block| {
                    block.instructions.iter().any(|instruction| {
                        matches!(
                            instruction.kind,
                            BytecodeInstructionKind::RegisterDefer { .. }
                        )
                    })
                })
            })
            .unwrap();
        let drain = program.functions[index]
            .blocks
            .iter()
            .position(|block| {
                matches!(
                    block.terminator.kind,
                    BytecodeTerminatorKind::DrainUnwind { .. }
                )
            })
            .unwrap();
        for mutation in [
            "missing",
            "duplicate",
            "normal",
            "target",
            "instructions",
            "arbitrary",
        ] {
            let mut invalid = program.clone();
            let function = &mut invalid.functions[index];
            match mutation {
                "missing" => {
                    function.blocks[drain].terminator.kind = BytecodeTerminatorKind::Unreachable
                }
                "duplicate" => function.blocks.push(function.blocks[drain].clone()),
                "normal" => function.blocks[drain].kind = BytecodeBlockKind::Normal,
                "target" => {
                    function.blocks[drain].terminator.kind = BytecodeTerminatorKind::DrainUnwind {
                        target: function.entry,
                    }
                }
                "instructions" => {
                    let instruction =
                        function.blocks[function.entry.index() as usize].instructions[0].clone();
                    function.blocks[drain].instructions.push(instruction);
                }
                "arbitrary" => {
                    function.blocks[drain].terminator.kind = BytecodeTerminatorKind::Goto {
                        target: function.unwind,
                    }
                }
                _ => unreachable!(),
            }
            let error = verify_bytecode(&invalid).unwrap_err();
            if mutation == "arbitrary" {
                assert!(
                    error.to_string().contains("contains executable bytecode"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn temporary_testing_helpers_require_filesystem_at_every_reference() {
        for source in [
            "import std.testing\ntest temporary {\n let temporary = testing.assertOk(testing.tempDirectory(\"capability\"))\n defer temporary.cleanup()\n}\n",
            "import std.testing\ntest temporary {\n let make = testing.tempDirectory\n let temporary = testing.assertOk(make(\"capability\"))\n temporary.cleanup()\n}\n",
            "import std.testing\nfn dispose(value: testing.TempDirectory) { value.cleanup() }\ntest temporary {}\n",
        ] {
            for (capabilities, expected) in [
                (BTreeSet::new(), CompilationStatus::Rejected),
                (
                    BTreeSet::from([CapabilityName::new("filesystem").unwrap()]),
                    CompilationStatus::Success,
                ),
            ] {
                let request = operation_request_with_capabilities(
                    Operation::Test,
                    source.as_bytes(),
                    SourceForm::Module,
                    ResourceLimits::default(),
                    capabilities,
                );
                let entries = discover_tests(&request).unwrap();
                let output = compile(request.for_test_entry(&entries[0]).unwrap()).unwrap();
                assert_eq!(
                    output.status(),
                    expected,
                    "{source}\n{}",
                    output.diagnostics().human()
                );
                if expected == CompilationStatus::Rejected {
                    let diagnostic = output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .find(|diagnostic| diagnostic.code() == "E1008")
                        .unwrap_or_else(|| panic!("{source}\n{}", output.diagnostics().human()));
                    assert!(
                        diagnostic
                            .message()
                            .contains("capability `filesystem` is missing"),
                        "{}",
                        diagnostic.message()
                    );
                    assert!(output.bytecode().is_none());
                } else {
                    assert!(output.bytecode().is_some());
                }
            }
        }
    }

    #[test]
    fn standard_function_values_specialize_generics_and_keep_testing_functions_private() {
        for (body, expected_code) in [
            (
                "let unwrap = testing.assertOk[Int, String]\n let value: Int ! String = ok(42)\n assert(unwrap(value) == 42)",
                None,
            ),
            ("let invoke = testing.__runLeaf", Some("E1102")),
            ("let invoke = testing.__runSuite", Some("E1102")),
            ("let invoke = testing.__beginSuiteCleanup", Some("E1102")),
        ] {
            let source = format!("import std.testing\ntest functions {{\n {body}\n}}\n");
            let request = operation_request(
                Operation::Test,
                source.as_bytes(),
                SourceForm::Module,
                ResourceLimits::default(),
            );
            let entries = discover_tests(&request).unwrap();
            let envelope = crate::test_control::EnvelopeHandle::new(
                "functions",
                crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
            );
            let output = execute(
                request
                    .for_test_entry(&entries[0])
                    .unwrap()
                    .with_test_envelope(envelope),
            )
            .unwrap();
            if let Some(code) = expected_code {
                assert_eq!(output.status(), CompilationStatus::Rejected, "{source}");
                assert!(
                    output
                        .diagnostics()
                        .diagnostics()
                        .iter()
                        .any(|diagnostic| diagnostic.code() == code),
                    "{source}\n{}",
                    output.diagnostics().human()
                );
            } else {
                assert_eq!(
                    output.status(),
                    CompilationStatus::Success,
                    "{source}\n{}",
                    output.diagnostics().human()
                );
                assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
            }
        }
    }

    #[test]
    fn temporary_paths_expose_receiver_methods_without_a_path_module_reference() {
        let request = operation_request(
            Operation::Test,
            b"import std.testing\ntest temporary {\n let temporary = testing.tempDirectory(\"methods\")?\n defer temporary.cleanup()\n let text = temporary.path().toString()?\n _ = text\n let bytes = temporary.path().toBytes()\n assert(bytes.length() > 0)\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&request).unwrap();
        let participation = crate::test_backend::TestParticipation::new(
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
            BTreeMap::new(),
            false,
        );
        let output = compile(
            request
                .for_test_participation(&entries, participation)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert!(output.bytecode().is_some());
    }

    #[test]
    fn shrink_candidates_bytecode_preserves_the_immutable_host_receiver_contract() {
        use tondo_vm::bytecode::{BytecodeParameterMode, BytecodeTypeKind, verify_bytecode};
        let request = operation_request(
            Operation::Test,
            b"test shrink {\n match Shrink.candidates(13, 2) {\n ok(values) => assert(values == [6, 3])\n err(_) => assert(false)\n }\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let entries = discover_tests(&request).unwrap();
        let output = compile(request.for_test_entry(&entries[0]).unwrap()).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        let bytecode = output.bytecode().unwrap();
        verify_bytecode(bytecode).unwrap();
        let index = bytecode
            .callables
            .iter()
            .position(|callable| callable.name.starts_with("std.testing.Shrink.candidates["))
            .unwrap();
        for mutation in ["mutable", "resizable", "receiver", "name", "borrowed limit"] {
            let mut invalid = bytecode.clone();
            let callable = &mut invalid.callables[index];
            match mutation {
                "mutable" => callable.parameters[0].mode = BytecodeParameterMode::Mut,
                "resizable" => callable.parameters[0].mode = BytecodeParameterMode::Var,
                "receiver" => callable.parameters[0].receiver = false,
                "name" => callable.name = "std.testing.Shrink.unrelated[Int]".into(),
                "borrowed limit" => callable.parameters[1].mode = BytecodeParameterMode::Ref,
                _ => unreachable!(),
            }
            let BytecodeTypeKind::Function(function) =
                &mut invalid.types[callable.function_type.index() as usize].kind
            else {
                unreachable!()
            };
            for (parameter, source) in function.parameters.iter_mut().zip(&callable.parameters) {
                parameter.mode = source.mode;
            }
            let error = verify_bytecode(&invalid).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("host callable ABI cannot receive borrowed parameters"),
                "{mutation}: {error}"
            );
        }
    }

    #[test]
    fn test_operation_executes_std_testing_value_helpers_through_the_vm() {
        let mut temporary_root = crate::test_temporaries::TemporaryRoot::create(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
        let base = operation_request(
            Operation::Test,
            b"import std.testing\ntest helpers {\n let tolerance = match testing.FloatTolerance.from(0.01, 0.1) {\n  ok(value) => value\n  err(_) => testing.failNow(\"invalid tolerance\")\n }\n testing.assertTextEqual(\"same\", \"same\")\n testing.assertFloatNear(10.0, 10.5, ref tolerance)\n testing.assertFloat32Near(10.0, 10.5, ref tolerance)\n let diff = testing.diffText(\"old\\n\", \"new\\n\")\n testing.assertTextEqual(diff.render(), \"--- expected\\n+++ actual\\n-old\\n+new\\n\")\n let workspace = match testing.tempDirectory(\"wave5\") {\n  ok(value) => value\n  err(_) => testing.failNow(\"temp directory unavailable\")\n }\n let root = workspace.path()\n workspace.cleanup()\n var generator = testing.Generator.new(7)\n let first = match generator.nextUInt() {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator failed\")\n }\n let second = match generator.nextUInt() {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator failed\")\n }\n testing.assertNotEqual(ref first, ref second)\n let shrunk = match testing.shrink(ref first) {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator shrink failed\")\n }\n _ = shrunk\n var replay = testing.Generator.forCase(7, 2)\n let replayId = replay.id()\n let replayDrawCount = replay.drawCount()\n _ = replayId\n _ = replayDrawCount\n let replayBool = match replay.nextBool() {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator bool failed\")\n }\n let replayInt = match replay.nextInt(0, 4) {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator int failed\")\n }\n let replayBytes = match replay.nextBytes(4) {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator bytes failed\")\n }\n _ = replayBool\n _ = replayInt\n _ = replayBytes\n let generated = match generator.nextText(16) {\n  ok(value) => value\n  err(_) => testing.failNow(\"generator text failed\")\n }\n testing.assertTextEqual(generated, generated)\n}\n",
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
            .with_test_temporary_root(temporary_root.path().to_owned())
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
        temporary_root.cleanup().unwrap();
        assert!(!temporary_root.path().exists());
    }

    #[test]
    fn test_operation_checks_console_stream_protocol_through_the_hir() {
        let request = operation_request(
            Operation::Check,
            b"import std.console\nimport std.io\nimport std.bytes\nfn acquire(): console.Output ! console.ConsoleError {\n console.stdout()\n}\nfn emit(data: bytes.Bytes): Int ! (io.IoError | console.ConsoleError) {\n var output = console.stdout()?\n output.write(data)?\n}\n",
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
    }

    #[test]
    fn test_operation_executes_generic_testing_assertions_and_wrapper_consumers() {
        let base = operation_request(
            Operation::Test,
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
            Operation::Test,
            b"import std.testing\nimport std.time\ntest virtual_clock {\n match testing.withVirtualTime((clock) {\n  let before = time.now()?\n  clock.advance(time.Duration.fromNanoseconds(100))\n  let after = time.now()?\n  assert(after.durationSince(before)?.toNanoseconds() == 100)\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual clock failed\")\n }\n}\n",
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
            Operation::Test,
            b"import std.bytes\nimport std.testing\nimport std.time\nfn exerciseVirtualTime(): Unit ! (bytes.BytesError | time.ClockError) {\n var affine = bytes.builder()?\n testing.withVirtualTime((clock) {\n  _ = affine.finish()?\n  let before = time.now()?\n  scope {\n   let sleeper = spawn time.sleep(time.Duration.fromNanoseconds(40))\n   clock.settle()\n   await sleeper?\n  }\n  let after = time.now()?\n  assert(after.durationSince(before)?.toNanoseconds() == 40)\n })?\n testing.withVirtualTime((clock) {\n  _ = time.now()?\n  clock.advance(time.Duration.fromNanoseconds(5))\n })?\n}\ntest virtual_settle {\n match exerciseVirtualTime() {\n  ok(_) => ()\n  err(_) => testing.failNow(\"virtual time failed\")\n }\n}\n",
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
            Operation::Test,
            b"import std.testing\nimport std.time\ntest invalid_spawn {\n scope {\n  let task = spawn testing.withVirtualTime((clock) {\n   _ = time.now()?\n   clock.settle()\n  })\n  _ = await task\n }\n}\n",
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
            Operation::Test,
            b"import std.testing\nfn consumeUnit(value: Unit) {\n match value {\n  () => ()\n }\n}\nfn ready(): Unit suspends {}\ntest invalid_capture {\n scope {\n  let task = spawn ready()\n  match testing.withVirtualTime((clock) {\n   _ = clock\n   _ = await task\n  }) {\n   ok(_) => ()\n   err(_) => ()\n  }\n }\n}\n",
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
            Operation::Test,
            b"import std.testing\nimport std.time\ntest virtual_panic {\n match testing.withVirtualTime((clock) {\n  _ = time.now()?\n  clock.advance(time.Duration.fromNanoseconds(7))\n  panic(\"inside virtual time\")\n }) {\n  ok(_) => ()\n  err(_) => testing.failNow(\"unexpected clock error\")\n }\n}\n",
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
            Operation::Test,
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
            Operation::Test,
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
        let source = b"import std.console\nfn main() { _ = console.print(\"ready\")\n }\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        assert_eq!(
            rejected.diagnostics().diagnostics().len(),
            2,
            "{}",
            rejected.diagnostics().human()
        );
        let diagnostic = &rejected.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E1008");
        assert!(
            diagnostic
                .message()
                .contains("capability `console` is missing")
        );
        assert_eq!(rejected.diagnostics().diagnostics()[1].code(), "E1001");
        let import_only = execute(operation_request_with_capabilities(
            Operation::Check,
            b"import std.console\nfn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(
            import_only.diagnostics().diagnostics().len(),
            1,
            "{}",
            import_only.diagnostics().human()
        );
        assert_eq!(import_only.diagnostics().diagnostics()[0].code(), "E1008");

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
        assert!(CapabilityName::new("civil-clock").is_ok());
        let stream_source = b"import std.console\nimport std.io\n\nfn acquire_input(): console.Input ! console.ConsoleError {\n    console.stdin()\n}\n\nfn acquire_output(): console.Output ! console.ConsoleError {\n    console.stdout()\n}\n";
        let stream_checked = execute(operation_request(
            Operation::Check,
            stream_source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            stream_checked.status(),
            CompilationStatus::Success,
            "{}",
            stream_checked.diagnostics().human()
        );
        assert!(stream_checked.diagnostics().diagnostics().is_empty());

        let process_source =
            b"import std.process\nfn main() {\n    let command = process.command(\"true\")\n}\n";
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
    fn path_constructors_use_the_documented_type_qualified_surface() {
        let output = execute(operation_request(
            Operation::Run,
            include_bytes!("../../../tests/runtime/m11-std-path-001.to"),
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:?}",
            output.diagnostics()
        );
        assert_eq!(output.stdout(), b"path-ok\n");
        for (call, code) in [
            ("path.fromString(\"a\")", "E1102"),
            ("path.fromBytes(bytes.empty()?)", "E1102"),
            ("path.Path.fromString(1)", "E1102"),
            ("path.Path.fromBytes(\"a\")", "E1102"),
            ("path.Path.fromString[Int](\"a\")", "E1104"),
            ("path.Path[Int].fromString(\"a\")", "E1104"),
        ] {
            let source = format!(
                "import std.path\nimport std.bytes\nfn main(): !(path.PathError | bytes.BytesError) {{\n _ = {call}\n}}\n"
            );
            let output = execute(operation_request(
                Operation::Check,
                source.as_bytes(),
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected, "{call}");
            assert!(
                output
                    .diagnostics()
                    .diagnostics()
                    .iter()
                    .any(|diagnostic| diagnostic.code() == code),
                "{call}: {:?}",
                output.diagnostics()
            );
        }
    }

    #[test]
    fn filesystem_module_requires_the_explicit_target_capability() {
        let source = b"import std.path\nimport std.fs\nfn main(): !(path.PathError | fs.FsError) {\n    let file_path = path.Path.fromString(\"Cargo.toml\")?\n    let contents = fs.readAll(file_path)?\n}\n";
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
                .contains("capability `filesystem` is missing")
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
    fn program_arguments_reach_env_snapshot_without_cli_options() {
        let source = br#"
import std.console
import std.env

fn text(value: env.Value): String {
    match value.asText() {
        some(argumentText) => argumentText
        none => panic("argument is not UTF-8")
    }
}

fn main(): !env.EnvError {
    let arguments = env.snapshot()?.arguments()
    assert(text(arguments[0]) == "--flag")
    assert(text(arguments[1]) == "two words")
    assert(text(arguments[2]) == "*")
    assert(text(arguments[3]) == "$HOME")
    _ = console.print("args-ok\n")
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
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
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
    fn typed_json_bootstrap_source_is_selected_only_by_a_real_standard_import() {
        for source in [
            &b"import std.json\nfn main() {}\n"[..],
            &b"  import std.json as wire\r\nfn main() {}\r\n"[..],
            &b"\xef\xbb\xbfimport std.json\nfn main() {}\n"[..],
        ] {
            assert!(imports_bootstrap_json(source));
        }
        for source in [
            &b"fn main() {}\n"[..],
            &b"// import std.json\nfn main() {}\n"[..],
            &b"import std.jsonExtra\nfn main() {}\n"[..],
            &b"importstd.json\nfn main() {}\n"[..],
        ] {
            assert!(!imports_bootstrap_json(source));
        }
    }

    #[test]
    fn io_bootstrap_sources_require_real_imports_and_exact_reuse() {
        for (source, expected) in [
            (b"import std.io\nfn main() {}\n".as_slice(), 1),
            (b"import std.console\nfn main() {}\n".as_slice(), 2),
            (b"import std.fs\nfn main() {}\n".as_slice(), 2),
            (b"// import std.fs\nfn main() {}\n".as_slice(), 0),
            (b"// import std.io\nfn main() {}\n".as_slice(), 0),
            (b"import std.ioExtra\nfn main() {}\n".as_slice(), 0),
        ] {
            let mut request = source_request(source, SourceForm::Module, ResourceLimits::default());
            let initial = request.sources.len();
            install_bootstrap_standard_sources(&mut request).unwrap();
            assert_eq!(request.sources.len(), initial + expected);
            install_bootstrap_standard_sources(&mut request).unwrap();
            assert_eq!(request.sources.len(), initial + expected);
        }
        for (origin, bytes) in [
            (
                crate::source::SourceOrigin::Virtual,
                include_bytes!("bootstrap/io.to").as_slice(),
            ),
            (
                crate::source::SourceOrigin::GeneratedStandard,
                b"pub trait Reader {}\n".as_slice(),
            ),
        ] {
            let mut request = source_request(
                b"import std.io\nfn main() {}\n",
                SourceForm::Module,
                ResourceLimits::default(),
            );
            let source_id = request
                .packages
                .package(request.packages.standard())
                .unwrap()
                .source_id()
                .clone();
            request
                .sources
                .add(crate::source::SourceInput::new(
                    source_id,
                    crate::source::ModulePath::new("io").unwrap(),
                    crate::source::LogicalPath::new("compiler/io.to").unwrap(),
                    origin,
                    bytes,
                ))
                .unwrap();
            let error = install_bootstrap_standard_sources(&mut request).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("differs from the selected compiler")
            );
        }
    }

    #[test]
    fn filesystem_adapter_does_not_replace_an_explicit_source_module() {
        let mut request = source_request(
            b"import std.fs\nfn consume(value: fs.Marker) {}\nfn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let standard = request
            .packages
            .package(request.packages.standard())
            .unwrap()
            .source_id()
            .clone();
        request
            .sources
            .add(crate::source::SourceInput::virtual_file(
                standard,
                crate::source::ModulePath::new("fs").unwrap(),
                crate::source::LogicalPath::new("provided-fs.to").unwrap(),
                b"pub type Marker = Int\n".as_slice(),
            ))
            .unwrap();
        install_bootstrap_standard_sources(&mut request).unwrap();
        assert!(
            !request
                .sources
                .iter()
                .any(|(_, source)| { source.path().as_str() == "compiler/fs_io.to" })
        );
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
    }

    #[test]
    fn incomplete_bootstrap_io_dependencies_return_a_package_error() {
        let mut request = source_request(
            b"import std.console\nfn main() {\n _ = console.print(\"value\")\n}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        );
        let standard = request.packages.standard().clone();
        let nodes = request
            .packages
            .packages()
            .map(|node| {
                if node.id() == &standard {
                    crate::package::PackageNode::new(
                        node.id().clone(),
                        node.source_id().clone(),
                        node.local_name().clone(),
                        node.edition(),
                        [crate::source::ModulePath::new("console").unwrap()],
                        [],
                    )
                    .unwrap()
                } else {
                    node.clone()
                }
            })
            .collect::<Vec<_>>();
        request.packages =
            PackageGraph::new(request.packages.root().clone(), standard, nodes).unwrap();
        let result = execute(request);
        assert!(
            matches!(&result, Err(DriverError::Hir(HirError::Package(
            crate::package::PackageGraphError::UndeclaredModule { module, .. }
        ))) if module.as_str() == "io"),
            "{result:?}"
        );
    }

    #[test]
    fn unused_parameter_warnings_require_an_implementation_body() {
        let source = b"trait Protocol {\n\
            fn required(self, value: Int)\n\
            fn provided(self, unused: Int) {}\n\
        }\n\
        fn ordinary(unused: Int) {}\n";
        let output = execute(
            source_request(source, SourceForm::Module, ResourceLimits::default())
                .with_warning_profiles([WarningProfile::Core]),
        )
        .unwrap();
        assert_eq!(output.exit_code(), 0, "{}", output.diagnostics().human());
        let diagnostics = output.diagnostics().diagnostics();
        assert_eq!(diagnostics.len(), 2, "{}", output.diagnostics().human());
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code() == "W1003")
        );
        assert!(!output.diagnostics().human().contains("parameter `value`"));
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
    fn static_test_diagnostics_respect_the_shared_compiler_budget() {
        for source in [
            &b"test first {}\ntest second {}\n"[..],
            &b"import std.testing\nimport std.testing as again\nfn main() {}\n"[..],
        ] {
            let output = execute(source_request(
                source,
                SourceForm::Module,
                ResourceLimits {
                    max_diagnostics: 1,
                    ..ResourceLimits::default()
                },
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics().len(), 1);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
            assert!(
                output.diagnostics().diagnostics()[0]
                    .message()
                    .contains("primary diagnostic count")
            );
        }
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
              _ = console.print(\"{answer}\")\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(sync.status(), CompilationStatus::Success);
        assert_eq!(sync.exit_code(), 0);
        assert_eq!(sync.stdout(), b"42");

        let asynchronous = execute(operation_request(
            Operation::Run,
            b"fn tick(): Int suspends { 42 }\n\
              let answer = tick()\n\
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
    fn script_direct_fail_and_nested_closures_keep_separate_inferred_error_channels() {
        for source in [
            b"enum Fault { Broken }\nfail Fault.Broken\n".as_slice(),
            b"enum Fault { Broken }\nfn invoke[E: Discard](body: fn(): Unit ! E): Unit ! E { body()? }\ninvoke(() {\n fail Fault.Broken\n})?\n".as_slice(),
        ] {
            let output = execute(operation_request(Operation::Run, source, SourceForm::Script, ResourceLimits::default())).unwrap();
            assert_eq!(output.exit_code(), 1);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "R0001", "{:?}", output.diagnostics());
            assert!(output.diagnostics().diagnostics()[0].message().contains("Fault"));
        }
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
    fn compile_verifies_all_test_bodies_without_entering_the_vm() {
        let request = operation_request(
            Operation::Test,
            b"suite outer { assert(false)\n test first { assert(false) }\n test second { assert(false) }\n}\n",
            SourceForm::Module,
            ResourceLimits { max_vm_steps: 0, ..ResourceLimits::default() },
        );
        let entries = discover_tests(&request).unwrap();
        assert_eq!(entries.len(), 2);
        let participation = test_backend::TestParticipation::new(
            crate::test_control::EnvelopeLimits::new(4096, 4096, 4096),
            BTreeMap::new(),
            false,
        );
        let output = compile(
            request
                .for_test_participation(&entries, participation.clone())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert!(output.stdout().is_empty());
        assert!(output.diagnostic_trace().is_none());
        assert!(output.mir_summary().is_some());
        tondo_vm::bytecode::verify_bytecode(output.bytecode().unwrap()).unwrap();
        assert!(participation.executions().unwrap().is_empty());

        for limits in [
            ResourceLimits {
                max_mir_verification_steps: 0,
                ..ResourceLimits::default()
            },
            ResourceLimits {
                max_bytecode_verification_steps: 0,
                ..ResourceLimits::default()
            },
        ] {
            let output = compile(operation_request(
                Operation::Test,
                b"test body { assert(true) }\n",
                SourceForm::Module,
                limits,
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
            assert!(output.bytecode().is_none());
        }
        assert!(
            compile(operation_request(
                Operation::Check,
                b"fn main() {}\n",
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .is_err()
        );
    }

    #[test]
    fn native_probe_can_invoke_a_verified_scalar_function_in_the_vm() {
        let output = execute(
            operation_request(
                Operation::Run,
                b"fn add(left: Int, right: Int): Int { left + right }\n\
                  fn main() {}\n",
                SourceForm::Script,
                ResourceLimits::default(),
            )
            .with_bytecode_observation(),
        )
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        let bytecode = output
            .bytecode()
            .expect("run output retains verified bytecode");
        let callable = bytecode
            .callables
            .iter()
            .find(|callable| callable.name.ends_with("::value::add"))
            .expect("add callable is present");
        let function = callable
            .implementation
            .expect("add callable has a bytecode function");
        let mut host = RejectingHost;
        let execution = execute_with_arguments(
            bytecode,
            function,
            vec![RuntimeValue::Integer(20), RuntimeValue::Integer(22)],
            &mut host,
        )
        .expect("scalar function should execute in the VM");
        assert!(matches!(
            execution.outcome,
            VmOutcome::Returned(RuntimeValue::Integer(42))
        ));

        let mut host = RejectingHost;
        assert!(
            execute_with_arguments(
                bytecode,
                function,
                vec![RuntimeValue::Integer(20)],
                &mut host,
            )
            .is_err()
        );
        let mut host = RejectingHost;
        assert!(
            execute_with_arguments(
                bytecode,
                function,
                vec![
                    RuntimeValue::String("not-scalar".to_owned()),
                    RuntimeValue::Integer(22),
                ],
                &mut host,
            )
            .is_err()
        );
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
              let later: fn(): Int suspends = () { 3 }\n\
              let both: unsafe fn(): Int suspends = unsafe () { 4 }\n\
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

        for source in [
            &b"fn main() {\n    let operation: fn(): Int suspends = (): Int { 1 }\n    _ = operation()\n}\n"[..],
            &b"fn operation(): Int suspends { 1 }\nfn main() {\n    _ = operation()\n}\n"[..],
        ] {
            let output = execute(operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Success);
            assert!(output.diagnostics().diagnostics().is_empty());
        }

        for source in [
            &b"fn main() {\n    let operation = unsafe (): Int { 1 }\n    _ = operation()\n}\n"[..],
            &b"unsafe fn operation(): Int { 1 }\nfn main() {\n    _ = operation()\n}\n"[..],
        ] {
            let output = execute(operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1701");
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
        let source = b"unsafe fn raw(value: Int): Int suspends { value + 1 }\n\
            fn main() {\n\
                let result = unsafe { raw(41) }\n\
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

        let invalid = b"unsafe fn raw(): Int suspends { 1 }\n\
            fn main() {\n\
                _ = raw()\n\
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
    fn sync_bootstrap_surface_is_lowered_when_the_module_is_imported() {
        const SOURCE: &[u8] = br#"import std.sync
fn main(): !sync.SyncError {
    let mutex = sync.mutex(1)?
    var guard = mutex.lock()?
    _ = guard.get()
    _ = guard.getMut()
    guard.unlock()
    let maybe_guard = mutex.tryLock()
    match maybe_guard {
        some(guard2) => guard2.unlock()
        none => ()
    }

    let rw = sync.rwLock(2)?
    let reader = rw.read()?
    _ = reader.get()
    let maybe_reader = rw.tryRead()
    match maybe_reader {
        some(reader2) => reader2.unlock()
        none => ()
    }
    reader.unlock()
    var writer = rw.write()?
    _ = writer.get()
    _ = writer.getMut()
    writer.unlock()
    let maybe_writer = rw.tryWrite()
    match maybe_writer {
        some(writer2) => writer2.unlock()
        none => ()
    }

    let condition = sync.condition()?
    let condition_mutex = sync.mutex(0)?
    var condition_guard = condition_mutex.lock()?
    let waited_guard = condition.wait(var condition_guard)
    waited_guard.unlock()
    condition.notifyOne()
    condition.notifyAll()

    let semaphore = sync.semaphore(2)?
    let permit = semaphore.acquire()
    permit.release()
    let maybe_permit = semaphore.tryAcquire()
    match maybe_permit {
        some(permit2) => permit2.release()
        none => ()
    }

    let once = sync.once[Int, sync.SyncError]()
    _ = once.get()
    _ = once.getOrInit(() { 1 })
    _ = once.isReady()
    let barrier = sync.barrier(1)?
    _ = barrier.wait()?

    let atomic = sync.atomic(1)
    let relaxed = sync.MemoryOrder.Relaxed()
    let acquire = sync.MemoryOrder.Acquire()
    let release = sync.MemoryOrder.Release()
    let acq_rel = sync.MemoryOrder.AcqRel()
    let seq_cst = sync.MemoryOrder.SeqCst()
    _ = relaxed
    _ = acquire
    _ = release
    _ = acq_rel
    _ = seq_cst
    let order = sync.MemoryOrder.SeqCst
    _ = atomic.load(order)
    atomic.store(2, order)
    _ = atomic.swap(3, order)
    _ = atomic.compareExchange(3, 4, order, order)
}
"#;
        let output = execute(operation_request(
            Operation::Run,
            SOURCE,
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
    fn sync_collection_surface_executes_through_the_hosted_vm() {
        let output = execute(operation_request(
            Operation::Run,
            br#"import std.sync as concurrent
fn main() {
    let array: concurrent.Array[Int] = concurrent.Array[1, 2]
    assert(array.length() == 2)
    assert(array.get(0) == some(1))
    _ = array.set(1, 3)
    assert(array.get(1) == some(3))
    _ = array.compareExchange(0, 1, 4)
    assert(array.get(0) == some(4))
    _ = array.snapshot()

    let map: concurrent.Map[String, Int] = concurrent.Map["a": 1]
    assert(map.contains("a"))
    _ = map.insert("b", 2)
    assert(map.get("b") == some(2))
    _ = map.remove("a")
    _ = map.compareExchange("b", some(2), none)
    _ = map.snapshot()

    let set: concurrent.Set[String] = concurrent.Set["a"]
    _ = set.insert("b")
    assert(set.contains("b"))
    _ = set.remove("a")
    _ = set.snapshot()

    let stack: concurrent.Stack[Int] = concurrent.Stack[1]
    _ = stack.push(2)
    assert(stack.peek() == some(2))
    _ = stack.pop()
    _ = stack.snapshot()

    let queue: concurrent.Queue[Int] = concurrent.Queue[1]
    _ = queue.enqueue(2)
    assert(queue.peek() == some(1))
    _ = queue.dequeue()
    _ = queue.snapshot()
}
"#,
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
    fn sync_collection_direct_for_uses_finite_host_cursor_order() {
        let output = execute(operation_request(
            Operation::Run,
            br#"import std.sync as concurrent
fn main() {
    let array: concurrent.Array[Int] = concurrent.Array[1, 2, 3]
    var array_sum: Int = 0
    for item in array {
        array_sum += item
    }
    assert(array_sum == 6)

    let map: concurrent.Map[String, Int] = concurrent.Map["a": 1, "b": 2]
    var map_sum: Int = 0
    for (key, value) in map {
        if key == "a" {
            ()
        } else {
            assert(key == "b")
        }
        map_sum += value
    }
    assert(map_sum == 3)

    let set: concurrent.Set[Int] = concurrent.Set[4, 5]
    var set_sum: Int = 0
    for item in set {
        set_sum += item
    }
    assert(set_sum == 9)

    let stack: concurrent.Stack[Int] = concurrent.Stack[1, 2, 3]
    var stack_sum: Int = 0
    for item in stack {
        stack_sum = stack_sum * 10 + item
    }
    assert(stack_sum == 321)

    let queue: concurrent.Queue[Int] = concurrent.Queue[1, 2, 3]
    var queue_sum: Int = 0
    for item in queue {
        queue_sum = queue_sum * 10 + item
    }
    assert(queue_sum == 123)
}
"#,
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
    fn sync_bootstrap_rejects_unknown_static_and_module_operations() {
        for source in [
            b"import std.sync\nfn main() { _ = sync.MemoryOrder.Unknown() }\n".as_slice(),
            b"import std.sync\nfn main() { _ = sync.unknown() }\n".as_slice(),
        ] {
            let output = execute(operation_request(
                Operation::Run,
                source,
                SourceForm::Script,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected);
            assert!(!output.diagnostics().diagnostics().is_empty());
        }
    }

    #[test]
    fn inferred_suspending_main_executes_in_the_runtime_root_scope() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn work() suspends {}\nfn main() {\n    scope {\n        let job = spawn work()\n        await job\n    }\n}\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn direct_suspension_is_inferred_and_join_can_cross_a_function_boundary() {
        let output = execute(operation_request_with_capabilities(
            Operation::Run,
            b"fn work(): Int suspends { 1 }\n\
              fn prepare(): Join[Int, Never] {\n\
                  scope {\n\
                      return spawn thread work()\n\
                  }\n\
              }\n\
              fn main() {\n\
                  let job = prepare()\n\
                  let value = await job\n\
                  _ = value\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
            BTreeSet::from([CapabilityName::new("threads").unwrap()]),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn thread_spawn_requires_an_explicit_threads_target_capability() {
        let source = b"fn work(): Int suspends { 1 }\n\
                      fn main() {\n\
                          scope {\n\
                              let job = spawn thread work()\n\
                              let value = await job\n\
                              _ = value\n\
                          }\n\
                      }\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        let diagnostic = rejected
            .diagnostics()
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code() == "E1008")
            .expect("missing threads diagnostic");
        assert!(
            diagnostic
                .message()
                .contains("capability `threads` is missing")
        );

        let accepted = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
            BTreeSet::from([CapabilityName::new("threads").unwrap()]),
        ))
        .unwrap();
        assert_eq!(accepted.status(), CompilationStatus::Success);
        assert!(accepted.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn blocking_pool_requires_an_explicit_threads_target_capability() {
        let source = b"import std.executor\n\
                      fn main(): !executor.ExecutorError {\n\
                          let pool = executor.blockingPool(1, 1)?\n\
                          pool.shutdown()\n\
                      }\n";
        let rejected = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(rejected.status(), CompilationStatus::Rejected);
        let diagnostic = rejected
            .diagnostics()
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.code() == "E1008")
            .expect("missing threads diagnostic");
        assert_eq!(
            diagnostic.message(),
            "capability `threads` is missing for `executor.blockingPool`"
        );

        let accepted = execute(operation_request_with_capabilities(
            Operation::Check,
            source,
            SourceForm::Script,
            ResourceLimits::default(),
            BTreeSet::from([CapabilityName::new("threads").unwrap()]),
        ))
        .unwrap();
        assert_eq!(accepted.status(), CompilationStatus::Success);
        assert!(accepted.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_iterator_for_awaits_each_poll_without_an_array_intermediate() {
        let output = execute(operation_request(
            Operation::Run,
            b"type Counter = { value: Int }\n\
              impl AsyncIterator[Int] for Counter {\n\
                  fn next(mut self): Int? suspends { none }\n\
              }\n\
              fn consume(cursor: Counter) {\n\
                  for item in cursor {\n\
                      _ = item\n\
                  }\n\
              }\n\
              fn main() { consume(Counter { value: 0 }) }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_iterator_collect_materializes_with_a_bound_without_an_extra_poll() {
        let output = execute(operation_request(
            Operation::Run,
            b"type Counter = { remaining: Int }\n\
              impl AsyncIterator[Int] for Counter {\n\
                  fn next(mut self): Int? suspends {\n\
                      if self.remaining == 0 {\n\
                          return none\n\
                      }\n\
                      let current = self.remaining\n\
                      self.remaining -= 1\n\
                      some(current)\n\
                  }\n\
              }\n\
              fn consume(cursor: Counter) {\n\
                  let result = cursor.collect(limit: 2)\n\
                  match result {\n\
                      ok(values) => assert(values == [3, 2])\n\
                      err(_) => panic(\"collect failed\")\n\
                  }\n\
              }\n\
              fn main() { consume(Counter { remaining: 3 }) }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_iterator_collect_spawn_uses_the_same_generic_cursor_and_cancellation() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.async\n\
              type Counter = { remaining: Int }\n\
              impl AsyncIterator[Int] for Counter {\n\
                  fn next(mut self): Int? suspends {\n\
                      tick()\n\
                      if self.remaining == 0 {\n\
                          return none\n\
                      }\n\
                      let current = self.remaining\n\
                      self.remaining -= 1\n\
                      some(current)\n\
                  }\n\
              }\n\
              type Blocked = { waiter: async.Waiter[Int, String] }\n\
              impl AsyncIterator[Int] for Blocked {\n\
                  fn next(mut self): Int? suspends {\n\
                      let result = self.waiter.wait()\n\
                      match result {\n\
                          ok(value) => some(value)\n\
                          err(_) => none\n\
                      }\n\
                  }\n\
              }\n\
              fn tick() suspends {}\n\
              fn bounded() {\n\
                  scope {\n\
                      let pending = spawn Counter { remaining: 3 }.collect(limit: 2)\n\
                      let result = await pending\n\
                      match result {\n\
                          ok(values) => assert(values == [3, 2])\n\
                          err(_) => panic(\"spawn collect failed\")\n\
                      }\n\
                  }\n\
              }\n\
              fn cancelled(waiter: async.Waiter[Int, String]) {\n\
                  scope {\n\
                      let pending = spawn Blocked { waiter: waiter }.collect(limit: 1)\n\
                      return\n\
                  }\n\
              }\n\
              fn main() {\n\
                  bounded()\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  cancelled(waiter)\n\
                  _ = completer.cancel()\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_oneshot_completes_pending_waiters_once_and_cancels_cleanly() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.async\n\
              fn waitOnce(waiter: async.Waiter[Int, String]): Int ! String suspends {\n\
                  var owned = waiter\n\
                  owned.wait()\n\
              }\n\
              fn direct() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  _ = completer.complete(7)\n\
                  let value = waiter.wait()\n\
                  match value {\n\
                      ok(7) => ()\n\
                      ok(_) => panic(\"unexpected one-shot value\")\n\
                      err(_) => panic(\"unexpected one-shot error\")\n\
                  }\n\
              }\n\
              fn pending() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  scope {\n\
                      let job = spawn waitOnce(waiter)\n\
                      _ = completer.complete(9)\n\
                      let value = await job\n\
                      match value {\n\
                          ok(9) => ()\n\
                          ok(_) => panic(\"unexpected pending value\")\n\
                          err(_) => panic(\"unexpected pending error\")\n\
                      }\n\
                  }\n\
              }\n\
              fn duplicate_result() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  _ = completer.complete(1)\n\
                  let duplicate = completer.complete(2)\n\
                  match duplicate {\n\
                      ok(_) => panic(\"duplicate completion succeeded\")\n\
                      err(_) => ()\n\
                  }\n\
                  _ = waiter.wait()\n\
              }\n\
              fn failed_result() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  _ = completer.fail(\"failed\")\n\
                  let value = waiter.wait()\n\
                  match value {\n\
                      ok(_) => panic(\"one-shot failure succeeded\")\n\
                      err(_) => ()\n\
                  }\n\
              }\n\
              fn main() {\n\
                  direct()\n\
                  pending()\n\
                  duplicate_result()\n\
                  failed_result()\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_oneshot_cancellation_propagates_without_leaking_frames() {
        let result = execute(operation_request(
            Operation::Run,
            b"import std.async\n\
              fn main() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  _ = completer.cancel()\n\
                  let value = waiter.wait()\n\
                  _ = value\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ));
        let Err(DriverError::Vm(VmError::Invariant(message))) = result else {
            panic!("one-shot cancellation did not propagate as a VM cancellation: {result:?}");
        };
        assert!(message.contains("root task was cancelled"), "{message}");
    }

    #[test]
    fn script_entry_infers_suspension_for_direct_waiter_calls() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.async\n\
              let pair = async.oneshot[Int, String]()\n\
              var (waiter, completer) = pair\n\
              _ = completer.complete(42)\n\
              let value = waiter.wait()\n\
              _ = value\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn inferred_suspendible_defer_runs_without_extra_syntax() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.async\n\
              fn waitOnce(waiter: async.Waiter[Int, String]): Int ! String suspends {\n\
                  var owned = waiter\n\
                  owned.wait()\n\
              }\n\
              fn cleanup() {\n\
                  let pair = async.oneshot[Int, String]()\n\
                  var (waiter, completer) = pair\n\
                  scope {\n\
                      let job = spawn waitOnce(waiter)\n\
                      _ = completer.complete(1)\n\
                      let value = await job\n\
                      _ = value\n\
                  }\n\
              }\n\
              fn main() {\n\
                  defer cleanup()\n\
              }\n",
            SourceForm::Script,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics()
        );
        assert_eq!(output.exit_code(), 0);
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn async_intrinsic_types_accept_explicit_generic_module_paths() {
        let output = execute(operation_request(
            Operation::Check,
            b"import std.async\n\
              type W = async.Waiter[Int, String]\n\
              type C = async.Completer[Int, String]\n\
              fn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Success);
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
    fn async_deferred_cleanup_panic_is_suppressed_after_the_primary() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn cleanup() suspends {\n\
                  panic(\"cleanup\")\n\
              }\n\
              fn main() {\n\
                  defer cleanup()\n\
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
        assert!(json.contains("suppressed explicit-panic: cleanup"));
    }

    #[test]
    fn structured_child_panics_use_creation_order_after_sibling_cleanup() {
        let output = execute(operation_request(
            Operation::Run,
            b"fn tick() suspends {}\n\
              fn first() {\n\
                  tick()\n\
                  tick()\n\
                  tick()\n\
                  panic(\"first child\")\n\
              }\n\
              fn second() {\n\
                  tick()\n\
                  tick()\n\
                  tick()\n\
                  panic(\"second child\")\n\
              }\n\
              fn main() {\n\
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
            b"import std.console\n\nfn main() {\n    _ = console.print(\"Hello, world\")\n}\n",
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
    fn async_deferred_cleanup_does_not_mask_vm_resource_failure() {
        let output = execute(operation_request(
            Operation::Run,
            b"import std.console\n\
              fn cleanup() suspends {\n\
                  _ = console.print(\"cleanup\\n\")\n\
              }\n\
              fn main() {\n\
                  defer cleanup()\n\
                  for {}\n\
              }\n",
            SourceForm::Script,
            ResourceLimits {
                max_vm_steps: 64,
                ..ResourceLimits::default()
            },
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.exit_code(), 1);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "T0002");
        // A VM/toolchain resource failure is terminal rather than a language
        // unwind. It must retain `T0002` and must not manufacture a partial
        // user cleanup result; panic and cooperative cancellation are the
        // paths that drain inferred suspendible cleanup.
        assert!(output.stdout().is_empty());
    }

    #[test]
    fn async_deferred_cleanup_requires_its_host_capability() {
        let output = execute(operation_request_with_capabilities(
            Operation::Check,
            b"import std.time\n\
              fn cleanup() suspends {\n\
                  let zero = time.Duration.fromNanoseconds(0)\n\
                  match time.sleep(zero) {\n\
                      ok(_) => (),\n\
                      err(_) => panic(\"clock\")\n\
                  }\n\
              }\n\
              fn main() {\n\
                  defer cleanup()\n\
              }\n",
            SourceForm::Module,
            ResourceLimits::default(),
            BTreeSet::new(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E1008");
        assert!(
            output.diagnostics().diagnostics()[0]
                .message()
                .contains("capability `clock` is missing")
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

    #[test]
    fn reflection_public_api_rejects_identity_construction_ordering_and_value_access() {
        for body in [
            "fn invalid(): reflect.TypeId { reflect.TypeId(1) }",
            "fn invalid(): String { \"{reflect.typeInfo[Int]().id()}\" }",
            "fn invalid(): Bool { reflect.typeInfo[Int]().id() < reflect.typeInfo[String]().id() }",
            "fn invalid(): Int { reflect.typeInfo[Int]().get() }",
            "fn invalid(): Int { reflect.allTypes() }",
            "fn invalid(): reflect.TypeInfo { reflect.typeInfo[Int](1) }",
        ] {
            let source = format!("import std.reflect\n{body}\n");
            let output = execute(operation_request(
                Operation::Check,
                source.as_bytes(),
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(output.status(), CompilationStatus::Rejected, "{source}");
            assert!(!output.diagnostics().diagnostics().is_empty());
            assert!(
                output
                    .diagnostics()
                    .diagnostics()
                    .iter()
                    .all(|diagnostic| !diagnostic.code().starts_with("E000")),
                "{}",
                output.diagnostics().human()
            );
        }
    }

    #[test]
    fn derive_providers_are_compiled_in_one_atomic_frontend_round() {
        let output = execute(operation_request(
            Operation::Check,
            b"import std.serialization\n\
              type User = { id: Int, name: String }\n\
              derive serialization.Encode[Json] + serialization.Decode[Json] for User\n\
              fn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert!(output.diagnostics().is_empty());
    }

    #[test]
    fn derive_supports_generic_enum_and_newtype_targets() {
        let output = execute(operation_request(
            Operation::Check,
            b"import std.serialization\n\
              type Boxed[T] = { value: T }\n\
              enum Choice { Empty, Item(Int) }\n\
              type UserId = Int\n\
              derive[T] serialization.Encode[Json] + serialization.Decode[Json] for Boxed[T]\n\
              derive serialization.Encode[Json] + serialization.Decode[Json] for Choice\n\
              derive serialization.Encode[Json] + serialization.Decode[Json] for UserId\n\
              fn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
    }

    #[test]
    fn meta_derive_publication_retains_final_source_provenance_and_query() {
        let text = b"import std.serialization\n\
            /// Public schema documentation.\n\
            type User = { id: Int, name: String }\n\
            derive serialization.Encode[Json] + serialization.Decode[Json] for User\n";
        let mut hashes = Vec::new();
        for _ in 0..2 {
            let output = execute(operation_request(
                Operation::Check,
                text,
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(
                output.status(),
                CompilationStatus::Success,
                "{}",
                output.diagnostics().human()
            );
            let artifact = output.artifact().unwrap();
            assert_eq!(artifact.generation().len(), 2);
            let model = output.semantic_model().unwrap();
            let query = model.meta_expansions().unwrap();
            assert_eq!(query.expansions().len(), 2);
            for record in artifact.generation() {
                assert_eq!(record.kind, "derive");
                assert_eq!(record.outputs.len(), 1);
                let source = &record.outputs[0];
                assert_eq!(
                    record.id,
                    record.request_hash.replacen("sha256:", "derive:", 1)
                );
                assert_eq!(source.source_id, record.id);
                assert_eq!(
                    source.path,
                    format!("@generated/derive/{}.to", &record.request_hash[7..])
                );
                let expansion = query.by_output(&source.source_id).unwrap();
                assert!(expansion.target().unwrap().ends_with("::type::User"));
                assert!(expansion.source().starts_with("import std.serialization\n"));
                assert_eq!(
                    crate::artifact::sha256(expansion.source().as_bytes()),
                    source.sha256
                );
                let (_, file) = model
                    .sources()
                    .iter()
                    .find(|(_, file)| file.source_id().as_str() == source.source_id)
                    .unwrap();
                assert_eq!(file.path().as_str(), source.path);
                assert_eq!(file.bytes(), expansion.source().as_bytes());
                assert!(!file.diagnostic_mappings().is_empty());
            }
            let bytes = query.canonical_bytes().unwrap();
            assert_eq!(
                crate::meta_query::MetaQueryDocument::decode(&bytes).unwrap(),
                query
            );
            hashes.push((artifact.build_hash().to_owned(), bytes));
        }
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    fn meta_standard_derives_resolve_aliases_and_reject_duplicate_trait_identity() {
        let source = b"import std.serialization as codec\n\
            type User = { id: Int }\n\
            derive codec.Encode[Json] + codec.Decode[Json] for User\n";
        let output = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(output.artifact().unwrap().generation().len(), 2);
        let source = b"import std.serialization\nimport std.serialization as codec\n\
            type User = { id: Int }\n\
            derive serialization.Encode[Json] for User\n\
            derive codec.Encode[Json] for User\n";
        let output = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(output.artifact().is_none());
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "E2103")
        );
    }

    #[test]
    fn meta_module_ownership_keeps_other_module_private_fields_inaccessible() {
        let output = execute(multimodule_request(
            Operation::Check,
            b"import app.api\nfn inspect(value: api.User): Int { value.secret }\n",
            b"pub type User = { priv secret: Int }\n",
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(
            output
                .diagnostics()
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code() == "E1501")
        );
        assert!(output.artifact().is_none());
    }

    #[test]
    fn meta_derive_duplicate_detection_uses_resolved_argument_types() {
        let source = b"import std.serialization\n\
            alias JsonAlias = Json\n\
            type User = { id: Int }\n\
            derive serialization.Encode[Json] for User\n\
            derive serialization.Encode[JsonAlias] for User\n";
        let output = execute(operation_request(
            Operation::Check,
            source,
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert!(output.artifact().is_none());
        assert_eq!(
            output.diagnostics().diagnostics().len(),
            1,
            "{}",
            output.diagnostics().human()
        );
        assert_eq!(
            output.diagnostics().diagnostics()[0].code(),
            "E2103",
            "{}",
            output.diagnostics().human()
        );
    }

    #[test]
    fn derive_directions_compile_empty_and_scalar_records_independently() {
        for (label, declaration, derived_trait) in [
            (
                "encode empty",
                "type Item = {}",
                "serialization.Encode[Json]",
            ),
            (
                "decode empty",
                "type Item = {}",
                "serialization.Decode[Json]",
            ),
            (
                "encode scalar",
                "type Item = { value: Int }",
                "serialization.Encode[Json]",
            ),
            (
                "decode scalar",
                "type Item = { value: Int }",
                "serialization.Decode[Json]",
            ),
            (
                "encode generic",
                "type Item[T] = { value: T }",
                "[T] serialization.Encode[Json]",
            ),
            (
                "decode generic",
                "type Item[T] = { value: T }",
                "[T] serialization.Decode[Json]",
            ),
        ] {
            let source = format!(
                "import std.serialization\n{declaration}\nderive {derived_trait} for Item\nfn main() {{}}\n"
            );
            let output = execute(operation_request(
                Operation::Check,
                source.as_bytes(),
                SourceForm::Module,
                ResourceLimits::default(),
            ))
            .unwrap();
            assert_eq!(
                output.status(),
                CompilationStatus::Success,
                "{label}: {}",
                output.diagnostics().human()
            );
        }
    }

    #[test]
    fn derive_rejects_duplicate_pairs_before_publishing_generated_sources() {
        let output = execute(operation_request(
            Operation::Check,
            b"import std.serialization\n\
              type User = { id: Int }\n\
              derive serialization.Encode[Json] for User\n\
              derive serialization.Encode[Json] for User\n\
              fn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        assert_eq!(output.diagnostics().diagnostics().len(), 1);
        assert_eq!(output.diagnostics().diagnostics()[0].code(), "E2103");
    }

    #[test]
    fn derive_provider_errors_are_mapped_to_the_authorizing_source() {
        let output = execute(operation_request(
            Operation::Check,
            b"import std.serialization\n\
              type User = { @json(base64) id: Int }\n\
              derive serialization.Encode[Json] for User\n\
              fn main() {}\n",
            SourceForm::Module,
            ResourceLimits::default(),
        ))
        .unwrap();
        assert_eq!(output.status(), CompilationStatus::Rejected);
        let diagnostic = &output.diagnostics().diagnostics()[0];
        assert_eq!(diagnostic.code(), "E2104");
        assert!(diagnostic.message().contains("requires Bytes field"));
        assert!(output.diagnostics().human().contains("main.to:"));
    }
}
