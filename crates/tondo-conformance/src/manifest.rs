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
            let physical = root.join(&file.path);
            let bytes = fs::read(&physical).map_err(|error| ManifestError::Io {
                path: physical,
                message: error.to_string(),
            })?;
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
    if manifest.version != "0.1.0" {
        return invalid(format!("unsupported suite version `{}`", manifest.version));
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
    Ok(())
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
        CaseAction::Source(action) => validate_source_action(&case.id, action),
        CaseAction::Semantic(action) => {
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

fn referenced_files(manifest: &SuiteManifest) -> Vec<PinnedFile> {
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
}
