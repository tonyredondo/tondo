use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ADAPTER_PROTOCOL, SUITE_FORMAT, SUITE_NAME, sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormativeRegistry {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub panics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDeclaration {
    pub name: String,
    pub profile: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseGroup {
    LexParseFormat,
    CompilePass,
    CompileFail,
    SemanticQueries,
    Runtime,
    Concurrency,
    Hosted,
    Memory,
    Determinism,
    Documentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceOperation {
    Format,
    Check,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceForm {
    Module,
    Script,
    Fragment,
    Syntax,
    StandaloneBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    pub source_id: String,
    pub module: String,
    pub logical_path: String,
    pub contents: PinnedFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceAction {
    pub operation: SourceOperation,
    pub form: SourceForm,
    pub root: String,
    pub sources: Vec<SourceFile>,
    #[serde(default)]
    pub warning_profiles: Vec<String>,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub gc_threshold: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "query")]
pub enum SemanticQuery {
    ExpressionType { file: String, start: u32, end: u32 },
    Entities { file: String, start: u32, end: u32 },
    References { file: String, start: u32, end: u32 },
    Signature { file: String, start: u32, end: u32 },
    TypeMembers { file: String, start: u32, end: u32 },
    ClosedCallErrors { file: String, start: u32, end: u32 },
    TypeFacts { file: String, start: u32, end: u32 },
    ExpressionFacts { file: String, start: u32, end: u32 },
    SemanticSnapshot { file: String },
    FormattedAst,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticAction {
    pub source: SourceAction,
    pub queries: Vec<SemanticQuery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryScenario {
    ReachableRoots,
    UnreachableCycles,
    SustainedPressure,
    RetryBeforeOom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    pub logical_path: String,
    pub contents: PinnedFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterminismAction {
    pub manifest: PinnedFile,
    pub lockfile: PinnedFile,
    pub inputs: Vec<BuildInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentAction {
    pub markdown: PinnedFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum CaseAction {
    Source(SourceAction),
    Semantic(SemanticAction),
    Memory { scenario: MemoryScenario },
    Determinism(DeterminismAction),
    Document(DocumentAction),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Expectation {
    Exact { observation: PinnedFile },
    OneOf { observations: PinnedFile },
}

impl Expectation {
    pub fn pinned_file(&self) -> &PinnedFile {
        match self {
            Self::Exact { observation } => observation,
            Self::OneOf { observations } => observations,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceCase {
    pub id: String,
    pub group: CaseGroup,
    pub target: String,
    pub profile: String,
    pub capabilities: Vec<String>,
    pub repeat: u32,
    #[serde(default)]
    pub covers: Vec<String>,
    #[serde(default)]
    pub positive_for: Vec<String>,
    #[serde(default)]
    pub requirements: Vec<String>,
    pub action: CaseAction,
    pub expectation: Expectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteManifest {
    pub format: String,
    pub suite: String,
    pub version: String,
    pub edition: String,
    pub adapter_protocol: String,
    pub specification: PinnedFile,
    pub fixture_manifest: PinnedFile,
    pub registry: NormativeRegistry,
    pub targets: Vec<TargetDeclaration>,
    pub cases: Vec<ConformanceCase>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        message: String,
    },
    Json(String),
    Invalid(String),
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "cannot read `{}`: {message}", path.display())
            }
            Self::Json(message) => write!(formatter, "invalid suite manifest JSON: {message}"),
            Self::Invalid(message) => write!(formatter, "invalid suite manifest: {message}"),
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "pinned file `{path}` has SHA-256 `{actual}`, expected `{expected}`"
            ),
        }
    }
}

impl Error for ManifestError {}

#[derive(Debug)]
pub struct LoadedSuite {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest_bytes: Vec<u8>,
    manifest: SuiteManifest,
    pinned: BTreeMap<String, Vec<u8>>,
}

impl LoadedSuite {
    pub fn load(
        root: impl Into<PathBuf>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Self, ManifestError> {
        Self::load_with_overrides(root, manifest_path, BTreeMap::new())
    }

    pub fn load_with_overrides(
        root: impl Into<PathBuf>,
        manifest_path: impl AsRef<Path>,
        mut overrides: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, ManifestError> {
        let root = root.into();
        let manifest_path = manifest_path.as_ref().to_path_buf();
        let absolute_manifest = root.join(&manifest_path);
        let manifest_bytes = fs::read(&absolute_manifest).map_err(|error| ManifestError::Io {
            path: absolute_manifest,
            message: error.to_string(),
        })?;
        let manifest: SuiteManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| ManifestError::Json(error.to_string()))?;
        validate_manifest(&manifest)?;

        let canonical = serde_json::to_vec(&manifest)
            .map_err(|error| ManifestError::Json(error.to_string()))?;
        if canonical != manifest_bytes {
            return Err(ManifestError::Invalid(
                "the manifest is not in canonical compact JSON encoding".into(),
            ));
        }

        let mut pinned: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for file in referenced_files(&manifest) {
            validate_pinned_file(&file)?;
            if let Some(previous) = pinned.get(&file.path) {
                let previous_hash = sha256(previous);
                if previous_hash != file.sha256 {
                    return Err(ManifestError::Invalid(format!(
                        "path `{}` is pinned with conflicting hashes",
                        file.path
                    )));
                }
                continue;
            }
            let bytes = if let Some(bytes) = overrides.remove(&file.path) {
                bytes
            } else {
                let physical = root.join(&file.path);
                fs::read(&physical).map_err(|error| ManifestError::Io {
                    path: physical,
                    message: error.to_string(),
                })?
            };
            let actual = sha256(&bytes);
            if actual != file.sha256 {
                return Err(ManifestError::HashMismatch {
                    path: file.path,
                    expected: file.sha256,
                    actual,
                });
            }
            pinned.insert(file.path, bytes);
        }
        if let Some(path) = overrides.keys().next() {
            return Err(ManifestError::Invalid(format!(
                "override path `{path}` is not referenced by the suite manifest"
            )));
        }

        validate_expectations(&manifest, &pinned)?;
        Ok(Self {
            root,
            manifest_path,
            manifest_bytes,
            manifest,
            pinned,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn manifest(&self) -> &SuiteManifest {
        &self.manifest
    }

    pub fn manifest_sha256(&self) -> String {
        sha256(&self.manifest_bytes)
    }

    pub fn file(&self, pinned: &PinnedFile) -> &[u8] {
        self.pinned
            .get(&pinned.path)
            .expect("all manifest references are loaded")
    }

    pub fn json_file(&self, pinned: &PinnedFile) -> Result<Value, ManifestError> {
        serde_json::from_slice(self.file(pinned))
            .map_err(|error| ManifestError::Json(format!("{}: {error}", pinned.path)))
    }
}

fn validate_manifest(manifest: &SuiteManifest) -> Result<(), ManifestError> {
    if manifest.format != SUITE_FORMAT {
        return invalid(format!("unsupported format `{}`", manifest.format));
    }
    if manifest.suite != SUITE_NAME {
        return invalid(format!("unsupported suite `{}`", manifest.suite));
    }
    if manifest.version != "draft" {
        return invalid(format!("unsupported suite state `{}`", manifest.version));
    }
    if manifest.edition != "0.1" {
        return invalid(format!(
            "unsupported language edition `{}`",
            manifest.edition
        ));
    }
    if manifest.adapter_protocol != ADAPTER_PROTOCOL {
        return invalid(format!(
            "unsupported adapter protocol `{}`",
            manifest.adapter_protocol
        ));
    }
    validate_registry(&manifest.registry)?;
    require_sorted_unique(
        "target declarations",
        manifest.targets.iter().map(|target| target.name.as_str()),
    )?;
    let mut target_profiles = BTreeMap::new();
    for target in &manifest.targets {
        validate_identity("target name", &target.name)?;
        validate_identity("target profile", &target.profile)?;
        require_sorted_unique(
            "target capabilities",
            target.capabilities.iter().map(String::as_str),
        )?;
        target_profiles.insert(target.name.as_str(), target);
    }
    if manifest.targets.is_empty() {
        return invalid("at least one target declaration is required");
    }
    require_sorted_unique(
        "case IDs",
        manifest.cases.iter().map(|case| case.id.as_str()),
    )?;
    if manifest.cases.is_empty() {
        return invalid("at least one conformance case is required");
    }
    for case in &manifest.cases {
        validate_case(case, &manifest.registry, &target_profiles)?;
    }
    validate_draft_contract(manifest)?;
    Ok(())
}

fn validate_draft_contract(manifest: &SuiteManifest) -> Result<(), ManifestError> {
    let [target] = manifest.targets.as_slice() else {
        return invalid("the draft must declare exactly one conformance target");
    };
    if target.name != "tondo-vm-hosted"
        || target.profile != "hosted"
        || target
            .capabilities
            .iter()
            .map(String::as_str)
            .ne(["console", "process"])
    {
        return invalid("the draft target must be tondo-vm-hosted/hosted with [console, process]");
    }

    let groups = manifest
        .cases
        .iter()
        .map(|case| case.group)
        .collect::<BTreeSet<_>>();
    for group in [
        CaseGroup::LexParseFormat,
        CaseGroup::CompilePass,
        CaseGroup::CompileFail,
        CaseGroup::SemanticQueries,
        CaseGroup::Runtime,
        CaseGroup::Concurrency,
        CaseGroup::Hosted,
        CaseGroup::Memory,
        CaseGroup::Determinism,
        CaseGroup::Documentation,
    ] {
        if !groups.contains(&group) {
            return invalid(format!("draft suite has no {group:?} cases"));
        }
    }

    let requirements = manifest
        .cases
        .iter()
        .flat_map(|case| case.requirements.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    for requirement in [
        "CONF-002",
        "CONF-003",
        "CONF-004",
        "CONF-005",
        "CONF-006",
        "CONF-007",
        "CONF-008",
        "CONF-009",
        "CONF-010",
        "CONC-CONF-001",
        "DETERMINISM-001",
        "FMT-CONF-001",
        "MEM-CONF-001",
        "QUERY-CONF-001",
    ] {
        if !requirements.contains(requirement) {
            return invalid(format!(
                "draft suite has no case for requirement `{requirement}`"
            ));
        }
    }

    let covered = manifest
        .cases
        .iter()
        .flat_map(|case| case.covers.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let positive = manifest
        .cases
        .iter()
        .flat_map(|case| case.positive_for.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    for code in &manifest.registry.errors {
        if !covered.contains(code.as_str()) {
            return invalid(format!("draft suite does not cover `{code}`"));
        }
        if !positive.contains(code.as_str()) {
            return invalid(format!("draft suite has no positive neighbor for `{code}`"));
        }
    }
    for code in &manifest.registry.panics {
        if !covered.contains(code.as_str()) {
            return invalid(format!("draft suite does not cover `{code}`"));
        }
    }

    let core_warning_codes = [
        "W1001", "W1002", "W1003", "W1004", "W1005", "W1006", "W1007", "W1008", "W1011",
    ];
    let core_covered = manifest
        .cases
        .iter()
        .filter(|case| {
            case_source_action(case)
                .is_some_and(|action| action.warning_profiles.iter().any(|name| name == "core"))
        })
        .flat_map(|case| case.covers.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    for code in core_warning_codes {
        if !core_covered.contains(code) {
            return invalid(format!(
                "core warning profile has no conformance case for `{code}`"
            ));
        }
    }

    let mut omitted_capabilities = BTreeSet::new();
    let mut memory_scenarios = BTreeSet::new();
    let mut determinism_cases = 0;
    let mut documentation_cases = 0;
    for case in &manifest.cases {
        let group_requirements: &[&str] = match case.group {
            CaseGroup::LexParseFormat => &["CONF-004", "FMT-CONF-001"],
            CaseGroup::CompilePass | CaseGroup::CompileFail => &["CONF-005"],
            CaseGroup::SemanticQueries => &["CONF-006", "QUERY-CONF-001"],
            CaseGroup::Runtime => &["CONF-007"],
            CaseGroup::Concurrency => &["CONC-CONF-001", "CONF-008"],
            CaseGroup::Hosted => &["CONF-009"],
            CaseGroup::Memory => &["CONF-010", "MEM-CONF-001"],
            CaseGroup::Determinism => &["DETERMINISM-001"],
            CaseGroup::Documentation => &["CONF-002", "CONF-003"],
        };
        for requirement in group_requirements {
            if !case.requirements.iter().any(|actual| actual == requirement) {
                return invalid(format!(
                    "case `{}` lacks group requirement `{requirement}`",
                    case.id
                ));
            }
        }

        if case.capabilities != target.capabilities {
            if !case.covers.iter().any(|code| code == "E1008")
                || !case
                    .requirements
                    .iter()
                    .any(|requirement| requirement == "CONF-009")
            {
                return invalid(format!(
                    "case `{}` omits a target capability without an E1008 boundary proof",
                    case.id
                ));
            }
            omitted_capabilities.extend(
                target
                    .capabilities
                    .iter()
                    .filter(|capability| !case.capabilities.contains(capability))
                    .cloned(),
            );
        }
        if case.group == CaseGroup::Concurrency && case.repeat < 32 {
            return invalid(format!(
                "concurrency case `{}` must run at least 32 calibrated repetitions",
                case.id
            ));
        }

        match (&case.group, &case.action) {
            (
                CaseGroup::Runtime | CaseGroup::Concurrency | CaseGroup::Hosted,
                CaseAction::Source(action),
            ) if action.operation == SourceOperation::Run => {}
            (
                CaseGroup::Runtime | CaseGroup::Concurrency | CaseGroup::Hosted,
                CaseAction::Source(_),
            ) => {
                return invalid(format!("executable case `{}` must run", case.id));
            }
            (CaseGroup::CompilePass, CaseAction::Source(action))
                if action.operation == SourceOperation::Check => {}
            (CaseGroup::CompilePass, CaseAction::Source(_)) => {
                return invalid(format!("compile-pass case `{}` must check", case.id));
            }
            (CaseGroup::Memory, CaseAction::Memory { scenario }) => {
                memory_scenarios.insert(match scenario {
                    MemoryScenario::ReachableRoots => "reachable-roots",
                    MemoryScenario::UnreachableCycles => "unreachable-cycles",
                    MemoryScenario::SustainedPressure => "sustained-pressure",
                    MemoryScenario::RetryBeforeOom => "retry-before-oom",
                });
            }
            (CaseGroup::Determinism, CaseAction::Determinism(_)) => {
                determinism_cases += 1;
                if case.repeat < 3 {
                    return invalid("determinism case must run at least three repetitions");
                }
            }
            (CaseGroup::Documentation, CaseAction::Document(_)) => {
                documentation_cases += 1;
            }
            _ => {}
        }
    }
    if omitted_capabilities != target.capabilities.iter().cloned().collect::<BTreeSet<_>>() {
        return invalid("draft suite does not prove every absent capability boundary");
    }
    if memory_scenarios
        != [
            "reachable-roots",
            "retry-before-oom",
            "sustained-pressure",
            "unreachable-cycles",
        ]
        .into_iter()
        .collect()
        || manifest
            .cases
            .iter()
            .filter(|case| case.group == CaseGroup::Memory)
            .count()
            != 4
    {
        return invalid("draft suite must contain each private memory scenario exactly once");
    }
    if determinism_cases != 1 {
        return invalid("draft suite must contain exactly one closed-project determinism case");
    }
    if documentation_cases != 1 {
        return invalid("draft suite must contain exactly one normative documentation case");
    }
    if !manifest.cases.iter().any(|case| {
        case.group == CaseGroup::LexParseFormat
            && matches!(
                &case.action,
                CaseAction::Source(SourceAction {
                    operation: SourceOperation::Format,
                    ..
                })
            )
    }) {
        return invalid("draft suite has no formatter case");
    }
    Ok(())
}

fn case_source_action(case: &ConformanceCase) -> Option<&SourceAction> {
    match &case.action {
        CaseAction::Source(action) => Some(action),
        CaseAction::Semantic(action) => Some(&action.source),
        CaseAction::Memory { .. } | CaseAction::Determinism(_) | CaseAction::Document(_) => None,
    }
}

fn validate_registry(registry: &NormativeRegistry) -> Result<(), ManifestError> {
    for (name, prefix, values) in [
        ("error registry", b'E', &registry.errors),
        ("warning registry", b'W', &registry.warnings),
        ("panic registry", b'P', &registry.panics),
    ] {
        require_sorted_unique(name, values.iter().map(String::as_str))?;
        if values.is_empty() {
            return invalid(format!("{name} cannot be empty"));
        }
        for value in values {
            validate_code(value, prefix)?;
        }
    }
    Ok(())
}

fn validate_case(
    case: &ConformanceCase,
    registry: &NormativeRegistry,
    targets: &BTreeMap<&str, &TargetDeclaration>,
) -> Result<(), ManifestError> {
    validate_case_id(&case.id)?;
    let target = targets
        .get(case.target.as_str())
        .ok_or_else(|| ManifestError::Invalid(format!("case `{}` uses unknown target", case.id)))?;
    if case.profile != target.profile {
        return invalid(format!(
            "case `{}` selects profile `{}` instead of target profile `{}`",
            case.id, case.profile, target.profile
        ));
    }
    require_sorted_unique(
        "case capabilities",
        case.capabilities.iter().map(String::as_str),
    )?;
    for capability in &case.capabilities {
        if !target.capabilities.contains(capability) {
            return invalid(format!(
                "case `{}` selects unsupported capability `{capability}`",
                case.id
            ));
        }
    }
    if case.repeat == 0 || case.repeat > 100_000 {
        return invalid(format!("case `{}` repeat must be in 1..=100000", case.id));
    }
    for (name, values) in [
        ("covers", &case.covers),
        ("positive_for", &case.positive_for),
        ("requirements", &case.requirements),
    ] {
        require_sorted_unique(
            &format!("case `{}` {name}", case.id),
            values.iter().map(String::as_str),
        )?;
    }
    let known_codes = registry
        .errors
        .iter()
        .chain(&registry.warnings)
        .chain(&registry.panics)
        .collect::<BTreeSet<_>>();
    for code in case.covers.iter().chain(&case.positive_for) {
        if !known_codes.contains(code) {
            return invalid(format!(
                "case `{}` references unknown normative code `{code}`",
                case.id
            ));
        }
    }
    validate_case_action(case)?;
    Ok(())
}

fn validate_case_action(case: &ConformanceCase) -> Result<(), ManifestError> {
    match &case.action {
        CaseAction::Source(action) => {
            if matches!(
                case.group,
                CaseGroup::SemanticQueries
                    | CaseGroup::Memory
                    | CaseGroup::Determinism
                    | CaseGroup::Documentation
            ) {
                return invalid(format!(
                    "source case `{}` cannot belong to the {:?} group",
                    case.id, case.group
                ));
            }
            validate_source_action(&case.id, action)
        }
        CaseAction::Semantic(action) => {
            if case.group != CaseGroup::SemanticQueries {
                return invalid(format!(
                    "semantic case `{}` must belong to the semantic-queries group",
                    case.id
                ));
            }
            validate_source_action(&case.id, &action.source)?;
            if action.source.operation != SourceOperation::Check {
                return invalid(format!(
                    "semantic case `{}` must use the check operation",
                    case.id
                ));
            }
            if action.queries.is_empty() {
                return invalid(format!(
                    "semantic case `{}` must declare at least one query",
                    case.id
                ));
            }
            Ok(())
        }
        CaseAction::Memory { .. } => {
            if case.group != CaseGroup::Memory {
                return invalid(format!(
                    "memory case `{}` must belong to the memory group",
                    case.id
                ));
            }
            Ok(())
        }
        CaseAction::Determinism(action) => {
            if case.group != CaseGroup::Determinism {
                return invalid(format!(
                    "determinism case `{}` must belong to the determinism group",
                    case.id
                ));
            }
            if action.inputs.is_empty() {
                return invalid(format!(
                    "determinism case `{}` must declare project inputs",
                    case.id
                ));
            }
            require_sorted_unique(
                &format!("determinism case `{}` input paths", case.id),
                action
                    .inputs
                    .iter()
                    .map(|input| input.logical_path.as_str()),
            )?;
            Ok(())
        }
        CaseAction::Document(_) => {
            if case.group != CaseGroup::Documentation {
                return invalid(format!(
                    "document case `{}` must belong to the documentation group",
                    case.id
                ));
            }
            Ok(())
        }
    }
}

fn validate_source_action(case_id: &str, action: &SourceAction) -> Result<(), ManifestError> {
    if action.sources.is_empty() {
        return invalid(format!("source case `{case_id}` has no source files"));
    }
    require_sorted_unique(
        &format!("source case `{case_id}` logical paths"),
        action
            .sources
            .iter()
            .map(|source| source.logical_path.as_str()),
    )?;
    if !action
        .sources
        .iter()
        .any(|source| source.logical_path == action.root)
    {
        return invalid(format!(
            "source case `{case_id}` root `{}` is not one of its sources",
            action.root
        ));
    }
    for source in &action.sources {
        validate_identity("source ID", &source.source_id)?;
        validate_identity("module", &source.module)?;
        validate_logical_path(&source.logical_path)?;
    }
    require_sorted_unique(
        &format!("source case `{case_id}` warning profiles"),
        action.warning_profiles.iter().map(String::as_str),
    )?;
    for profile in &action.warning_profiles {
        if profile != "core" {
            return invalid(format!(
                "source case `{case_id}` selects unknown warning profile `{profile}`"
            ));
        }
    }
    if action.operation != SourceOperation::Run && !action.arguments.is_empty() {
        return invalid(format!(
            "source case `{case_id}` supplies arguments to a non-run operation"
        ));
    }
    if action.gc_threshold == Some(0) {
        return invalid(format!("source case `{case_id}` has a zero GC threshold"));
    }
    Ok(())
}

fn validate_expectations(
    manifest: &SuiteManifest,
    pinned: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ManifestError> {
    for case in &manifest.cases {
        let file = case.expectation.pinned_file();
        let bytes = &pinned[&file.path];
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| ManifestError::Json(format!("{}: {error}", file.path)))?;
        match &case.expectation {
            Expectation::Exact { .. } if !value.is_object() => {
                return invalid(format!(
                    "case `{}` exact expectation must be one observation object",
                    case.id
                ));
            }
            Expectation::OneOf { .. }
                if value
                    .as_array()
                    .is_none_or(|observations| observations.is_empty()) =>
            {
                return invalid(format!(
                    "case `{}` one-of expectation must be a non-empty array",
                    case.id
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn validate_embedded_suite(
    manifest: &SuiteManifest,
    manifest_bytes: &[u8],
    pinned: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ManifestError> {
    validate_manifest(manifest)?;
    let canonical =
        serde_json::to_vec(manifest).map_err(|error| ManifestError::Json(error.to_string()))?;
    if canonical != manifest_bytes {
        return invalid("the embedded manifest is not in canonical compact JSON encoding");
    }

    let mut expected = BTreeMap::new();
    for file in referenced_files(manifest) {
        validate_pinned_file(&file)?;
        if let Some(previous) = expected.insert(file.path.clone(), file.sha256.clone())
            && previous != file.sha256
        {
            return invalid(format!(
                "path `{}` is pinned with conflicting hashes",
                file.path
            ));
        }
    }
    if pinned.keys().ne(expected.keys()) {
        return invalid("the embedded suite object closure differs from its manifest");
    }
    for (path, expected_hash) in expected {
        let actual = sha256(&pinned[&path]);
        if actual != expected_hash {
            return Err(ManifestError::HashMismatch {
                path,
                expected: expected_hash,
                actual,
            });
        }
    }
    validate_expectations(manifest, pinned)
}

pub(crate) fn referenced_files(manifest: &SuiteManifest) -> Vec<PinnedFile> {
    let mut files = vec![
        manifest.specification.clone(),
        manifest.fixture_manifest.clone(),
    ];
    for case in &manifest.cases {
        files.push(case.expectation.pinned_file().clone());
        match &case.action {
            CaseAction::Source(action) => push_source_files(&mut files, action),
            CaseAction::Semantic(action) => push_source_files(&mut files, &action.source),
            CaseAction::Memory { .. } => {}
            CaseAction::Determinism(action) => {
                files.push(action.manifest.clone());
                files.push(action.lockfile.clone());
                files.extend(action.inputs.iter().map(|input| input.contents.clone()));
            }
            CaseAction::Document(action) => files.push(action.markdown.clone()),
        }
    }
    files
}

fn push_source_files(files: &mut Vec<PinnedFile>, action: &SourceAction) {
    files.extend(action.sources.iter().map(|source| source.contents.clone()));
}

fn validate_pinned_file(file: &PinnedFile) -> Result<(), ManifestError> {
    validate_logical_path(&file.path)?;
    validate_hash(&file.sha256)
}

fn validate_hash(value: &str) -> Result<(), ManifestError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!("invalid lowercase SHA-256 `{value}`"));
    }
    Ok(())
}

fn validate_code(value: &str, prefix: u8) -> Result<(), ManifestError> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[0] != prefix || !bytes[1..].iter().all(u8::is_ascii_digit) {
        return invalid(format!("invalid normative code `{value}`"));
    }
    Ok(())
}

fn validate_case_id(value: &str) -> Result<(), ManifestError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_' | b'.')
                })
        })
    {
        return invalid(format!("invalid case ID `{value}`"));
    }
    Ok(())
}

fn validate_identity(name: &str, value: &str) -> Result<(), ManifestError> {
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return invalid(format!("{name} must be non-empty and single-line"));
    }
    Ok(())
}

fn validate_logical_path(value: &str) -> Result<(), ManifestError> {
    if value.is_empty()
        || value.contains('\\')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.split('/').any(|component| component.is_empty())
    {
        return invalid(format!("invalid logical path `{value}`"));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || component.as_os_str().to_str().is_none_or(|part| {
                    part.is_empty() || part.contains('\n') || part.contains('\r')
                })
        })
    {
        return invalid(format!("invalid logical path `{value}`"));
    }
    Ok(())
}

fn require_sorted_unique<'a>(
    name: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), ManifestError> {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invalid(format!("{name} must be sorted and unique"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ManifestError> {
    Err(ManifestError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(path: &str) -> PinnedFile {
        PinnedFile {
            path: path.into(),
            sha256: "a".repeat(64),
        }
    }

    fn source_action(operation: SourceOperation) -> SourceAction {
        SourceAction {
            operation,
            form: SourceForm::Module,
            root: "main.to".into(),
            sources: vec![SourceFile {
                source_id: "test:main".into(),
                module: "main".into(),
                logical_path: "main.to".into(),
                contents: pinned("main.to"),
            }],
            warning_profiles: Vec::new(),
            arguments: Vec::new(),
            gc_threshold: None,
        }
    }

    fn conformance_case(group: CaseGroup, action: CaseAction) -> ConformanceCase {
        ConformanceCase {
            id: "test/case".into(),
            group,
            target: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
            repeat: 1,
            covers: Vec::new(),
            positive_for: Vec::new(),
            requirements: Vec::new(),
            action,
            expectation: Expectation::Exact {
                observation: pinned("expected.json"),
            },
        }
    }

    fn manifest_with_case(case: ConformanceCase) -> SuiteManifest {
        SuiteManifest {
            format: SUITE_FORMAT.into(),
            suite: SUITE_NAME.into(),
            version: "draft".into(),
            edition: "0.1".into(),
            adapter_protocol: ADAPTER_PROTOCOL.into(),
            specification: pinned("spec.md"),
            fixture_manifest: pinned("fixtures.json"),
            registry: NormativeRegistry {
                errors: vec!["E0001".into()],
                warnings: vec!["W0001".into()],
                panics: vec!["P0001".into()],
            },
            targets: vec![TargetDeclaration {
                name: "tondo-vm-hosted".into(),
                profile: "hosted".into(),
                capabilities: vec!["console".into(), "process".into()],
            }],
            cases: vec![case],
        }
    }

    fn invalid_message<T>(result: Result<T, ManifestError>) -> String {
        match result {
            Err(ManifestError::Invalid(message)) => message,
            Err(other) => panic!("expected an invalid-manifest error, got {other}"),
            Ok(_) => panic!("expected validation to fail"),
        }
    }

    #[test]
    fn paths_and_case_ids_are_closed() {
        for path in ["cases/main.to", "TONDO_LANGUAGE_SPEC.md"] {
            validate_logical_path(path).unwrap();
        }
        for path in ["", "/root", "../escape", "a//b", "a\\b", "./a"] {
            assert!(validate_logical_path(path).is_err(), "{path}");
        }
        for id in ["compile/e1001", "runtime/p0001-bounds"] {
            validate_case_id(id).unwrap();
        }
        for id in ["", "/case", "Case", "case/", "case//nested", "case space"] {
            assert!(validate_case_id(id).is_err(), "{id}");
        }
    }

    #[test]
    fn hashes_and_codes_are_exact() {
        validate_hash(&"a".repeat(64)).unwrap();
        assert!(validate_hash(&"A".repeat(64)).is_err());
        assert!(validate_hash(&"a".repeat(63)).is_err());
        validate_code("E0001", b'E').unwrap();
        assert!(validate_code("W0001", b'E').is_err());
    }

    #[test]
    fn case_actions_reject_every_cross_group_and_incomplete_shape() {
        let source = source_action(SourceOperation::Check);
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Memory,
                CaseAction::Source(source.clone()),
            )))
            .contains("source case")
        );

        let semantic = SemanticAction {
            source: source.clone(),
            queries: vec![SemanticQuery::FormattedAst],
        };
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::CompilePass,
                CaseAction::Semantic(semantic.clone()),
            )))
            .contains("semantic-queries")
        );
        let mut wrong_operation = semantic.clone();
        wrong_operation.source.operation = SourceOperation::Run;
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::SemanticQueries,
                CaseAction::Semantic(wrong_operation),
            )))
            .contains("check operation")
        );
        let mut no_queries = semantic;
        no_queries.queries.clear();
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::SemanticQueries,
                CaseAction::Semantic(no_queries),
            )))
            .contains("at least one query")
        );

        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Runtime,
                CaseAction::Memory {
                    scenario: MemoryScenario::ReachableRoots,
                },
            )))
            .contains("memory group")
        );
        let determinism = DeterminismAction {
            manifest: pinned("project/tondo.json"),
            lockfile: pinned("project/tondo.lock.json"),
            inputs: vec![BuildInput {
                logical_path: "src/main.to".into(),
                contents: pinned("project/src/main.to"),
            }],
        };
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Runtime,
                CaseAction::Determinism(determinism.clone()),
            )))
            .contains("determinism group")
        );
        let mut empty = determinism.clone();
        empty.inputs.clear();
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Determinism,
                CaseAction::Determinism(empty),
            )))
            .contains("project inputs")
        );
        let mut duplicate = determinism;
        duplicate.inputs.push(duplicate.inputs[0].clone());
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Determinism,
                CaseAction::Determinism(duplicate),
            )))
            .contains("sorted and unique")
        );
        assert!(
            invalid_message(validate_case_action(&conformance_case(
                CaseGroup::Runtime,
                CaseAction::Document(DocumentAction {
                    markdown: pinned("spec.md"),
                }),
            )))
            .contains("documentation group")
        );
    }

    #[test]
    fn source_and_case_validation_close_each_user_controlled_boundary() {
        let mut action = source_action(SourceOperation::Check);
        action.sources.clear();
        assert!(
            invalid_message(validate_source_action("test/case", &action)).contains("no source")
        );

        let mut action = source_action(SourceOperation::Check);
        action.sources.push(action.sources[0].clone());
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("sorted and unique")
        );
        let mut action = source_action(SourceOperation::Check);
        action.root = "missing.to".into();
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("not one of its sources")
        );
        let mut action = source_action(SourceOperation::Check);
        action.sources[0].source_id.clear();
        assert!(
            invalid_message(validate_source_action("test/case", &action)).contains("non-empty")
        );
        let mut action = source_action(SourceOperation::Check);
        action.warning_profiles = vec!["core".into(), "core".into()];
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("sorted and unique")
        );
        let mut action = source_action(SourceOperation::Check);
        action.warning_profiles = vec!["strict".into()];
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("unknown warning profile")
        );
        let mut action = source_action(SourceOperation::Check);
        action.arguments = vec!["argument".into()];
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("non-run operation")
        );
        let mut action = source_action(SourceOperation::Run);
        action.gc_threshold = Some(0);
        assert!(
            invalid_message(validate_source_action("test/case", &action))
                .contains("zero GC threshold")
        );

        let registry = NormativeRegistry {
            errors: vec!["E0001".into()],
            warnings: vec!["W0001".into()],
            panics: vec!["P0001".into()],
        };
        let target = TargetDeclaration {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        };
        let targets = BTreeMap::from([(target.name.as_str(), &target)]);
        let base = conformance_case(
            CaseGroup::CompilePass,
            CaseAction::Source(source_action(SourceOperation::Check)),
        );
        let mut mutations: Vec<(ConformanceCase, &str)> = Vec::new();
        let mut case = base.clone();
        case.target = "unknown".into();
        mutations.push((case, "unknown target"));
        let mut case = base.clone();
        case.profile = "sandboxed".into();
        mutations.push((case, "selects profile"));
        let mut case = base.clone();
        case.capabilities = vec!["console".into(), "console".into()];
        mutations.push((case, "sorted and unique"));
        let mut case = base.clone();
        case.capabilities = vec!["network".into()];
        mutations.push((case, "unsupported capability"));
        let mut case = base.clone();
        case.repeat = 0;
        mutations.push((case, "repeat must be"));
        let mut case = base.clone();
        case.covers = vec!["E0001".into(), "E0001".into()];
        mutations.push((case, "sorted and unique"));
        let mut case = base;
        case.covers = vec!["E9999".into()];
        mutations.push((case, "unknown normative code"));

        for (case, expected) in mutations {
            assert!(
                invalid_message(validate_case(&case, &registry, &targets)).contains(expected),
                "mutation should report {expected}"
            );
        }
    }

    #[test]
    fn expectations_accessors_and_error_text_are_exact() {
        let exact = Expectation::Exact {
            observation: pinned("exact.json"),
        };
        let one_of = Expectation::OneOf {
            observations: pinned("one-of.json"),
        };
        assert_eq!(exact.pinned_file().path, "exact.json");
        assert_eq!(one_of.pinned_file().path, "one-of.json");

        let case = conformance_case(
            CaseGroup::CompilePass,
            CaseAction::Source(source_action(SourceOperation::Check)),
        );
        let manifest = manifest_with_case(case.clone());
        let pinned_files = BTreeMap::from([("expected.json".into(), b"null".to_vec())]);
        assert!(
            invalid_message(validate_expectations(&manifest, &pinned_files))
                .contains("one observation object")
        );
        let mut one_of_case = case;
        one_of_case.expectation = Expectation::OneOf {
            observations: pinned("expected.json"),
        };
        let manifest = manifest_with_case(one_of_case);
        let pinned_files = BTreeMap::from([("expected.json".into(), b"[]".to_vec())]);
        assert!(
            invalid_message(validate_expectations(&manifest, &pinned_files))
                .contains("non-empty array")
        );

        assert_eq!(
            ManifestError::Io {
                path: PathBuf::from("missing.json"),
                message: "not found".into(),
            }
            .to_string(),
            "cannot read `missing.json`: not found"
        );
        assert_eq!(
            ManifestError::Json("bad token".into()).to_string(),
            "invalid suite manifest JSON: bad token"
        );
        assert_eq!(
            ManifestError::Invalid("bad shape".into()).to_string(),
            "invalid suite manifest: bad shape"
        );
        assert_eq!(
            ManifestError::HashMismatch {
                path: "source.to".into(),
                expected: "expected".into(),
                actual: "actual".into(),
            }
            .to_string(),
            "pinned file `source.to` has SHA-256 `actual`, expected `expected`"
        );
    }

    #[test]
    fn baseline_suite_accessors_preserve_loaded_identity() {
        use crate::lineage::DraftLineage;

        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let lineage = DraftLineage::load(root, "conformance/draft/manifest.json").unwrap();
        let suite = lineage.baseline_suite();
        assert_eq!(suite.root(), root);
        assert_eq!(
            suite.manifest_path(),
            Path::new("conformance/0.1/manifest.json")
        );
        assert_eq!(suite.manifest_sha256().len(), 64);
        let expectation = suite.manifest().cases[0].expectation.pinned_file();
        let value = suite.json_file(expectation).unwrap();
        assert!(value.is_object() || value.is_array());

        let pinned = referenced_files(suite.manifest())
            .into_iter()
            .map(|file| (file.path.clone(), suite.file(&file).to_vec()))
            .collect::<BTreeMap<_, _>>();
        validate_embedded_suite(suite.manifest(), &suite.manifest_bytes, &pinned).unwrap();
        let mut extra = pinned.clone();
        extra.insert("unreferenced.txt".into(), Vec::new());
        assert!(validate_embedded_suite(suite.manifest(), &suite.manifest_bytes, &extra).is_err());
        let mut noncanonical = suite.manifest_bytes.clone();
        noncanonical.push(b'\n');
        assert!(validate_embedded_suite(suite.manifest(), &noncanonical, &pinned).is_err());
        let mut invalid = suite.manifest().clone();
        invalid.cases[0].repeat = 0;
        assert!(validate_embedded_suite(&invalid, &suite.manifest_bytes, &pinned).is_err());

        assert!(matches!(
            LoadedSuite::load(root, "conformance/0.1/manifest.json"),
            Err(ManifestError::HashMismatch { path, .. }) if path == "TONDO_LANGUAGE_SPEC.md"
        ));

        let overrides = BTreeMap::from([
            (
                "TONDO_LANGUAGE_SPEC.md".into(),
                lineage.baseline_specification().to_vec(),
            ),
            ("unreferenced.txt".into(), b"not part of the suite".to_vec()),
        ]);
        assert!(matches!(
            LoadedSuite::load_with_overrides(
                root,
                "conformance/0.1/manifest.json",
                overrides
            ),
            Err(ManifestError::Invalid(message))
                if message.contains("override path `unreferenced.txt` is not referenced")
        ));
    }
}
