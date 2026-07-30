//! Validated toolchain format `0.1/2`.
//!
//! The bootstrap compiler still owns the historical `/1` reader in
//! [`crate::project`] and [`crate::artifact`].  This module is the explicit
//! reader for the draft `/2` contracts: it has no filesystem, process, or
//! generation side effects and can therefore be used by a closed orchestrator
//! before any source is lexed.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifact::{CAPABILITY_REGISTRY, COMPILER_ID, validate_sha256};
use crate::package::{Name, PackageId};
use crate::source::{LogicalPath, ModulePath};

pub const MANIFEST_FORMAT: &str = "tondo-manifest-0.1/2";
pub const LOCKFILE_FORMAT: &str = "tondo-lock-0.1/2";
pub const INTERFACE_FORMAT: &str = "tondo-interface-0.1/2";
pub const ARTIFACT_FORMAT: &str = "tondo-artifact-0.1/2";
pub const STANDARD_DESCRIPTOR_FORMAT: &str = "tondo-standard-descriptor-0.1/1";
pub const PRIVILEGED_UNIT_FORMAT: &str = "tondo-privileged-unit-0.1/1";
pub const META_MODEL: &str = "tondo-meta-model-0.1/1";
pub const META_TARGET: &str = "tondo-meta";
pub const META_PROFILE: &str = "meta";

const CAPABILITIES: &[&str] = &[
    "clock",
    "console",
    "dynamic-linking",
    "entropy",
    "environment",
    "filesystem",
    "network",
    "process",
    "threads",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    Json(String),
    UnsupportedFormat {
        expected: &'static str,
        actual: String,
    },
    Invalid(String),
    NonCanonical(&'static str),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(message) => write!(f, "invalid toolchain JSON: {message}"),
            Self::UnsupportedFormat { expected, actual } => {
                write!(f, "expected format `{expected}`, found `{actual}`")
            }
            Self::Invalid(message) => f.write_str(message),
            Self::NonCanonical(kind) => write!(f, "{kind} is not canonically encoded"),
        }
    }
}

impl Error for FormatError {}

fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, FormatError> {
    serde_json::from_slice(bytes).map_err(|e| FormatError::Json(e.to_string()))
}

fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, FormatError> {
    serde_json::to_vec(value).map_err(|e| FormatError::Json(e.to_string()))
}

fn decode_canonical<T>(bytes: &[u8], kind: &'static str) -> Result<T, FormatError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let value: T = decode(bytes)?;
    if encode(&value)? != bytes {
        return Err(FormatError::NonCanonical(kind));
    }
    Ok(value)
}

fn sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn hash(value: &impl Serialize) -> Result<String, FormatError> {
    Ok(sha256(&encode(value)?))
}

fn require_hash(value: &str, field: &str) -> Result<(), FormatError> {
    validate_sha256(value)
        .map_err(|e| FormatError::Invalid(format!("{field} is not a SHA-256 identity: {e}")))
}

fn require_nonempty(field: &str, value: &str) -> Result<(), FormatError> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(FormatError::Invalid(format!(
            "{field} is empty or contains a line break"
        )));
    }
    Ok(())
}

fn require_sorted_unique<T: Ord>(field: &str, values: &[T]) -> Result<(), FormatError> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(FormatError::Invalid(format!(
            "{field} must be sorted and unique"
        )));
    }
    Ok(())
}

fn require_unique<T: Ord + Clone>(field: &str, values: &[T]) -> Result<(), FormatError> {
    let mut sorted = values.to_vec();
    sorted.sort();
    if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(FormatError::Invalid(format!(
            "{field} contains a duplicate"
        )));
    }
    Ok(())
}

fn require_kebab(field: &str, value: &str) -> Result<(), FormatError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(FormatError::Invalid(format!("{field} is empty")));
    };
    if !first.is_ascii_lowercase()
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || value.ends_with('-')
    {
        return Err(FormatError::Invalid(format!(
            "{field} `{value}` is not kebab-case"
        )));
    }
    Ok(())
}

fn require_unit_id(field: &str, value: &str) -> Result<(), FormatError> {
    if value.is_empty()
        || value.split('.').any(|part| {
            part.is_empty()
                || part.starts_with('-')
                || part.ends_with('-')
                || !part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    {
        return Err(FormatError::Invalid(format!(
            "{field} `{value}` is not a valid unit ID"
        )));
    }
    Ok(())
}

fn require_package_id(field: &str, value: &str) -> Result<(), FormatError> {
    PackageId::new(value.to_owned())
        .map(|_| ())
        .map_err(|e| FormatError::Invalid(format!("{field}: {e}")))
}

fn require_name(field: &str, value: &str) -> Result<(), FormatError> {
    Name::new(value)
        .map(|_| ())
        .map_err(|e| FormatError::Invalid(format!("{field} `{value}` is invalid: {e}")))
}

fn require_module(field: &str, value: &str) -> Result<(), FormatError> {
    ModulePath::new(value)
        .map(|_| ())
        .map_err(|e| FormatError::Invalid(format!("{field} `{value}` is invalid: {e}")))
}

fn require_path(field: &str, value: &str, source: bool) -> Result<(), FormatError> {
    let normalized = LogicalPath::new(value)
        .map_err(|e| FormatError::Invalid(format!("{field} `{value}` is invalid: {e}")))?;
    if normalized.as_str() != value {
        return Err(FormatError::Invalid(format!(
            "{field} `{value}` is not NFC-normalized"
        )));
    }
    if source && !value.ends_with(".to") {
        return Err(FormatError::Invalid(format!("{field} must end in `.to`")));
    }
    if value.starts_with("@generated/") {
        return Err(FormatError::Invalid(format!(
            "{field} uses reserved `@generated/`"
        )));
    }
    Ok(())
}

fn require_generated_path(field: &str, value: &str) -> Result<(), FormatError> {
    let normalized = LogicalPath::new(value)
        .map_err(|e| FormatError::Invalid(format!("{field} `{value}` is invalid: {e}")))?;
    if normalized.as_str() != value || !value.ends_with(".to") {
        return Err(FormatError::Invalid(format!(
            "{field} must be an NFC-normalized `.to` path"
        )));
    }
    Ok(())
}

fn standard_meta_id(standard: &str) -> String {
    standard.replace(":std:", ":std-meta:")
}

fn require_capabilities(values: &[String]) -> Result<(), FormatError> {
    require_sorted_unique("capabilities", values)?;
    for value in values {
        if !CAPABILITIES.contains(&value.as_str()) {
            return Err(FormatError::Invalid(format!(
                "unknown capability `{value}`"
            )));
        }
    }
    Ok(())
}

fn require_identity_lists(value: &Lists) -> Result<(), FormatError> {
    require_capabilities(&value.capabilities)?;
    require_sorted_unique("features", &value.features)?;
    require_sorted_unique("source_sets", &value.source_sets)?;
    require_sorted_unique("modules", &value.modules)?;
    Ok(())
}

fn validate_compilation_target(
    target: &str,
    profile: &str,
    capabilities: &[String],
) -> Result<(), FormatError> {
    match (target, profile) {
        ("tondo-vm-hosted", "hosted") => {
            if capabilities
                .iter()
                .any(|capability| !matches!(capability.as_str(), "console" | "process"))
            {
                return Err(FormatError::Invalid(
                    "tondo-vm-hosted only implements console and process capabilities".into(),
                ));
            }
        }
        (META_TARGET, META_PROFILE) => {
            if !capabilities.is_empty() {
                return Err(FormatError::Invalid(
                    "tondo-meta cannot carry runtime capabilities".into(),
                ));
            }
        }
        _ => {
            return Err(FormatError::Invalid(format!(
                "unsupported compilation target/profile `{target}`/`{profile}`"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub name: String,
    pub profile: String,
    pub capability_registry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

impl Target {
    fn validate(&self) -> Result<(), FormatError> {
        require_nonempty("target.name", &self.name)?;
        require_nonempty("target.profile", &self.profile)?;
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(FormatError::Invalid(format!(
                "target uses capability registry `{}`, expected `{CAPABILITY_REGISTRY}`",
                self.capability_registry
            )));
        }
        require_unique("target capabilities", &self.capabilities)?;
        for capability in &self.capabilities {
            if !CAPABILITIES.contains(&capability.as_str()) {
                return Err(FormatError::Invalid(format!(
                    "unknown capability `{capability}`"
                )));
            }
        }
        require_unique("features", &self.features)?;
        for feature in &self.features {
            require_kebab("feature", feature)?;
        }
        validate_compilation_target(&self.name, &self.profile, &self.capabilities)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Root {
    pub package: String,
    pub source: String,
    pub form: String,
}

impl Root {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("root.package", &self.package)?;
        require_path("root.source", &self.source, true)?;
        if !matches!(self.form.as_str(), "module" | "script" | "fragment") {
            return Err(FormatError::Invalid(format!(
                "unknown root form `{}`",
                self.form
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub alias: String,
    pub package: String,
}

impl Dependency {
    fn validate(&self) -> Result<(), FormatError> {
        require_name("dependency.alias", &self.alias)?;
        require_package_id("dependency.package", &self.package)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSetCondition {
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub requires_capabilities: Vec<String>,
    #[serde(default)]
    pub excludes_capabilities: Vec<String>,
    #[serde(default)]
    pub requires_features: Vec<String>,
    #[serde(default)]
    pub excludes_features: Vec<String>,
}

impl SourceSetCondition {
    fn validate(&self) -> Result<(), FormatError> {
        require_unique("source-set targets", &self.targets)?;
        require_unique("source-set profiles", &self.profiles)?;
        require_unique("required capabilities", &self.requires_capabilities)?;
        require_unique("excluded capabilities", &self.excludes_capabilities)?;
        if self
            .targets
            .iter()
            .any(|target| target != "tondo-vm-hosted")
            || self.profiles.iter().any(|profile| profile != "hosted")
        {
            return Err(FormatError::Invalid(
                "source-set condition names an unsupported target or profile".into(),
            ));
        }
        for value in self
            .targets
            .iter()
            .chain(&self.profiles)
            .chain(&self.requires_features)
            .chain(&self.excludes_features)
        {
            require_nonempty("source-set condition value", value)?;
        }
        for capability in &self.requires_capabilities {
            if !CAPABILITIES.contains(&capability.as_str()) {
                return Err(FormatError::Invalid(format!(
                    "unknown capability `{capability}`"
                )));
            }
        }
        for capability in &self.excludes_capabilities {
            if !CAPABILITIES.contains(&capability.as_str()) {
                return Err(FormatError::Invalid(format!(
                    "unknown capability `{capability}`"
                )));
            }
        }
        if self
            .requires_capabilities
            .iter()
            .any(|value| self.excludes_capabilities.contains(value))
        {
            return Err(FormatError::Invalid(
                "a capability cannot be both required and excluded".into(),
            ));
        }
        require_unique("required features", &self.requires_features)?;
        require_unique("excluded features", &self.excludes_features)?;
        for feature in self.requires_features.iter().chain(&self.excludes_features) {
            require_kebab("source-set feature", feature)?;
        }
        Ok(())
    }

    fn matches(&self, target: &Target) -> bool {
        (self.targets.is_empty() || self.targets.iter().any(|v| v == &target.name))
            && (self.profiles.is_empty() || self.profiles.iter().any(|v| v == &target.profile))
            && self
                .requires_capabilities
                .iter()
                .all(|v| target.capabilities.contains(v))
            && self
                .excludes_capabilities
                .iter()
                .all(|v| !target.capabilities.contains(v))
            && self
                .requires_features
                .iter()
                .all(|v| target.features.contains(v))
            && self
                .excludes_features
                .iter()
                .all(|v| !target.features.contains(v))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub physical_path: String,
    pub logical_path: String,
    pub module: String,
}

impl Source {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_path(&format!("{field}.physical_path"), &self.physical_path, true)?;
        require_path(&format!("{field}.logical_path"), &self.logical_path, true)?;
        require_module(&format!("{field}.module"), &self.module)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSet {
    pub id: String,
    #[serde(default)]
    pub when: SourceSetCondition,
    pub sources: Vec<Source>,
}

impl SourceSet {
    fn validate(&self, package: &str) -> Result<(), FormatError> {
        require_kebab("source-set.id", &self.id)?;
        self.when.validate()?;
        if self.sources.is_empty() {
            return Err(FormatError::Invalid(format!(
                "package `{package}` source set `{}` is empty",
                self.id
            )));
        }
        let mut logical = BTreeSet::new();
        for source in &self.sources {
            source.validate("source-set source")?;
            if !logical.insert(&source.logical_path) {
                return Err(FormatError::Invalid(format!(
                    "package `{package}` repeats logical path `{}`",
                    source.logical_path
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub id: String,
    pub local_name: String,
    pub edition: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub source_sets: Vec<SourceSet>,
}

impl Package {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("package.id", &self.id)?;
        require_name("package.local_name", &self.local_name)?;
        require_nonempty("package.edition", &self.edition)?;
        let aliases = self
            .dependencies
            .iter()
            .map(|v| v.alias.clone())
            .collect::<Vec<_>>();
        require_unique("dependency aliases", &aliases)?;
        for dependency in &self.dependencies {
            dependency.validate()?;
            if dependency.alias == self.local_name {
                return Err(FormatError::Invalid(format!(
                    "package `{}` aliases itself as `{}`",
                    self.id, dependency.alias
                )));
            }
        }
        let sets = self
            .source_sets
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>();
        require_unique("source-set IDs", &sets)?;
        if self.source_sets.is_empty() {
            return Err(FormatError::Invalid(format!(
                "package `{}` has no source sets",
                self.id
            )));
        }
        for source_set in &self.source_sets {
            source_set.validate(&self.id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaPackage {
    pub id: String,
    pub local_name: String,
    pub edition: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub sources: Vec<Source>,
}

impl MetaPackage {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("meta package.id", &self.id)?;
        require_name("meta package.local_name", &self.local_name)?;
        require_nonempty("meta package.edition", &self.edition)?;
        let aliases = self
            .dependencies
            .iter()
            .map(|v| v.alias.clone())
            .collect::<Vec<_>>();
        require_unique("meta dependency aliases", &aliases)?;
        for dependency in &self.dependencies {
            dependency.validate()?;
            if dependency.alias == self.local_name {
                return Err(FormatError::Invalid(format!(
                    "meta package `{}` aliases itself",
                    self.id
                )));
            }
        }
        if self.sources.is_empty() {
            return Err(FormatError::Invalid(format!(
                "meta package `{}` has no sources",
                self.id
            )));
        }
        let keys = self
            .sources
            .iter()
            .map(|v| {
                (
                    v.physical_path.clone(),
                    v.logical_path.clone(),
                    v.module.clone(),
                )
            })
            .collect::<Vec<_>>();
        require_unique("meta sources", &keys)?;
        for source in &self.sources {
            source.validate("meta source")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedPath {
    pub name: String,
    pub path: String,
}

impl NamedPath {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_nonempty(&format!("{field}.name"), &self.name)?;
        if self.name.starts_with("privileged:") {
            return Err(FormatError::Invalid(format!(
                "{field}.name uses reserved prefix"
            )));
        }
        require_path(&format!("{field}.path"), &self.path, false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub package: String,
    pub entry: String,
}

impl Provider {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_package_id(&format!("{field}.package"), &self.package)?;
        require_module(&format!("{field}.entry"), &self.entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoot {
    pub package: String,
    pub module: String,
}

impl ModelRoot {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("model_root.package", &self.package)?;
        require_module("model_root.module", &self.module)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub logical_path: String,
    pub module: String,
}

impl Output {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_path(&format!("{field}.logical_path"), &self.logical_path, true)?;
        require_module(&format!("{field}.module"), &self.module)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub steps: u64,
    pub memory_bytes: u64,
    pub output_bytes: u64,
}

impl Limits {
    fn validate(&self) -> Result<(), FormatError> {
        if self.steps == 0 || self.memory_bytes == 0 || self.output_bytes == 0 {
            return Err(FormatError::Invalid(
                "generator limits must be positive".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub id: String,
    pub owner_package: String,
    pub provider: Provider,
    pub meta_model: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub model_roots: Vec<ModelRoot>,
    pub outputs: Vec<Output>,
    pub limits: Limits,
}

impl Generator {
    fn validate(&self) -> Result<(), FormatError> {
        require_kebab("generator.id", &self.id)?;
        require_package_id("generator.owner_package", &self.owner_package)?;
        self.provider.validate("generator.provider")?;
        if self.meta_model != META_MODEL {
            return Err(FormatError::Invalid(format!(
                "unsupported generator meta model `{}`",
                self.meta_model
            )));
        }
        require_unique("generator.inputs", &self.inputs)?;
        for input in &self.inputs {
            require_nonempty("generator input", input)?;
        }
        let roots = self
            .model_roots
            .iter()
            .map(|v| (v.package.clone(), v.module.clone()))
            .collect::<Vec<_>>();
        require_unique("generator.model_roots", &roots)?;
        for root in &self.model_roots {
            root.validate()?;
        }
        let outputs = self
            .outputs
            .iter()
            .map(|v| (v.logical_path.clone(), v.module.clone()))
            .collect::<Vec<_>>();
        require_unique("generator.outputs", &outputs)?;
        if self.outputs.is_empty() {
            return Err(FormatError::Invalid(format!(
                "generator `{}` has no outputs",
                self.id
            )));
        }
        for output in &self.outputs {
            output.validate("generator output")?;
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraitIdentity {
    pub package: String,
    pub module: String,
    pub name: String,
}

impl TraitIdentity {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("trait.package", &self.package)?;
        require_module("trait.module", &self.module)?;
        require_name("trait.name", &self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeriveProvider {
    #[serde(rename = "trait")]
    pub trait_: TraitIdentity,
    pub provider: Provider,
    pub meta_model: String,
    pub limits: Limits,
}

impl DeriveProvider {
    fn validate(&self) -> Result<(), FormatError> {
        self.trait_.validate()?;
        self.provider.validate("derive provider")?;
        if self.meta_model != META_MODEL {
            return Err(FormatError::Invalid(format!(
                "unsupported derive meta model `{}`",
                self.meta_model
            )));
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub target: Target,
    pub root: Root,
    pub standard: String,
    #[serde(default)]
    pub meta_packages: Vec<MetaPackage>,
    pub packages: Vec<Package>,
    #[serde(default)]
    pub generator_inputs: Vec<NamedPath>,
    #[serde(default)]
    pub generators: Vec<Generator>,
    #[serde(default)]
    pub derive_providers: Vec<DeriveProvider>,
    #[serde(default)]
    pub privileged_units: Vec<NamedPath>,
}

impl Manifest {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Self = decode(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        encode(self)
    }

    /// Returns the deterministic representation used when a producer wants
    /// to hash a manifest.  Parsing itself intentionally accepts arbitrary
    /// list order because manifest bytes are user-authored inputs.
    pub fn canonicalize(&self) -> Result<Self, FormatError> {
        let mut value = self.clone();
        value.target.capabilities.sort();
        value.target.features.sort();
        for package in &mut value.packages {
            package
                .dependencies
                .sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
            package.source_sets.sort_by(|a, b| a.id.cmp(&b.id));
            for source_set in &mut package.source_sets {
                source_set
                    .sources
                    .sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
                canonicalize_condition(&mut source_set.when);
            }
        }
        for package in &mut value.meta_packages {
            package
                .dependencies
                .sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
            package
                .sources
                .sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
        }
        value.packages.sort_by(|a, b| a.id.cmp(&b.id));
        value.meta_packages.sort_by(|a, b| a.id.cmp(&b.id));
        value.generator_inputs.sort_by(|a, b| a.name.cmp(&b.name));
        value.generators.sort_by(|a, b| a.id.cmp(&b.id));
        for generator in &mut value.generators {
            generator.inputs.sort();
            generator
                .model_roots
                .sort_by(|a, b| (&a.package, &a.module).cmp(&(&b.package, &b.module)));
            generator
                .outputs
                .sort_by(|a, b| (&a.logical_path, &a.module).cmp(&(&b.logical_path, &b.module)));
        }
        value.derive_providers.sort_by(|a, b| {
            (&a.trait_.package, &a.trait_.module, &a.trait_.name).cmp(&(
                &b.trait_.package,
                &b.trait_.module,
                &b.trait_.name,
            ))
        });
        value.privileged_units.sort_by(|a, b| a.name.cmp(&b.name));
        value.validate()?;
        Ok(value)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FormatError> {
        self.canonicalize()?.encode()
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        if self.format != MANIFEST_FORMAT {
            return Err(FormatError::UnsupportedFormat {
                expected: MANIFEST_FORMAT,
                actual: self.format.clone(),
            });
        }
        self.target.validate()?;
        self.root.validate()?;
        require_package_id("standard", &self.standard)?;
        if self.packages.is_empty() {
            return Err(FormatError::Invalid(
                "manifest has no runtime packages".into(),
            ));
        }
        let runtime_ids = self
            .packages
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        require_unique("runtime package IDs", &runtime_ids)?;
        let meta_ids = self
            .meta_packages
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        require_unique("meta package IDs", &meta_ids)?;
        if runtime_ids.iter().any(|id| meta_ids.contains(id)) {
            return Err(FormatError::Invalid(
                "runtime and meta PackageIds must be disjoint".into(),
            ));
        }
        for package in &self.packages {
            package.validate()?;
            if package.id == self.standard {
                return Err(FormatError::Invalid(
                    "the standard runtime package must not be repeated in packages".into(),
                ));
            }
        }
        for package in &self.meta_packages {
            package.validate()?;
        }
        let all_sources = self
            .packages
            .iter()
            .flat_map(|p| p.source_sets.iter().flat_map(|s| s.sources.iter()))
            .chain(self.meta_packages.iter().flat_map(|p| p.sources.iter()))
            .map(|s| s.physical_path.clone())
            .collect::<Vec<_>>();
        require_unique("manifest physical paths", &all_sources)?;
        let package_ids = runtime_ids.iter().collect::<BTreeSet<_>>();
        if !package_ids.contains(&self.root.package) {
            return Err(FormatError::Invalid(format!(
                "root package `{}` is not declared",
                self.root.package
            )));
        }
        let root_sources = self
            .packages
            .iter()
            .find(|p| p.id == self.root.package)
            .into_iter()
            .flat_map(|p| p.source_sets.iter())
            .flat_map(|s| s.sources.iter())
            .map(|s| s.physical_path.as_str())
            .collect::<BTreeSet<_>>();
        if !root_sources.contains(self.root.source.as_str()) {
            return Err(FormatError::Invalid(format!(
                "root source `{}` is not declared",
                self.root.source
            )));
        }
        validate_graph(&self.packages, false)?;
        validate_graph(&self.meta_packages, true)?;
        let input_names = self
            .generator_inputs
            .iter()
            .map(|v| v.name.clone())
            .collect::<Vec<_>>();
        require_unique("generator input names", &input_names)?;
        for input in &self.generator_inputs {
            input.validate("generator input")?;
        }
        let generator_ids = self
            .generators
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>();
        require_unique("generator IDs", &generator_ids)?;
        let meta_set = meta_ids.iter().collect::<BTreeSet<_>>();
        let standard_meta = standard_meta_id(&self.standard);
        let active_paths = self
            .packages
            .iter()
            .flat_map(|package| package.source_sets.iter())
            .filter(|set| set.when.matches(&self.target))
            .flat_map(|set| set.sources.iter().map(|source| source.logical_path.clone()))
            .collect::<BTreeSet<_>>();
        let mut active_package_paths = BTreeSet::new();
        for package in &self.packages {
            for set in &package.source_sets {
                if set.when.matches(&self.target) {
                    for source in &set.sources {
                        if !active_package_paths
                            .insert((package.id.clone(), source.logical_path.clone()))
                        {
                            return Err(FormatError::Invalid(format!(
                                "active package `{}` repeats logical path `{}`",
                                package.id, source.logical_path
                            )));
                        }
                    }
                }
            }
        }
        let mut generated_paths = BTreeSet::new();
        for generator in &self.generators {
            generator.validate()?;
            if !package_ids.contains(&generator.owner_package) {
                return Err(FormatError::Invalid(format!(
                    "generator `{}` owner is not a runtime package",
                    generator.id
                )));
            }
            if !meta_set.contains(&generator.provider.package)
                && generator.provider.package != standard_meta
            {
                return Err(FormatError::Invalid(format!(
                    "generator `{}` provider is not in the meta graph",
                    generator.id
                )));
            }
            for input in &generator.inputs {
                if !input_names.contains(input) {
                    return Err(FormatError::Invalid(format!(
                        "generator `{}` uses unknown input `{input}`",
                        generator.id
                    )));
                }
            }
            for output in &generator.outputs {
                if active_paths.contains(&output.logical_path)
                    || !generated_paths.insert(output.logical_path.clone())
                {
                    return Err(FormatError::Invalid(format!(
                        "generator `{}` output `{}` collides with another source/output",
                        generator.id, output.logical_path
                    )));
                }
            }
        }
        let traits = self
            .derive_providers
            .iter()
            .map(|v| {
                (
                    v.trait_.package.clone(),
                    v.trait_.module.clone(),
                    v.trait_.name.clone(),
                )
            })
            .collect::<Vec<_>>();
        require_unique("derive provider traits", &traits)?;
        for provider in &self.derive_providers {
            provider.validate()?;
            if !package_ids.contains(&provider.trait_.package) {
                return Err(FormatError::Invalid(
                    "derive trait must belong to a runtime package".into(),
                ));
            }
            if !meta_set.contains(&provider.provider.package)
                && provider.provider.package != standard_meta
            {
                return Err(FormatError::Invalid(
                    "derive provider is not in the meta graph".into(),
                ));
            }
        }
        let unit_names = self
            .privileged_units
            .iter()
            .map(|v| v.name.clone())
            .collect::<Vec<_>>();
        require_unique("privileged unit names", &unit_names)?;
        for unit in &self.privileged_units {
            unit.validate("privileged unit")?;
            require_unit_id("privileged unit name", &unit.name)?;
        }
        Ok(())
    }

    pub fn active_sources(&self) -> Vec<(String, Source)> {
        let mut result = Vec::new();
        for package in &self.packages {
            for set in &package.source_sets {
                if set.when.matches(&self.target) {
                    result.extend(
                        set.sources
                            .iter()
                            .cloned()
                            .map(|source| (set.id.clone(), source)),
                    );
                }
            }
        }
        result.sort_by(|a, b| a.1.physical_path.cmp(&b.1.physical_path));
        result
    }
}

fn canonicalize_condition(condition: &mut SourceSetCondition) {
    condition.targets.sort();
    condition.profiles.sort();
    condition.requires_capabilities.sort();
    condition.excludes_capabilities.sort();
    condition.requires_features.sort();
    condition.excludes_features.sort();
}

fn validate_meta_reachability_roots(
    packages: &[MetaPackage],
    roots: impl IntoIterator<Item = String>,
) -> Result<(), FormatError> {
    let ids = packages
        .iter()
        .map(|p| p.id.clone())
        .collect::<BTreeSet<_>>();
    let mut reachable = BTreeSet::new();
    let mut stack = roots
        .into_iter()
        .filter(|id| ids.contains(id))
        .collect::<Vec<_>>();
    while let Some(id) = stack.pop() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        if let Some(package) = packages.iter().find(|p| p.id == id) {
            stack.extend(
                package
                    .dependencies
                    .iter()
                    .filter(|dependency| ids.contains(&dependency.package))
                    .map(|dependency| dependency.package.clone()),
            );
        }
    }
    if reachable != ids {
        let unused = ids
            .difference(&reachable)
            .next()
            .expect("difference is non-empty");
        return Err(FormatError::Invalid(format!(
            "meta package `{unused}` is not reachable from a provider"
        )));
    }
    Ok(())
}

fn validate_graph<T>(packages: &[T], meta: bool) -> Result<(), FormatError>
where
    T: PackageLike,
{
    let ids = packages
        .iter()
        .map(PackageLike::id)
        .collect::<BTreeSet<_>>();
    for package in packages {
        for dependency in package.dependencies() {
            if !ids.contains(dependency.package.as_str()) {
                return Err(FormatError::Invalid(format!(
                    "{} package `{}` depends on unknown package `{}`",
                    if meta { "meta" } else { "runtime" },
                    package.id(),
                    dependency.package
                )));
            }
        }
    }
    let graph = packages
        .iter()
        .map(|p| {
            (
                p.id().to_owned(),
                p.dependencies()
                    .iter()
                    .map(|d| d.package.clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in graph.keys() {
        visit(id, &graph, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit(
    id: &str,
    graph: &BTreeMap<String, Vec<String>>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), FormatError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.to_owned()) {
        return Err(FormatError::Invalid(format!(
            "dependency graph contains a cycle at `{id}`"
        )));
    }
    if let Some(dependencies) = graph.get(id) {
        for dependency in dependencies {
            visit(dependency, graph, visiting, visited)?;
        }
    }
    visiting.remove(id);
    visited.insert(id.to_owned());
    Ok(())
}

trait PackageLike {
    fn id(&self) -> &str;
    fn dependencies(&self) -> &[Dependency];
}

impl PackageLike for Package {
    fn id(&self) -> &str {
        &self.id
    }
    fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }
}

impl PackageLike for MetaPackage {
    fn id(&self) -> &str {
        &self.id
    }
    fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardRef {
    pub package_id: String,
    pub content_hash: String,
}

impl StandardRef {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_package_id(&format!("{field}.package_id"), &self.package_id)?;
        require_hash(&self.content_hash, &format!("{field}.content_hash"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSource {
    pub source_set: String,
    pub physical_path: String,
    pub logical_path: String,
    pub module: String,
    pub sha256: String,
}

impl LockedSource {
    fn validate(&self) -> Result<(), FormatError> {
        require_kebab("locked source_set", &self.source_set)?;
        Source {
            physical_path: self.physical_path.clone(),
            logical_path: self.logical_path.clone(),
            module: self.module.clone(),
        }
        .validate("locked source")?;
        require_hash(&self.sha256, "locked source.sha256")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedMetaSource {
    pub physical_path: String,
    pub logical_path: String,
    pub module: String,
    pub sha256: String,
}

impl LockedMetaSource {
    fn validate(&self) -> Result<(), FormatError> {
        Source {
            physical_path: self.physical_path.clone(),
            logical_path: self.logical_path.clone(),
            module: self.module.clone(),
        }
        .validate("locked meta source")?;
        require_hash(&self.sha256, "locked meta source.sha256")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub id: String,
    pub content_hash: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub sources: Vec<LockedSource>,
    pub interface: Option<String>,
}

impl LockedPackage {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("locked package.id", &self.id)?;
        require_hash(&self.content_hash, "locked package.content_hash")?;
        let keys = self
            .dependencies
            .iter()
            .map(|v| (v.alias.clone(), v.package.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("locked dependencies", &keys)?;
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        let source_keys = self
            .sources
            .iter()
            .map(|v| v.physical_path.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked sources", &source_keys)?;
        for source in &self.sources {
            source.validate()?;
        }
        if let Some(interface) = &self.interface {
            require_hash(interface, "locked package.interface")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedMetaPackage {
    pub id: String,
    pub content_hash: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub sources: Vec<LockedMetaSource>,
}

impl LockedMetaPackage {
    fn validate(&self) -> Result<(), FormatError> {
        require_package_id("locked meta package.id", &self.id)?;
        require_hash(&self.content_hash, "locked meta package.content_hash")?;
        let keys = self
            .dependencies
            .iter()
            .map(|v| (v.alias.clone(), v.package.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("locked meta dependencies", &keys)?;
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        let source_keys = self
            .sources
            .iter()
            .map(|v| v.physical_path.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked meta sources", &source_keys)?;
        for source in &self.sources {
            source.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedNamedInput {
    pub name: String,
    pub sha256: String,
}

impl LockedNamedInput {
    fn validate(&self, field: &str) -> Result<(), FormatError> {
        require_nonempty(&format!("{field}.name"), &self.name)?;
        require_hash(&self.sha256, &format!("{field}.sha256"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedGenerator {
    pub id: String,
    pub owner_package: String,
    pub provider_package: String,
    pub entry: String,
    pub meta_model: String,
    pub provider_hash: String,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub model_roots: Vec<ModelRoot>,
    pub outputs: Vec<Output>,
    pub limits: Limits,
}

impl LockedGenerator {
    fn validate(&self) -> Result<(), FormatError> {
        require_kebab("locked generator.id", &self.id)?;
        require_package_id("locked generator.owner_package", &self.owner_package)?;
        require_package_id("locked generator.provider_package", &self.provider_package)?;
        require_module("locked generator.entry", &self.entry)?;
        if self.meta_model != META_MODEL {
            return Err(FormatError::Invalid(
                "locked generator has an unsupported meta model".into(),
            ));
        }
        require_hash(&self.provider_hash, "locked generator.provider_hash")?;
        require_sorted_unique("locked generator.inputs", &self.inputs)?;
        let roots = self
            .model_roots
            .iter()
            .map(|v| (v.package.clone(), v.module.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("locked generator.model_roots", &roots)?;
        for root in &self.model_roots {
            root.validate()?;
        }
        let outputs = self
            .outputs
            .iter()
            .map(|v| (v.logical_path.clone(), v.module.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("locked generator.outputs", &outputs)?;
        for output in &self.outputs {
            output.validate("locked generator output")?;
        }
        self.limits.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedDeriveProvider {
    pub origin: String,
    pub trait_package: String,
    pub trait_module: String,
    pub trait_name: String,
    pub provider_package: String,
    pub entry: String,
    pub meta_model: String,
    pub provider_hash: String,
    pub limits: Limits,
}

impl LockedDeriveProvider {
    fn validate(&self) -> Result<(), FormatError> {
        if !matches!(self.origin.as_str(), "standard" | "manifest") {
            return Err(FormatError::Invalid(format!(
                "unknown derive provider origin `{}`",
                self.origin
            )));
        }
        require_package_id("locked trait_package", &self.trait_package)?;
        require_module("locked trait_module", &self.trait_module)?;
        require_name("locked trait_name", &self.trait_name)?;
        require_package_id("locked provider_package", &self.provider_package)?;
        require_module("locked provider entry", &self.entry)?;
        if self.meta_model != META_MODEL {
            return Err(FormatError::Invalid(
                "locked derive provider has an unsupported meta model".into(),
            ));
        }
        require_hash(&self.provider_hash, "locked derive provider.provider_hash")?;
        self.limits.validate()
    }

    fn key(&self) -> (String, String, String) {
        (
            self.trait_package.clone(),
            self.trait_module.clone(),
            self.trait_name.clone(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub format: String,
    pub manifest_hash: String,
    pub standard: StandardRef,
    pub meta_standard: StandardRef,
    #[serde(default)]
    pub packages: Vec<LockedPackage>,
    #[serde(default)]
    pub meta_packages: Vec<LockedMetaPackage>,
    #[serde(default)]
    pub generator_inputs: Vec<LockedNamedInput>,
    #[serde(default)]
    pub generators: Vec<LockedGenerator>,
    #[serde(default)]
    pub derive_providers: Vec<LockedDeriveProvider>,
    #[serde(default)]
    pub privileged_units: Vec<LockedNamedInput>,
}

impl Lockfile {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Self = decode(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        encode(self)
    }

    pub fn canonicalize(&self) -> Result<Self, FormatError> {
        let mut value = self.clone();
        value.packages.sort_by(|a, b| a.id.cmp(&b.id));
        value.meta_packages.sort_by(|a, b| a.id.cmp(&b.id));
        for package in &mut value.packages {
            package
                .dependencies
                .sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
            package
                .sources
                .sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
        }
        for package in &mut value.meta_packages {
            package
                .dependencies
                .sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
            package
                .sources
                .sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
        }
        value.generator_inputs.sort_by(|a, b| a.name.cmp(&b.name));
        value.generators.sort_by(|a, b| a.id.cmp(&b.id));
        for generator in &mut value.generators {
            generator.inputs.sort();
            generator
                .model_roots
                .sort_by(|a, b| (&a.package, &a.module).cmp(&(&b.package, &b.module)));
            generator
                .outputs
                .sort_by(|a, b| (&a.logical_path, &a.module).cmp(&(&b.logical_path, &b.module)));
        }
        value
            .derive_providers
            .sort_by_key(LockedDeriveProvider::key);
        value.privileged_units.sort_by(|a, b| a.name.cmp(&b.name));
        value.validate()?;
        Ok(value)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, FormatError> {
        self.canonicalize()?.encode()
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        if self.format != LOCKFILE_FORMAT {
            return Err(FormatError::UnsupportedFormat {
                expected: LOCKFILE_FORMAT,
                actual: self.format.clone(),
            });
        }
        require_hash(&self.manifest_hash, "lockfile.manifest_hash")?;
        self.standard.validate("lockfile.standard")?;
        self.meta_standard.validate("lockfile.meta_standard")?;
        let ids = self
            .packages
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked package IDs", &ids)?;
        let meta_ids = self
            .meta_packages
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked meta package IDs", &meta_ids)?;
        if ids.iter().any(|id| meta_ids.contains(id)) {
            return Err(FormatError::Invalid(
                "locked runtime and meta PackageIds overlap".into(),
            ));
        }
        for package in &self.packages {
            package.validate()?;
        }
        for package in &self.meta_packages {
            package.validate()?;
        }
        let input_names = self
            .generator_inputs
            .iter()
            .map(|v| v.name.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked generator input names", &input_names)?;
        for input in &self.generator_inputs {
            input.validate("locked generator input")?;
        }
        let generator_ids = self
            .generators
            .iter()
            .map(|v| v.id.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked generator IDs", &generator_ids)?;
        for generator in &self.generators {
            generator.validate()?;
        }
        let derive_keys = self
            .derive_providers
            .iter()
            .map(LockedDeriveProvider::key)
            .collect::<Vec<_>>();
        require_sorted_unique("locked derive provider identities", &derive_keys)?;
        for provider in &self.derive_providers {
            provider.validate()?;
        }
        let unit_names = self
            .privileged_units
            .iter()
            .map(|v| v.name.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("locked privileged units", &unit_names)?;
        for unit in &self.privileged_units {
            unit.validate("locked privileged unit")?;
            require_unit_id("locked privileged unit name", &unit.name)?;
        }
        Ok(())
    }

    /// Validates the lockfile against the exact manifest bytes and descriptor.
    /// This is the last pure boundary before an orchestrator reads any input.
    pub fn validate_against(
        &self,
        manifest: &Manifest,
        manifest_bytes: &[u8],
        descriptor: &StandardDescriptor,
    ) -> Result<(), FormatError> {
        self.validate()?;
        if self.manifest_hash != sha256(manifest_bytes) {
            return Err(FormatError::Invalid(
                "lockfile manifest_hash does not match manifest bytes".into(),
            ));
        }
        if self.standard != descriptor.runtime {
            return Err(FormatError::Invalid(
                "lockfile standard does not match descriptor runtime".into(),
            ));
        }
        if self.meta_standard != descriptor.meta {
            return Err(FormatError::Invalid(
                "lockfile meta_standard does not match descriptor meta".into(),
            ));
        }
        if self.standard.package_id != manifest.standard {
            return Err(FormatError::Invalid(
                "manifest standard does not match lockfile standard".into(),
            ));
        }
        if self.meta_standard.package_id != standard_meta_id(&manifest.standard) {
            return Err(FormatError::Invalid(
                "lockfile meta_standard is not the companion of manifest standard".into(),
            ));
        }
        validate_runtime_packages(manifest, self)?;
        validate_meta_packages(manifest, self)?;
        let meta_roots = manifest
            .generators
            .iter()
            .map(|generator| generator.provider.package.clone())
            .chain(
                manifest
                    .derive_providers
                    .iter()
                    .map(|provider| provider.provider.package.clone()),
            )
            .chain(
                descriptor
                    .derive_providers
                    .iter()
                    .map(|provider| provider.provider_package.clone()),
            );
        validate_meta_reachability_roots(&manifest.meta_packages, meta_roots)?;
        validate_named_inputs(
            &manifest.generator_inputs,
            &self.generator_inputs,
            "generator input",
        )?;
        validate_named_inputs(
            &manifest.privileged_units,
            &self.privileged_units,
            "privileged unit",
        )?;
        validate_generators(manifest, self)?;
        let standard_keys = descriptor
            .derive_providers
            .iter()
            .map(LockedDeriveProvider::key)
            .collect::<BTreeSet<_>>();
        let manifest_keys = manifest
            .derive_providers
            .iter()
            .map(|provider| {
                (
                    provider.trait_.package.clone(),
                    provider.trait_.module.clone(),
                    provider.trait_.name.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        if standard_keys.intersection(&manifest_keys).next().is_some() {
            return Err(FormatError::Invalid(
                "manifest and standard derive providers overlap".into(),
            ));
        }
        let mut expected = descriptor.derive_providers.clone();
        for provider in &manifest.derive_providers {
            expected.push(LockedDeriveProvider {
                origin: "manifest".into(),
                trait_package: provider.trait_.package.clone(),
                trait_module: provider.trait_.module.clone(),
                trait_name: provider.trait_.name.clone(),
                provider_package: provider.provider.package.clone(),
                entry: provider.provider.entry.clone(),
                meta_model: provider.meta_model.clone(),
                provider_hash: String::new(),
                limits: provider.limits,
            });
        }
        // Manifest providers do not carry their executable hash; compare all
        // identity and limits fields and leave the lockfile as the hash source.
        let actual_keys = self
            .derive_providers
            .iter()
            .map(LockedDeriveProvider::key)
            .collect::<BTreeSet<_>>();
        let expected_keys = expected
            .iter()
            .map(LockedDeriveProvider::key)
            .collect::<BTreeSet<_>>();
        if actual_keys != expected_keys {
            return Err(FormatError::Invalid(
                "lockfile derive provider identities differ from manifest/descriptor".into(),
            ));
        }
        for expected_provider in &descriptor.derive_providers {
            let actual = self
                .derive_providers
                .iter()
                .find(|provider| {
                    provider.origin == "standard" && provider.key() == expected_provider.key()
                })
                .ok_or_else(|| FormatError::Invalid("missing standard derive provider".into()))?;
            if actual != expected_provider {
                return Err(FormatError::Invalid(
                    "standard derive provider differs from descriptor".into(),
                ));
            }
        }
        for provider in &self.derive_providers {
            if provider.origin == "manifest" {
                let source = manifest
                    .derive_providers
                    .iter()
                    .find(|candidate| {
                        candidate.trait_.package == provider.trait_package
                            && candidate.trait_.module == provider.trait_module
                            && candidate.trait_.name == provider.trait_name
                    })
                    .ok_or_else(|| {
                        FormatError::Invalid("missing manifest derive provider".into())
                    })?;
                if provider.provider_package != source.provider.package
                    || provider.entry != source.provider.entry
                    || provider.meta_model != source.meta_model
                    || provider.limits != source.limits
                {
                    return Err(FormatError::Invalid(
                        "manifest derive provider differs from lockfile".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_named_inputs(
    manifest: &[NamedPath],
    locked: &[LockedNamedInput],
    field: &str,
) -> Result<(), FormatError> {
    let names = manifest
        .iter()
        .map(|v| v.name.as_str())
        .collect::<BTreeSet<_>>();
    let locked_names = locked
        .iter()
        .map(|v| v.name.as_str())
        .collect::<BTreeSet<_>>();
    if names != locked_names {
        return Err(FormatError::Invalid(format!(
            "{field} names differ between manifest and lockfile"
        )));
    }
    Ok(())
}

fn validate_runtime_packages(manifest: &Manifest, lock: &Lockfile) -> Result<(), FormatError> {
    if manifest.packages.len() != lock.packages.len() {
        return Err(FormatError::Invalid("runtime package counts differ".into()));
    }
    for package in &manifest.packages {
        let locked = lock
            .packages
            .iter()
            .find(|v| v.id == package.id)
            .ok_or_else(|| {
                FormatError::Invalid(format!("package `{}` is absent from lockfile", package.id))
            })?;
        let expected_sources = package
            .source_sets
            .iter()
            .flat_map(|set| {
                set.sources
                    .iter()
                    .map(move |source| (set.id.as_str(), source))
            })
            .map(|(set, source)| {
                (
                    set.to_owned(),
                    source.physical_path.clone(),
                    source.logical_path.clone(),
                    source.module.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let actual_sources = locked
            .sources
            .iter()
            .map(|source| {
                (
                    source.source_set.clone(),
                    source.physical_path.clone(),
                    source.logical_path.clone(),
                    source.module.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        if expected_sources != actual_sources {
            return Err(FormatError::Invalid(format!(
                "sources for package `{}` differ",
                package.id
            )));
        }
        let expected_dependencies = package
            .dependencies
            .iter()
            .map(|v| (&v.alias, &v.package))
            .collect::<BTreeSet<_>>();
        let actual_dependencies = locked
            .dependencies
            .iter()
            .map(|v| (&v.alias, &v.package))
            .collect::<BTreeSet<_>>();
        if expected_dependencies != actual_dependencies {
            return Err(FormatError::Invalid(format!(
                "dependencies for package `{}` differ",
                package.id
            )));
        }
        if package.id == manifest.root.package {
            if locked.interface.is_some() {
                return Err(FormatError::Invalid(
                    "the root package must not consume its own interface".into(),
                ));
            }
        } else if locked.interface.is_none() {
            return Err(FormatError::Invalid(format!(
                "non-root package `{}` has no locked interface",
                package.id
            )));
        }
        let interface_hash = locked.interface.as_deref();
        let bytes = runtime_content_bytes(
            &package.id,
            &package.dependencies,
            &locked.sources,
            interface_hash,
        )?;
        if locked.content_hash != sha256(&bytes) {
            return Err(FormatError::Invalid(format!(
                "content_hash for package `{}` is inconsistent",
                package.id
            )));
        }
    }
    Ok(())
}

fn validate_meta_packages(manifest: &Manifest, lock: &Lockfile) -> Result<(), FormatError> {
    if manifest.meta_packages.len() != lock.meta_packages.len() {
        return Err(FormatError::Invalid("meta package counts differ".into()));
    }
    for package in &manifest.meta_packages {
        let locked = lock
            .meta_packages
            .iter()
            .find(|v| v.id == package.id)
            .ok_or_else(|| {
                FormatError::Invalid(format!(
                    "meta package `{}` is absent from lockfile",
                    package.id
                ))
            })?;
        let expected_sources = package
            .sources
            .iter()
            .map(|source| (&source.physical_path, &source.logical_path, &source.module))
            .collect::<BTreeSet<_>>();
        let actual_sources = locked
            .sources
            .iter()
            .map(|source| (&source.physical_path, &source.logical_path, &source.module))
            .collect::<BTreeSet<_>>();
        if expected_sources != actual_sources {
            return Err(FormatError::Invalid(format!(
                "sources for meta package `{}` differ",
                package.id
            )));
        }
        let expected_dependencies = package
            .dependencies
            .iter()
            .map(|v| (&v.alias, &v.package))
            .collect::<BTreeSet<_>>();
        let actual_dependencies = locked
            .dependencies
            .iter()
            .map(|v| (&v.alias, &v.package))
            .collect::<BTreeSet<_>>();
        if expected_dependencies != actual_dependencies {
            return Err(FormatError::Invalid(format!(
                "dependencies for meta package `{}` differ",
                package.id
            )));
        }
        let bytes = meta_content_bytes(&package.id, &package.dependencies, &locked.sources)?;
        if locked.content_hash != sha256(&bytes) {
            return Err(FormatError::Invalid(format!(
                "content_hash for meta package `{}` is inconsistent",
                package.id
            )));
        }
    }
    Ok(())
}

fn runtime_content_bytes(
    id: &str,
    dependencies: &[Dependency],
    sources: &[LockedSource],
    interface_hash: Option<&str>,
) -> Result<Vec<u8>, FormatError> {
    let mut dependencies = dependencies.to_vec();
    dependencies.sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
    let mut sources = sources.to_vec();
    sources.sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
    #[derive(Serialize)]
    struct Content<'a> {
        package_id: &'a str,
        dependencies: Vec<Dependency>,
        sources: Vec<LockedSource>,
        interface_hash: Option<&'a str>,
    }
    encode(&Content {
        package_id: id,
        dependencies,
        sources,
        interface_hash,
    })
}

fn meta_content_bytes(
    id: &str,
    dependencies: &[Dependency],
    sources: &[LockedMetaSource],
) -> Result<Vec<u8>, FormatError> {
    let mut dependencies = dependencies.to_vec();
    dependencies.sort_by(|a, b| (&a.alias, &a.package).cmp(&(&b.alias, &b.package)));
    let mut sources = sources.to_vec();
    sources.sort_by(|a, b| a.physical_path.cmp(&b.physical_path));
    #[derive(Serialize)]
    struct Content<'a> {
        package_id: &'a str,
        dependencies: Vec<Dependency>,
        sources: Vec<LockedMetaSource>,
    }
    encode(&Content {
        package_id: id,
        dependencies,
        sources,
    })
}

fn validate_generators(manifest: &Manifest, lock: &Lockfile) -> Result<(), FormatError> {
    if manifest.generators.len() != lock.generators.len() {
        return Err(FormatError::Invalid("generator counts differ".into()));
    }
    for generator in &manifest.generators {
        let locked = lock
            .generators
            .iter()
            .find(|v| v.id == generator.id)
            .ok_or_else(|| {
                FormatError::Invalid(format!(
                    "generator `{}` is absent from lockfile",
                    generator.id
                ))
            })?;
        let mut inputs = generator.inputs.clone();
        inputs.sort();
        let mut roots = generator.model_roots.clone();
        roots.sort_by(|a, b| (&a.package, &a.module).cmp(&(&b.package, &b.module)));
        let mut outputs = generator.outputs.clone();
        outputs.sort_by(|a, b| (&a.logical_path, &a.module).cmp(&(&b.logical_path, &b.module)));
        if locked.owner_package != generator.owner_package
            || locked.provider_package != generator.provider.package
            || locked.entry != generator.provider.entry
            || locked.meta_model != generator.meta_model
            || locked.inputs != inputs
            || locked.model_roots != roots
            || locked.outputs != outputs
            || locked.limits != generator.limits
        {
            return Err(FormatError::Invalid(format!(
                "generator `{}` differs from lockfile",
                generator.id
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardDescriptor {
    pub format: String,
    pub runtime: StandardRef,
    pub meta: StandardRef,
    #[serde(default)]
    pub derive_providers: Vec<LockedDeriveProvider>,
}

impl StandardDescriptor {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Self = decode_canonical(bytes, "standard descriptor")?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        encode(self)
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.format != STANDARD_DESCRIPTOR_FORMAT {
            return Err(FormatError::UnsupportedFormat {
                expected: STANDARD_DESCRIPTOR_FORMAT,
                actual: self.format.clone(),
            });
        }
        self.runtime.validate("descriptor.runtime")?;
        self.meta.validate("descriptor.meta")?;
        let keys = self
            .derive_providers
            .iter()
            .map(LockedDeriveProvider::key)
            .collect::<Vec<_>>();
        require_sorted_unique("descriptor derive provider identities", &keys)?;
        for provider in &self.derive_providers {
            if provider.origin != "standard" {
                return Err(FormatError::Invalid(
                    "standard descriptor can only contain origin=standard providers".into(),
                ));
            }
            provider.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDependency {
    pub alias: String,
    pub package_id: String,
    pub api_hash: String,
}

impl InterfaceDependency {
    fn validate(&self) -> Result<(), FormatError> {
        require_name("interface dependency.alias", &self.alias)?;
        require_package_id("interface dependency.package_id", &self.package_id)?;
        require_hash(&self.api_hash, "interface dependency.api_hash")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Lists {
    capabilities: Vec<String>,
    features: Vec<String>,
    source_sets: Vec<String>,
    modules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interface {
    pub format: String,
    pub compiler: String,
    pub edition: String,
    pub package_id: String,
    pub target: String,
    pub profile: String,
    pub capability_registry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    pub meta_model: Option<String>,
    #[serde(default)]
    pub source_sets: Vec<String>,
    #[serde(default)]
    pub modules: Vec<String>,
    pub generation_hash: String,
    pub api_hash: String,
    #[serde(default)]
    pub dependencies: Vec<InterfaceDependency>,
}

impl Interface {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Self = decode_canonical(bytes, "interface")?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        encode(self)
    }

    pub fn content_hash(&self) -> Result<String, FormatError> {
        Ok(sha256(&self.encode()?))
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.format != INTERFACE_FORMAT {
            return Err(FormatError::UnsupportedFormat {
                expected: INTERFACE_FORMAT,
                actual: self.format.clone(),
            });
        }
        if self.compiler != COMPILER_ID {
            return Err(FormatError::Invalid(format!(
                "interface compiler `{}` differs from `{COMPILER_ID}`",
                self.compiler
            )));
        }
        require_nonempty("interface.edition", &self.edition)?;
        require_package_id("interface.package_id", &self.package_id)?;
        require_nonempty("interface.target", &self.target)?;
        require_nonempty("interface.profile", &self.profile)?;
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(FormatError::Invalid(
                "interface uses an unsupported capability registry".into(),
            ));
        }
        require_identity_lists(&Lists {
            capabilities: self.capabilities.clone(),
            features: self.features.clone(),
            source_sets: self.source_sets.clone(),
            modules: self.modules.clone(),
        })?;
        validate_compilation_target(&self.target, &self.profile, &self.capabilities)?;
        for source_set in &self.source_sets {
            require_source_set_identity(source_set, &self.package_id)?;
        }
        if let Some(meta_model) = &self.meta_model
            && meta_model != META_MODEL
        {
            return Err(FormatError::Invalid(format!(
                "unsupported interface meta model `{meta_model}`"
            )));
        }
        require_hash(&self.generation_hash, "interface.generation_hash")?;
        require_hash(&self.api_hash, "interface.api_hash")?;
        let aliases = self
            .dependencies
            .iter()
            .map(|v| v.alias.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("interface dependency aliases", &aliases)?;
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        if self.meta_model.is_none() && self.generation_hash != sha256(&[]) {
            return Err(FormatError::Invalid(
                "interface without meta_model must use the empty generation hash".into(),
            ));
        }
        Ok(())
    }
}

fn require_source_set_identity(value: &str, package: &str) -> Result<(), FormatError> {
    let Some((prefix, local)) = value.split_once('#') else {
        return Err(FormatError::Invalid(format!(
            "invalid source-set identity `{value}`"
        )));
    };
    let Some((length, owner)) = prefix.strip_prefix('@').and_then(|v| v.split_once(':')) else {
        return Err(FormatError::Invalid(format!(
            "invalid source-set identity `{value}`"
        )));
    };
    if owner != package || length.parse::<usize>().ok() != Some(owner.len()) {
        return Err(FormatError::Invalid(format!(
            "source-set identity `{value}` belongs to another package"
        )));
    }
    require_kebab("source-set local name", local)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationOutput {
    pub source_id: String,
    pub module: String,
    pub path: String,
    pub sha256: String,
}

impl GenerationOutput {
    fn validate(&self) -> Result<(), FormatError> {
        require_nonempty("generation output.source_id", &self.source_id)?;
        require_module("generation output.module", &self.module)?;
        require_generated_path("generation output.path", &self.path)?;
        require_hash(&self.sha256, "generation output.sha256")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRecord {
    pub kind: String,
    pub id: String,
    pub provider_package: String,
    pub provider_hash: String,
    pub entry: String,
    #[serde(default)]
    pub model_roots: Vec<ModelRoot>,
    pub model_hash: String,
    pub request_hash: String,
    #[serde(default)]
    pub outputs: Vec<GenerationOutput>,
}

impl GenerationRecord {
    fn validate(&self) -> Result<(), FormatError> {
        if !matches!(self.kind.as_str(), "derive" | "generator") {
            return Err(FormatError::Invalid(format!(
                "unknown generation kind `{}`",
                self.kind
            )));
        }
        if self.kind == "generator" {
            require_kebab("generation.id", &self.id)?;
        } else {
            let Some(hex) = self.id.strip_prefix("derive:") else {
                return Err(FormatError::Invalid(
                    "derive generation IDs must start with `derive:`".into(),
                ));
            };
            if hex.len() != 64
                || !hex
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(FormatError::Invalid(
                    "derive generation ID must contain 64 lowercase hex digits".into(),
                ));
            }
        }
        require_package_id("generation.provider_package", &self.provider_package)?;
        require_hash(&self.provider_hash, "generation.provider_hash")?;
        require_module("generation.entry", &self.entry)?;
        let roots = self
            .model_roots
            .iter()
            .map(|v| (v.package.clone(), v.module.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("generation.model_roots", &roots)?;
        for root in &self.model_roots {
            root.validate()?;
        }
        require_hash(&self.model_hash, "generation.model_hash")?;
        require_hash(&self.request_hash, "generation.request_hash")?;
        let output_keys = self
            .outputs
            .iter()
            .map(|v| (v.source_id.clone(), v.module.clone(), v.path.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("generation outputs", &output_keys)?;
        for output in &self.outputs {
            output.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHash {
    pub source_id: String,
    pub module: String,
    pub path: String,
    pub sha256: String,
}

impl SourceHash {
    fn validate(&self) -> Result<(), FormatError> {
        require_nonempty("source_hash.source_id", &self.source_id)?;
        require_module("source_hash.module", &self.module)?;
        require_generated_path("source_hash.path", &self.path)?;
        require_hash(&self.sha256, "source_hash.sha256")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub format: String,
    pub compiler: String,
    pub edition: String,
    pub source_form: String,
    pub package_id: String,
    pub target: String,
    pub profile: String,
    pub capability_registry: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
    pub meta_model: Option<String>,
    #[serde(default)]
    pub source_sets: Vec<String>,
    pub manifest_hash: String,
    pub lockfile_hash: String,
    #[serde(default)]
    pub generator_inputs: BTreeMap<String, String>,
    #[serde(default)]
    pub generation: Vec<GenerationRecord>,
    #[serde(default)]
    pub source_hashes: Vec<SourceHash>,
    pub interface_hash: String,
    pub build_hash: String,
    pub reproducible: bool,
}

impl Artifact {
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let value: Self = decode_canonical(bytes, "artifact")?;
        value.validate()?;
        Ok(value)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        encode(self)
    }

    pub fn content_hash(&self) -> Result<String, FormatError> {
        Ok(sha256(&self.encode()?))
    }

    fn validate(&self) -> Result<(), FormatError> {
        if self.format != ARTIFACT_FORMAT {
            return Err(FormatError::UnsupportedFormat {
                expected: ARTIFACT_FORMAT,
                actual: self.format.clone(),
            });
        }
        if self.compiler != COMPILER_ID {
            return Err(FormatError::Invalid(format!(
                "artifact compiler `{}` differs from `{COMPILER_ID}`",
                self.compiler
            )));
        }
        require_nonempty("artifact.edition", &self.edition)?;
        if !matches!(self.source_form.as_str(), "module" | "script" | "fragment") {
            return Err(FormatError::Invalid(format!(
                "unknown artifact source form `{}`",
                self.source_form
            )));
        }
        require_package_id("artifact.package_id", &self.package_id)?;
        require_nonempty("artifact.target", &self.target)?;
        require_nonempty("artifact.profile", &self.profile)?;
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(FormatError::Invalid(
                "artifact uses an unsupported capability registry".into(),
            ));
        }
        require_identity_lists(&Lists {
            capabilities: self.capabilities.clone(),
            features: self.features.clone(),
            source_sets: self.source_sets.clone(),
            modules: Vec::new(),
        })?;
        validate_compilation_target(&self.target, &self.profile, &self.capabilities)?;
        for source_set in &self.source_sets {
            require_source_set_identity(source_set, &self.package_id)?;
        }
        if let Some(meta_model) = &self.meta_model
            && meta_model != META_MODEL
        {
            return Err(FormatError::Invalid(
                "unsupported artifact meta model".into(),
            ));
        }
        require_hash(&self.manifest_hash, "artifact.manifest_hash")?;
        require_hash(&self.lockfile_hash, "artifact.lockfile_hash")?;
        for (name, value) in &self.generator_inputs {
            require_nonempty("artifact generator input name", name)?;
            require_hash(value, "artifact generator input hash")?;
        }
        let generation_keys = self
            .generation
            .iter()
            .map(|v| (v.kind.clone(), v.id.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("artifact generation", &generation_keys)?;
        for generation in &self.generation {
            generation.validate()?;
        }
        let source_keys = self
            .source_hashes
            .iter()
            .map(|v| (v.source_id.clone(), v.module.clone(), v.path.clone()))
            .collect::<Vec<_>>();
        require_sorted_unique("artifact source hashes", &source_keys)?;
        for source in &self.source_hashes {
            source.validate()?;
        }
        require_hash(&self.interface_hash, "artifact.interface_hash")?;
        require_hash(&self.build_hash, "artifact.build_hash")?;
        if self.meta_model.is_none() != self.generation.is_empty() {
            return Err(FormatError::Invalid(
                "meta_model and generation must agree".into(),
            ));
        }
        if self.build_hash != self.calculated_build_hash()? {
            return Err(FormatError::Invalid(
                "artifact build_hash does not match its fields".into(),
            ));
        }
        Ok(())
    }

    pub fn calculated_build_hash(&self) -> Result<String, FormatError> {
        #[derive(Serialize)]
        struct Fingerprint<'a> {
            compiler: &'a str,
            edition: &'a str,
            source_form: &'a str,
            package_id: &'a str,
            target: &'a str,
            profile: &'a str,
            capabilities: &'a [String],
            features: &'a [String],
            meta_model: Option<&'a str>,
            source_sets: &'a [String],
            manifest_hash: &'a str,
            lockfile_hash: &'a str,
            generator_inputs: &'a BTreeMap<String, String>,
            generation: &'a [GenerationRecord],
            source_hashes: &'a [SourceHash],
            interface_hash: &'a str,
        }
        hash(&Fingerprint {
            compiler: &self.compiler,
            edition: &self.edition,
            source_form: &self.source_form,
            package_id: &self.package_id,
            target: &self.target,
            profile: &self.profile,
            capabilities: &self.capabilities,
            features: &self.features,
            meta_model: self.meta_model.as_deref(),
            source_sets: &self.source_sets,
            manifest_hash: &self.manifest_hash,
            lockfile_hash: &self.lockfile_hash,
            generator_inputs: &self.generator_inputs,
            generation: &self.generation,
            source_hashes: &self.source_hashes,
            interface_hash: &self.interface_hash,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredInputKind {
    Source,
    MetaSource,
    GeneratorInput,
    PrivilegedUnit,
}

impl RequiredInputKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::MetaSource => "meta-source",
            Self::GeneratorInput => "generator-input",
            Self::PrivilegedUnit => "privileged-unit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredInputV2 {
    path: String,
    kind: RequiredInputKind,
    sha256: String,
}

impl RequiredInputV2 {
    pub fn path(&self) -> &str {
        &self.path
    }
    pub const fn kind(&self) -> RequiredInputKind {
        self.kind
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Pure result of parsing the three `/2` project records.  It deliberately
/// does not open paths, compile providers, or perform generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectPlanV2 {
    manifest_hash: String,
    lockfile_hash: String,
    manifest: Manifest,
    lockfile: Lockfile,
    descriptor: StandardDescriptor,
    required_inputs: Vec<RequiredInputV2>,
}

impl ProjectPlanV2 {
    pub fn parse(
        manifest_bytes: &[u8],
        lockfile_bytes: &[u8],
        descriptor_bytes: &[u8],
    ) -> Result<Self, FormatError> {
        let manifest = Manifest::decode(manifest_bytes)?;
        let lockfile = Lockfile::decode(lockfile_bytes)?;
        let descriptor = StandardDescriptor::decode(descriptor_bytes)?;
        lockfile.validate_against(&manifest, manifest_bytes, &descriptor)?;

        let lockfile_hash = sha256(lockfile_bytes);
        let mut required_inputs = Vec::new();
        for (source_set, source) in manifest.active_sources() {
            let locked_package = lockfile
                .packages
                .iter()
                .find(|package| package.id == owner_for_source(&manifest, &source.physical_path))
                .ok_or_else(|| {
                    FormatError::Invalid("active source owner is missing from lockfile".into())
                })?;
            let locked = locked_package
                .sources
                .iter()
                .find(|candidate| {
                    candidate.source_set == source_set
                        && candidate.physical_path == source.physical_path
                })
                .ok_or_else(|| {
                    FormatError::Invalid(format!(
                        "source `{}` is missing from lockfile",
                        source.physical_path
                    ))
                })?;
            required_inputs.push(RequiredInputV2 {
                path: source.physical_path,
                kind: RequiredInputKind::Source,
                sha256: locked.sha256.clone(),
            });
        }
        // The meta graph is closed and its sources are inputs to compiling the
        // provider programs.  The lockfile has already rejected extra or
        // missing packages, so all declared meta sources are deterministic.
        for package in &lockfile.meta_packages {
            for source in &package.sources {
                required_inputs.push(RequiredInputV2 {
                    path: source.physical_path.clone(),
                    kind: RequiredInputKind::MetaSource,
                    sha256: source.sha256.clone(),
                });
            }
        }
        for input in &manifest.generator_inputs {
            let locked = lockfile
                .generator_inputs
                .iter()
                .find(|v| v.name == input.name)
                .unwrap();
            required_inputs.push(RequiredInputV2 {
                path: input.path.clone(),
                kind: RequiredInputKind::GeneratorInput,
                sha256: locked.sha256.clone(),
            });
        }
        for input in &manifest.privileged_units {
            let locked = lockfile
                .privileged_units
                .iter()
                .find(|v| v.name == input.name)
                .unwrap();
            required_inputs.push(RequiredInputV2 {
                path: input.path.clone(),
                kind: RequiredInputKind::PrivilegedUnit,
                sha256: locked.sha256.clone(),
            });
        }
        required_inputs.sort_by(|a, b| {
            (a.path.as_str(), a.kind.as_str()).cmp(&(b.path.as_str(), b.kind.as_str()))
        });
        if required_inputs
            .windows(2)
            .any(|pair| pair[0].path == pair[1].path)
        {
            return Err(FormatError::Invalid(
                "a required input path has more than one owner".into(),
            ));
        }
        Ok(Self {
            manifest_hash: sha256(manifest_bytes),
            lockfile_hash,
            manifest,
            lockfile,
            descriptor,
            required_inputs,
        })
    }

    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }
    pub fn lockfile_hash(&self) -> &str {
        &self.lockfile_hash
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn lockfile(&self) -> &Lockfile {
        &self.lockfile
    }
    pub fn descriptor(&self) -> &StandardDescriptor {
        &self.descriptor
    }
    pub fn required_inputs(&self) -> &[RequiredInputV2] {
        &self.required_inputs
    }
}

fn owner_for_source(manifest: &Manifest, path: &str) -> String {
    manifest
        .packages
        .iter()
        .find(|package| {
            package.source_sets.iter().any(|set| {
                set.sources
                    .iter()
                    .any(|source| source.physical_path == path)
            })
        })
        .map(|package| package.id.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            format: MANIFEST_FORMAT.into(),
            target: Target {
                name: "tondo-vm-hosted".into(),
                profile: "hosted".into(),
                capability_registry: CAPABILITY_REGISTRY.into(),
                capabilities: vec![],
                features: vec![],
            },
            root: Root {
                package: "workspace:app@1".into(),
                source: "app/src/main.to".into(),
                form: "module".into(),
            },
            standard: "toolchain:std:0.1.0".into(),
            meta_packages: vec![],
            packages: vec![Package {
                id: "workspace:app@1".into(),
                local_name: "app".into(),
                edition: "0.1".into(),
                dependencies: vec![],
                source_sets: vec![SourceSet {
                    id: "common".into(),
                    when: SourceSetCondition::default(),
                    sources: vec![Source {
                        physical_path: "app/src/main.to".into(),
                        logical_path: "src/main.to".into(),
                        module: "main".into(),
                    }],
                }],
            }],
            generator_inputs: vec![],
            generators: vec![],
            derive_providers: vec![],
            privileged_units: vec![],
        }
    }

    fn descriptor() -> StandardDescriptor {
        StandardDescriptor {
            format: STANDARD_DESCRIPTOR_FORMAT.into(),
            runtime: StandardRef {
                package_id: "toolchain:std:0.1.0".into(),
                content_hash: sha256(b"std"),
            },
            meta: StandardRef {
                package_id: "toolchain:std-meta:0.1.0".into(),
                content_hash: sha256(b"meta"),
            },
            derive_providers: vec![],
        }
    }

    fn lock(manifest_bytes: &[u8]) -> Lockfile {
        let source = LockedSource {
            source_set: "common".into(),
            physical_path: "app/src/main.to".into(),
            logical_path: "src/main.to".into(),
            module: "main".into(),
            sha256: sha256(b"source"),
        };
        let package = LockedPackage {
            id: "workspace:app@1".into(),
            content_hash: String::new(),
            dependencies: vec![],
            sources: vec![source],
            interface: None,
        };
        let content =
            runtime_content_bytes(&package.id, &package.dependencies, &package.sources, None)
                .unwrap();
        Lockfile {
            format: LOCKFILE_FORMAT.into(),
            manifest_hash: sha256(manifest_bytes),
            standard: descriptor().runtime,
            meta_standard: descriptor().meta,
            packages: vec![LockedPackage {
                content_hash: sha256(&content),
                ..package
            }],
            meta_packages: vec![],
            generator_inputs: vec![],
            generators: vec![],
            derive_providers: vec![],
            privileged_units: vec![],
        }
    }

    #[test]
    fn v2_plan_round_trips_without_touching_v1() {
        let manifest = manifest();
        let manifest_bytes = manifest.encode().unwrap();
        let lock = lock(&manifest_bytes);
        let lock_bytes = lock.encode().unwrap();
        let descriptor = descriptor();
        let descriptor_bytes = descriptor.encode().unwrap();
        let plan = ProjectPlanV2::parse(&manifest_bytes, &lock_bytes, &descriptor_bytes).unwrap();
        assert_eq!(plan.required_inputs()[0].path(), "app/src/main.to");
        assert_eq!(plan.required_inputs()[0].kind(), RequiredInputKind::Source);
        assert_eq!(Manifest::decode(&manifest_bytes).unwrap(), manifest);
    }

    #[test]
    fn canonical_records_reject_whitespace_and_unknown_fields() {
        let descriptor = descriptor();
        let bytes = descriptor.encode().unwrap();
        let pretty = serde_json::to_vec_pretty(&descriptor).unwrap();
        assert!(matches!(
            StandardDescriptor::decode(&pretty),
            Err(FormatError::NonCanonical("standard descriptor"))
        ));
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), serde_json::Value::Null);
        assert!(StandardDescriptor::decode(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn graph_cycles_and_bad_hashes_are_rejected() {
        let mut manifest = manifest();
        manifest.packages[0].dependencies.push(Dependency {
            alias: "dep".into(),
            package: "workspace:app@1".into(),
        });
        assert!(manifest.validate().is_err());
        let mut descriptor = descriptor();
        descriptor.runtime.content_hash = "sha256:not-a-hash".into();
        assert!(descriptor.encode().is_err());
    }
}
