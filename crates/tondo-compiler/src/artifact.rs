//! Deterministic build identity, compiled-interface, and artifact metadata.
//!
//! These formats are toolchain contracts. They deliberately describe source
//! compatibility and build inputs without claiming a stable native ABI.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hir::{
    HirCallableId, HirCapability, HirConstantValue, HirConstantValueKind, HirGenericParameter,
    HirImplementationMethodId, HirNominalShape, HirProgram, HirTraitConstructor, HirTraitReference,
    HirTypeDeclarationKind, HirVariantPayload,
};
use crate::package::{PackageGraph, PackageId};
use crate::resolve::{MemberId, MemberOwner, ResolvedProgram, SymbolId, SymbolKind, Visibility};
use crate::source::SourceDatabase;
use crate::types::TypeError;

pub const INTERFACE_FORMAT: &str = "tondo-interface-0.1/1";
pub const ARTIFACT_FORMAT: &str = "tondo-artifact-0.1/1";
pub const CAPABILITY_REGISTRY: &str = "tondo-capabilities/1";
pub const COMPILER_ID: &str = concat!("tondo-bootstrap/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FeatureName(String);

impl FeatureName {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        if !is_kebab_identifier(&value) {
            return Err(ArtifactError::InvalidFeature(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FeatureName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceSetId(String);

impl SourceSetId {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactError> {
        let value = value.into();
        if source_set_parts(&value).is_none() {
            return Err(ArtifactError::InvalidSourceSet(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn for_package(package: &PackageId, local: &str) -> Result<Self, ArtifactError> {
        if !is_kebab_identifier(local) {
            return Err(ArtifactError::InvalidSourceSet(local.into()));
        }
        Self::new(format!(
            "@{}:{}#{local}",
            package.as_str().len(),
            package.as_str()
        ))
    }

    fn belongs_to(&self, package: &PackageId) -> bool {
        source_set_parts(self.as_str())
            .is_some_and(|(owner, _)| owner.as_bytes() == package.as_str().as_bytes())
    }
}

impl fmt::Display for SourceSetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct DeclaredBuildInputs {
    features: BTreeSet<FeatureName>,
    source_sets: BTreeSet<SourceSetId>,
    manifest_hash: Option<String>,
    lockfile_hash: Option<String>,
    generator_inputs: BTreeMap<String, String>,
    dependency_interfaces: BTreeMap<PackageId, CompiledInterface>,
    require_dependency_interfaces: bool,
}

impl DeclaredBuildInputs {
    pub fn new(features: BTreeSet<FeatureName>, source_sets: BTreeSet<SourceSetId>) -> Self {
        Self {
            features,
            source_sets,
            ..Self::default()
        }
    }

    pub fn with_manifest_hash(mut self, hash: String) -> Result<Self, ArtifactError> {
        validate_sha256(&hash)?;
        self.manifest_hash = Some(hash);
        Ok(self)
    }

    pub fn with_lockfile_hash(mut self, hash: String) -> Result<Self, ArtifactError> {
        validate_sha256(&hash)?;
        self.lockfile_hash = Some(hash);
        Ok(self)
    }

    pub fn with_generator_inputs(
        mut self,
        inputs: BTreeMap<String, String>,
    ) -> Result<Self, ArtifactError> {
        for (name, hash) in &inputs {
            if name.is_empty() || name.contains(['\n', '\r']) {
                return Err(ArtifactError::InvalidGeneratorInput(name.clone()));
            }
            validate_sha256(hash)?;
        }
        self.generator_inputs = inputs;
        Ok(self)
    }

    pub fn with_dependency_interfaces(
        mut self,
        interfaces: BTreeMap<PackageId, CompiledInterface>,
        required: bool,
    ) -> Self {
        self.dependency_interfaces = interfaces;
        self.require_dependency_interfaces = required;
        self
    }

    pub fn features(&self) -> &BTreeSet<FeatureName> {
        &self.features
    }

    pub fn source_sets(&self) -> &BTreeSet<SourceSetId> {
        &self.source_sets
    }

    pub fn manifest_hash(&self) -> Option<&str> {
        self.manifest_hash.as_deref()
    }

    pub fn lockfile_hash(&self) -> Option<&str> {
        self.lockfile_hash.as_deref()
    }

    pub fn generator_inputs(&self) -> &BTreeMap<String, String> {
        &self.generator_inputs
    }

    pub fn dependency_interfaces(&self) -> &BTreeMap<PackageId, CompiledInterface> {
        &self.dependency_interfaces
    }

    pub fn require_dependency_interfaces(&self) -> bool {
        self.require_dependency_interfaces
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceDependency {
    alias: String,
    package_id: String,
    api_hash: String,
}

impl InterfaceDependency {
    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn api_hash(&self) -> &str {
        &self.api_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledInterface {
    format: String,
    compiler: String,
    edition: String,
    package_id: String,
    target: String,
    profile: String,
    capability_registry: String,
    capabilities: Vec<String>,
    features: Vec<String>,
    source_sets: Vec<String>,
    modules: Vec<String>,
    api_hash: String,
    dependencies: Vec<InterfaceDependency>,
}

impl CompiledInterface {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        edition: String,
        package_id: String,
        target: String,
        profile: String,
        capabilities: Vec<String>,
        features: Vec<String>,
        source_sets: Vec<String>,
        modules: Vec<String>,
        api_hash: String,
        dependencies: Vec<InterfaceDependency>,
    ) -> Result<Self, ArtifactError> {
        let interface = Self {
            format: INTERFACE_FORMAT.into(),
            compiler: COMPILER_ID.into(),
            edition,
            package_id,
            target,
            profile,
            capability_registry: CAPABILITY_REGISTRY.into(),
            capabilities,
            features,
            source_sets,
            modules,
            api_hash,
            dependencies,
        };
        interface.validate()?;
        Ok(interface)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ArtifactError> {
        let interface: Self = serde_json::from_slice(bytes)
            .map_err(|error| ArtifactError::InvalidInterface(error.to_string()))?;
        interface.validate()?;
        if interface.encode()? != bytes {
            return Err(ArtifactError::NonCanonicalInterface);
        }
        Ok(interface)
    }

    pub fn encode(&self) -> Result<Vec<u8>, ArtifactError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| ArtifactError::Serialization(error.to_string()))
    }

    pub fn content_hash(&self) -> Result<String, ArtifactError> {
        Ok(sha256(&self.encode()?))
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn compiler(&self) -> &str {
        &self.compiler
    }

    pub fn edition(&self) -> &str {
        &self.edition
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn capability_registry(&self) -> &str {
        &self.capability_registry
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn source_sets(&self) -> &[String] {
        &self.source_sets
    }

    pub fn modules(&self) -> &[String] {
        &self.modules
    }

    pub fn api_hash(&self) -> &str {
        &self.api_hash
    }

    pub fn dependencies(&self) -> &[InterfaceDependency] {
        &self.dependencies
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.format != INTERFACE_FORMAT {
            return Err(ArtifactError::UnsupportedInterfaceFormat(
                self.format.clone(),
            ));
        }
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(ArtifactError::UnsupportedCapabilityRegistry(
                self.capability_registry.clone(),
            ));
        }
        for value in [
            self.compiler.as_str(),
            self.edition.as_str(),
            self.package_id.as_str(),
            self.target.as_str(),
            self.profile.as_str(),
        ] {
            if value.is_empty() || value.contains(['\n', '\r']) {
                return Err(ArtifactError::InvalidInterface(
                    "required identity field is empty or contains a line break".into(),
                ));
            }
        }
        validate_sha256(&self.api_hash)?;
        require_sorted_unique("capabilities", &self.capabilities)?;
        require_sorted_unique("features", &self.features)?;
        require_sorted_unique("source sets", &self.source_sets)?;
        let package = PackageId::new(self.package_id.clone())
            .map_err(|error| ArtifactError::InvalidInterface(error.to_string()))?;
        for source_set in &self.source_sets {
            let source_set = SourceSetId::new(source_set.clone())?;
            if !source_set.belongs_to(&package) {
                return Err(ArtifactError::InvalidInterface(format!(
                    "source set `{source_set}` belongs to another package"
                )));
            }
        }
        require_sorted_unique("modules", &self.modules)?;
        let dependency_keys = self
            .dependencies
            .iter()
            .map(|dependency| dependency.alias.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("dependency aliases", &dependency_keys)?;
        for dependency in &self.dependencies {
            if dependency.alias.is_empty() || dependency.package_id.is_empty() {
                return Err(ArtifactError::InvalidInterface(
                    "dependency identity is empty".into(),
                ));
            }
            validate_sha256(&dependency.api_hash)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceHash {
    source_id: String,
    module: String,
    path: String,
    sha256: String,
}

impl SourceHash {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildArtifact {
    format: String,
    compiler: String,
    edition: String,
    source_form: String,
    package_id: String,
    target: String,
    profile: String,
    capability_registry: String,
    capabilities: Vec<String>,
    features: Vec<String>,
    source_sets: Vec<String>,
    manifest_hash: Option<String>,
    lockfile_hash: Option<String>,
    generator_inputs: BTreeMap<String, String>,
    source_hashes: Vec<SourceHash>,
    interface_hash: String,
    build_hash: String,
    reproducible: bool,
}

impl BuildArtifact {
    pub fn decode(bytes: &[u8]) -> Result<Self, ArtifactError> {
        let artifact: Self = serde_json::from_slice(bytes)
            .map_err(|error| ArtifactError::InvalidArtifact(error.to_string()))?;
        artifact.validate()?;
        if artifact.encode()? != bytes {
            return Err(ArtifactError::NonCanonicalArtifact);
        }
        Ok(artifact)
    }

    pub fn encode(&self) -> Result<Vec<u8>, ArtifactError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| ArtifactError::Serialization(error.to_string()))
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn compiler(&self) -> &str {
        &self.compiler
    }

    pub fn edition(&self) -> &str {
        &self.edition
    }

    pub fn source_form(&self) -> &str {
        &self.source_form
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn capability_registry(&self) -> &str {
        &self.capability_registry
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn interface_hash(&self) -> &str {
        &self.interface_hash
    }

    pub fn build_hash(&self) -> &str {
        &self.build_hash
    }

    pub fn source_hashes(&self) -> &[SourceHash] {
        &self.source_hashes
    }

    pub fn source_sets(&self) -> &[String] {
        &self.source_sets
    }

    pub fn manifest_hash(&self) -> Option<&str> {
        self.manifest_hash.as_deref()
    }

    pub fn lockfile_hash(&self) -> Option<&str> {
        self.lockfile_hash.as_deref()
    }

    pub fn generator_inputs(&self) -> &BTreeMap<String, String> {
        &self.generator_inputs
    }

    pub fn reproducible(&self) -> bool {
        self.reproducible
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.format != ARTIFACT_FORMAT {
            return Err(ArtifactError::UnsupportedArtifactFormat(
                self.format.clone(),
            ));
        }
        if self.compiler != COMPILER_ID {
            return Err(ArtifactError::IncompatibleCompiler(self.compiler.clone()));
        }
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(ArtifactError::UnsupportedCapabilityRegistry(
                self.capability_registry.clone(),
            ));
        }
        if !matches!(self.source_form.as_str(), "module" | "script" | "fragment") {
            return Err(ArtifactError::InvalidArtifact(format!(
                "unknown source form `{}`",
                self.source_form
            )));
        }
        for value in [
            self.edition.as_str(),
            self.source_form.as_str(),
            self.package_id.as_str(),
            self.target.as_str(),
            self.profile.as_str(),
        ] {
            if value.is_empty() || value.contains(['\n', '\r']) {
                return Err(ArtifactError::InvalidArtifact(
                    "required identity field is empty or contains a line break".into(),
                ));
            }
        }
        require_sorted_unique("capabilities", &self.capabilities)?;
        require_sorted_unique("features", &self.features)?;
        require_sorted_unique("source sets", &self.source_sets)?;
        for source_set in &self.source_sets {
            SourceSetId::new(source_set.clone())?;
        }
        if let Some(hash) = &self.manifest_hash {
            validate_sha256(hash)?;
        }
        if let Some(hash) = &self.lockfile_hash {
            validate_sha256(hash)?;
        }
        for (name, hash) in &self.generator_inputs {
            if name.is_empty() || name.contains(['\n', '\r']) {
                return Err(ArtifactError::InvalidGeneratorInput(name.clone()));
            }
            validate_sha256(hash)?;
        }
        let source_keys = self
            .source_hashes
            .iter()
            .map(|source| {
                (
                    source.source_id.clone(),
                    source.module.clone(),
                    source.path.clone(),
                )
            })
            .collect::<Vec<_>>();
        if source_keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ArtifactError::NonCanonicalList("source hashes"));
        }
        for source in &self.source_hashes {
            for value in [&source.source_id, &source.module, &source.path] {
                if value.is_empty() || value.contains(['\n', '\r']) {
                    return Err(ArtifactError::InvalidArtifact(
                        "source identity is empty or contains a line break".into(),
                    ));
                }
            }
            validate_sha256(&source.sha256)?;
        }
        validate_sha256(&self.interface_hash)?;
        validate_sha256(&self.build_hash)?;
        let expected_build_hash = self.calculated_build_hash()?;
        if self.build_hash != expected_build_hash {
            return Err(ArtifactError::InvalidArtifact(format!(
                "build hash `{}` does not match `{expected_build_hash}` derived from the artifact",
                self.build_hash
            )));
        }
        Ok(())
    }

    fn calculated_build_hash(&self) -> Result<String, ArtifactError> {
        let fingerprint = BuildFingerprint {
            compiler: &self.compiler,
            edition: &self.edition,
            source_form: &self.source_form,
            package_id: &self.package_id,
            target: &self.target,
            profile: &self.profile,
            capabilities: &self.capabilities,
            features: &self.features,
            source_sets: &self.source_sets,
            manifest_hash: self.manifest_hash.as_deref(),
            lockfile_hash: self.lockfile_hash.as_deref(),
            generator_inputs: &self.generator_inputs,
            source_hashes: &self.source_hashes,
            interface_hash: &self.interface_hash,
        };
        let bytes = serde_json::to_vec(&fingerprint)
            .map_err(|error| ArtifactError::Serialization(error.to_string()))?;
        Ok(sha256(&bytes))
    }
}

#[derive(Debug, Clone)]
pub struct BuildProducts {
    interface: CompiledInterface,
    artifact: BuildArtifact,
}

impl BuildProducts {
    pub fn interface(&self) -> &CompiledInterface {
        &self.interface
    }

    pub fn artifact(&self) -> &BuildArtifact {
        &self.artifact
    }

    pub fn into_parts(self) -> (CompiledInterface, BuildArtifact) {
        (self.interface, self.artifact)
    }
}

#[derive(Debug)]
pub enum ArtifactError {
    InvalidFeature(String),
    InvalidSourceSet(String),
    InvalidGeneratorInput(String),
    InvalidHash(String),
    InvalidInterface(String),
    InvalidArtifact(String),
    UnsupportedInterfaceFormat(String),
    UnsupportedArtifactFormat(String),
    UnsupportedCapabilityRegistry(String),
    IncompatibleCompiler(String),
    NonCanonicalInterface,
    NonCanonicalArtifact,
    NonCanonicalList(&'static str),
    MissingDependencyInterface(String),
    IncompatibleDependencyInterface { package: String, reason: String },
    MissingRootPackage(String),
    MissingResolvedSymbol(String),
    Type(TypeError),
    Serialization(String),
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFeature(value) => write!(formatter, "invalid feature name `{value}`"),
            Self::InvalidSourceSet(value) => write!(formatter, "invalid source-set ID `{value}`"),
            Self::InvalidGeneratorInput(value) => {
                write!(formatter, "invalid generator input name `{value}`")
            }
            Self::InvalidHash(value) => write!(formatter, "invalid SHA-256 identity `{value}`"),
            Self::InvalidInterface(message) => write!(formatter, "invalid interface: {message}"),
            Self::InvalidArtifact(message) => write!(formatter, "invalid artifact: {message}"),
            Self::UnsupportedInterfaceFormat(format) => {
                write!(formatter, "unsupported interface format `{format}`")
            }
            Self::UnsupportedArtifactFormat(format) => {
                write!(formatter, "unsupported artifact format `{format}`")
            }
            Self::UnsupportedCapabilityRegistry(registry) => {
                write!(formatter, "unsupported capability registry `{registry}`")
            }
            Self::IncompatibleCompiler(compiler) => {
                write!(
                    formatter,
                    "artifact was produced by incompatible compiler `{compiler}`"
                )
            }
            Self::NonCanonicalInterface => {
                formatter.write_str("compiled interface is not in canonical byte form")
            }
            Self::NonCanonicalArtifact => {
                formatter.write_str("build artifact is not in canonical byte form")
            }
            Self::NonCanonicalList(name) => {
                write!(formatter, "interface {name} are not sorted and unique")
            }
            Self::MissingDependencyInterface(package) => {
                write!(
                    formatter,
                    "missing compiled interface for dependency `{package}`"
                )
            }
            Self::IncompatibleDependencyInterface { package, reason } => {
                write!(
                    formatter,
                    "incompatible interface for `{package}`: {reason}"
                )
            }
            Self::MissingRootPackage(package) => {
                write!(formatter, "missing package metadata for `{package}`")
            }
            Self::MissingResolvedSymbol(symbol) => {
                write!(formatter, "missing resolved symbol `{symbol}`")
            }
            Self::Type(error) => error.fmt(formatter),
            Self::Serialization(message) => {
                write!(formatter, "artifact serialization failed: {message}")
            }
        }
    }
}

impl Error for ArtifactError {}

impl From<TypeError> for ArtifactError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_products(
    edition: &str,
    source_form: &str,
    target: &str,
    profile: &str,
    capabilities: impl IntoIterator<Item = String>,
    inputs: &DeclaredBuildInputs,
    packages: &PackageGraph,
    sources: &SourceDatabase,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<BuildProducts, ArtifactError> {
    let root = packages.root();
    let root_node = packages
        .package(root)
        .ok_or_else(|| ArtifactError::MissingRootPackage(root.to_string()))?;
    let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
    capabilities.sort();
    capabilities.dedup();
    let features = inputs
        .features()
        .iter()
        .map(|feature| feature.as_str().to_owned())
        .collect::<Vec<_>>();
    let source_sets = inputs
        .source_sets()
        .iter()
        .map(|source_set| source_set.as_str().to_owned())
        .collect::<Vec<_>>();
    let interface_source_sets = package_source_sets(inputs, root);
    let modules = root_node
        .modules()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let api_hashes = packages
        .packages()
        .filter(|package| package.id() != packages.standard())
        .map(|package| {
            Ok((
                package.id().clone(),
                public_api_hash(package.id(), resolved, hir)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, ArtifactError>>()?;
    let api_hash = api_hashes
        .get(root)
        .cloned()
        .ok_or_else(|| ArtifactError::MissingRootPackage(root.to_string()))?;

    for (package, interface) in inputs.dependency_interfaces() {
        let derived = api_hashes
            .get(package)
            .ok_or_else(|| ArtifactError::MissingRootPackage(package.to_string()))?;
        if interface.api_hash() != derived {
            return Err(ArtifactError::IncompatibleDependencyInterface {
                package: package.to_string(),
                reason: "API hash differs from the selected dependency sources".into(),
            });
        }
    }

    let mut dependencies = Vec::with_capacity(root_node.dependencies().len());
    for (alias, package) in root_node.dependencies() {
        let derived = api_hashes
            .get(package)
            .ok_or_else(|| ArtifactError::MissingRootPackage(package.to_string()))?;
        let api_hash = if let Some(interface) = inputs.dependency_interfaces().get(package) {
            validate_interface_for_build(
                interface,
                edition,
                package,
                target,
                profile,
                &capabilities,
                &features,
            )?;
            interface.api_hash().to_owned()
        } else if inputs.require_dependency_interfaces() {
            return Err(ArtifactError::MissingDependencyInterface(
                package.to_string(),
            ));
        } else {
            derived.clone()
        };
        dependencies.push(InterfaceDependency {
            alias: alias.to_string(),
            package_id: package.to_string(),
            api_hash,
        });
    }

    let interface = CompiledInterface::new(
        edition.to_owned(),
        root.to_string(),
        target.to_owned(),
        profile.to_owned(),
        capabilities.clone(),
        features.clone(),
        interface_source_sets,
        modules,
        api_hash,
        dependencies,
    )?;
    let mut source_hashes = sources
        .iter()
        .map(|(_, source)| SourceHash {
            source_id: source.source_id().to_string(),
            module: source.module().to_string(),
            path: source.path().to_string(),
            sha256: sha256(source.bytes()),
        })
        .collect::<Vec<_>>();
    source_hashes.sort_by(|left, right| {
        (
            left.source_id.as_str(),
            left.module.as_str(),
            left.path.as_str(),
        )
            .cmp(&(
                right.source_id.as_str(),
                right.module.as_str(),
                right.path.as_str(),
            ))
    });
    let interface_hash = interface.content_hash()?;
    let mut artifact = BuildArtifact {
        format: ARTIFACT_FORMAT.into(),
        compiler: COMPILER_ID.into(),
        edition: edition.into(),
        source_form: source_form.into(),
        package_id: root.to_string(),
        target: target.into(),
        profile: profile.into(),
        capability_registry: CAPABILITY_REGISTRY.into(),
        capabilities,
        features,
        source_sets,
        manifest_hash: inputs.manifest_hash.clone(),
        lockfile_hash: inputs.lockfile_hash.clone(),
        generator_inputs: inputs.generator_inputs.clone(),
        source_hashes,
        interface_hash,
        build_hash: String::new(),
        reproducible: true,
    };
    artifact.build_hash = artifact.calculated_build_hash()?;
    artifact.validate()?;
    Ok(BuildProducts {
        interface,
        artifact,
    })
}

pub(crate) fn validate_dependency_interfaces(
    edition: &str,
    target: &str,
    profile: &str,
    capabilities: impl IntoIterator<Item = String>,
    inputs: &DeclaredBuildInputs,
    packages: &PackageGraph,
) -> Result<(), ArtifactError> {
    if !inputs.require_dependency_interfaces() && inputs.dependency_interfaces().is_empty() {
        return Ok(());
    }
    let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
    capabilities.sort();
    capabilities.dedup();
    let features = inputs
        .features()
        .iter()
        .map(|feature| feature.as_str().to_owned())
        .collect::<Vec<_>>();
    for package in packages
        .packages()
        .filter(|package| package.id() != packages.root() && package.id() != packages.standard())
    {
        let Some(interface) = inputs.dependency_interfaces().get(package.id()) else {
            if inputs.require_dependency_interfaces() {
                return Err(ArtifactError::MissingDependencyInterface(
                    package.id().to_string(),
                ));
            }
            continue;
        };
        validate_interface_for_build(
            interface,
            package.edition().as_str(),
            package.id(),
            target,
            profile,
            &capabilities,
            &features,
        )?;
        let expected_modules = package
            .modules()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if interface.modules() != expected_modules {
            return Err(ArtifactError::IncompatibleDependencyInterface {
                package: package.id().to_string(),
                reason: "module set differs for the selected target".into(),
            });
        }
        let source_sets = package_source_sets(inputs, package.id());
        if interface.source_sets() != source_sets {
            return Err(ArtifactError::IncompatibleDependencyInterface {
                package: package.id().to_string(),
                reason: "selected source-set identity differs".into(),
            });
        }
        let mut actual_dependencies = BTreeMap::new();
        for dependency in interface.dependencies() {
            let package_id = PackageId::new(dependency.package_id().to_owned())
                .map_err(|error| ArtifactError::InvalidInterface(error.to_string()))?;
            let expected_api = inputs
                .dependency_interfaces()
                .get(&package_id)
                .map(CompiledInterface::api_hash)
                .ok_or_else(|| ArtifactError::MissingDependencyInterface(package_id.to_string()))?;
            actual_dependencies.insert(
                dependency.alias(),
                (dependency.package_id(), dependency.api_hash()),
            );
            if dependency.api_hash() != expected_api {
                return Err(ArtifactError::IncompatibleDependencyInterface {
                    package: package.id().to_string(),
                    reason: format!(
                        "dependency `{}` API hash differs from its supplied interface",
                        dependency.alias()
                    ),
                });
            }
        }
        let expected_dependencies = package
            .dependencies()
            .iter()
            .map(|(alias, package)| {
                let api = inputs
                    .dependency_interfaces()
                    .get(package)
                    .map(CompiledInterface::api_hash)
                    .ok_or_else(|| {
                        ArtifactError::MissingDependencyInterface(package.to_string())
                    })?;
                Ok((alias.as_str(), (package.as_str(), api)))
            })
            .collect::<Result<BTreeMap<_, _>, ArtifactError>>()?;
        if actual_dependencies != expected_dependencies {
            return Err(ArtifactError::IncompatibleDependencyInterface {
                package: package.id().to_string(),
                reason: "dependency aliases, exact transitive PackageIds, or API hashes differ"
                    .into(),
            });
        }
    }
    if inputs
        .dependency_interfaces()
        .keys()
        .any(|package| packages.package(package).is_none())
    {
        return Err(ArtifactError::InvalidInterface(
            "an interface names a package outside the closed graph".into(),
        ));
    }
    let root = packages
        .package(packages.root())
        .ok_or_else(|| ArtifactError::MissingRootPackage(packages.root().to_string()))?;
    if root.edition().as_str() != edition {
        return Err(ArtifactError::IncompatibleDependencyInterface {
            package: packages.root().to_string(),
            reason: "root package edition differs from the compilation request".into(),
        });
    }
    Ok(())
}

fn package_source_sets(inputs: &DeclaredBuildInputs, package: &PackageId) -> Vec<String> {
    inputs
        .source_sets()
        .iter()
        .filter(|source_set| source_set.belongs_to(package))
        .map(ToString::to_string)
        .collect()
}

fn validate_interface_for_build(
    interface: &CompiledInterface,
    edition: &str,
    package: &PackageId,
    target: &str,
    profile: &str,
    capabilities: &[String],
    features: &[String],
) -> Result<(), ArtifactError> {
    let mismatch = if interface.compiler() != COMPILER_ID {
        Some("compiler identity differs")
    } else if interface.edition() != edition {
        Some("language edition differs")
    } else if interface.package_id() != package.as_str() {
        Some("PackageId differs")
    } else if interface.target() != target {
        Some("target differs")
    } else if interface.profile() != profile {
        Some("host profile differs")
    } else if interface.capabilities() != capabilities {
        Some("target capability set differs")
    } else if interface.features() != features {
        Some("feature set differs")
    } else {
        None
    };
    if let Some(reason) = mismatch {
        return Err(ArtifactError::IncompatibleDependencyInterface {
            package: package.to_string(),
            reason: reason.into(),
        });
    }
    Ok(())
}

fn public_api_hash(
    package: &PackageId,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<String, ArtifactError> {
    let mut records = Vec::new();
    for symbol in resolved.symbols().filter(|symbol| {
        symbol.identity().package() == package
            && symbol.visibility() == Visibility::Public
            && !symbol.is_synthetic()
    }) {
        let mut record = format!(
            "symbol|{}|{:?}|{}",
            symbol.identity().canonical_name(),
            symbol.kind(),
            symbol.generic_arity()
        );
        match symbol.kind() {
            SymbolKind::Function => {
                let callable = hir
                    .callable(HirCallableId::Symbol(symbol.id()))
                    .ok_or_else(|| {
                        ArtifactError::MissingResolvedSymbol(symbol.identity().canonical_name())
                    })?;
                append_callable(&mut record, callable, resolved, hir)?;
            }
            SymbolKind::Constant => {
                let constant = hir.constant(symbol.id()).ok_or_else(|| {
                    ArtifactError::MissingResolvedSymbol(symbol.identity().canonical_name())
                })?;
                record.push('|');
                record.push_str(
                    &constant
                        .ty()
                        .map(|ty| hir.interner().canonical(ty))
                        .transpose()?
                        .unwrap_or_else(|| "<untyped>".into()),
                );
                if let Some(value) = constant.evaluated() {
                    record.push('|');
                    append_constant(&mut record, value, resolved, hir)?;
                }
            }
            SymbolKind::Type | SymbolKind::Alias | SymbolKind::Enum | SymbolKind::Trait => {
                let declaration = hir.declaration(symbol.id()).ok_or_else(|| {
                    ArtifactError::MissingResolvedSymbol(symbol.identity().canonical_name())
                })?;
                append_generics(&mut record, declaration.parameters(), resolved, hir)?;
                match declaration.kind() {
                    HirTypeDeclarationKind::Alias { target } => {
                        record.push_str("|alias|");
                        record.push_str(&hir.interner().canonical(*target)?);
                    }
                    HirTypeDeclarationKind::Nominal(definition) => {
                        record.push_str("|nominal|");
                        record.push_str(&hir.interner().canonical(definition.self_type())?);
                        append_nominal_shape(&mut record, definition.shape(), resolved, hir)?;
                        for capability in HirCapability::ALL {
                            record.push('|');
                            record.push_str(capability.as_str());
                            record.push('=');
                            record.push_str(&format!(
                                "{:?}",
                                hir.capability_status(definition.self_type(), capability)
                            ));
                        }
                        record.push_str(&format!(
                            "|terminal={:?}",
                            hir.terminal_status(definition.self_type())
                        ));
                    }
                    HirTypeDeclarationKind::Trait(definition) => {
                        record.push_str("|trait");
                        for method in definition.methods() {
                            let member = resolved.member(method.member()).ok_or_else(|| {
                                ArtifactError::MissingResolvedSymbol(format!(
                                    "member#{}",
                                    method.member().index()
                                ))
                            })?;
                            record.push('|');
                            record.push_str(member.name().as_str());
                            record.push('=');
                            record.push_str(if method.has_default() {
                                "default:"
                            } else {
                                "required:"
                            });
                            let callable = hir
                                .callable(HirCallableId::Member(method.member()))
                                .ok_or_else(|| {
                                    ArtifactError::MissingResolvedSymbol(format!(
                                        "member#{}",
                                        method.member().index()
                                    ))
                                })?;
                            append_callable(&mut record, callable, resolved, hir)?;
                        }
                    }
                }
            }
            SymbolKind::NewtypeConstructor => {}
        }
        records.push(record);
    }
    for member in resolved.members().filter(|member| {
        member.visibility() == Visibility::Public
            && member.kind().is_callable()
            && member_owner_package(member.owner(), resolved).is_some_and(|owner| owner == package)
    }) {
        let callable = hir
            .callable(HirCallableId::Member(member.id()))
            .ok_or_else(|| {
                ArtifactError::MissingResolvedSymbol(format!("member#{}", member.id().index()))
            })?;
        let mut record = format!(
            "member|{}|{:?}|{}",
            canonical_member(member.id(), resolved)?,
            member.kind(),
            member.generic_arity()
        );
        append_callable(&mut record, callable, resolved, hir)?;
        records.push(record);
    }
    for implementation in hir
        .implementations()
        .filter(|implementation| implementation.module().package() == package)
    {
        let mut record = format!(
            "impl|{}|{}",
            trait_reference(implementation.trait_reference(), resolved, hir)?,
            hir.interner().canonical(implementation.target())?
        );
        append_generics(&mut record, implementation.parameters(), resolved, hir)?;
        for method in implementation.methods() {
            record.push('|');
            record.push_str(method.name().as_str());
            if let Some(contract) = method.contract() {
                record.push('=');
                record.push_str(&hir.interner().canonical(contract.function_type())?);
            }
        }
        records.push(record);
    }
    records.sort();
    records.dedup();
    Ok(sha256(records.join("\n").as_bytes()))
}

fn append_callable(
    output: &mut String,
    callable: &crate::hir::HirCallableSignature,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<(), ArtifactError> {
    output.push('|');
    output.push_str(&hir.interner().canonical(callable.function_type())?);
    append_generics(output, callable.generics(), resolved, hir)?;
    output.push_str("|parameters=");
    for (index, parameter) in callable.parameters().iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        if parameter.is_receiver() {
            output.push_str("self");
        } else if let Some(local) = parameter.local() {
            output.push_str(
                resolved
                    .local(local)
                    .map(|local| local.name().as_str())
                    .unwrap_or("_"),
            );
        } else {
            output.push('_');
        }
        output.push(':');
        output.push_str(&format!("{:?}:", parameter.mode()));
        if let Some(element) = parameter.variadic_element() {
            output.push_str("...");
            output.push_str(&hir.interner().canonical(element)?);
        } else {
            output.push_str(&hir.interner().canonical(parameter.ty())?);
        }
    }
    if let Some(opaque) = callable.opaque_result() {
        output.push_str("|opaque=");
        for bound in opaque.bounds() {
            output.push_str(&trait_reference(bound, resolved, hir)?);
            output.push('+');
        }
    }
    Ok(())
}

fn append_generics(
    output: &mut String,
    parameters: &[HirGenericParameter],
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<(), ArtifactError> {
    output.push_str("|generics=");
    for parameter in parameters {
        output.push_str(&parameter.position().to_string());
        output.push(':');
        for bound in parameter.bounds() {
            output.push_str(&trait_reference(bound, resolved, hir)?);
            output.push('+');
        }
        output.push(';');
    }
    Ok(())
}

fn append_nominal_shape(
    output: &mut String,
    shape: &HirNominalShape,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<(), ArtifactError> {
    match shape {
        HirNominalShape::Newtype { underlying } => {
            output.push_str("|newtype:");
            output.push_str(&hir.interner().canonical(*underlying)?);
        }
        HirNominalShape::Record { fields } => {
            output.push_str("|record:");
            for field in fields {
                let member = resolved.member(field.member()).ok_or_else(|| {
                    ArtifactError::MissingResolvedSymbol(format!(
                        "member#{}",
                        field.member().index()
                    ))
                })?;
                output.push_str(member.name().as_str());
                output.push(':');
                output.push_str(&format!("{:?}:", member.visibility()));
                output.push_str(&hir.interner().canonical(field.ty())?);
                output.push(';');
            }
        }
        HirNominalShape::Enum { variants } => {
            output.push_str("|enum:");
            for variant in variants {
                let member = resolved.member(variant.member()).ok_or_else(|| {
                    ArtifactError::MissingResolvedSymbol(format!(
                        "member#{}",
                        variant.member().index()
                    ))
                })?;
                output.push_str(member.name().as_str());
                match variant.payload() {
                    HirVariantPayload::Unit => output.push_str("();"),
                    HirVariantPayload::Tuple(items) => {
                        output.push('(');
                        for item in items {
                            output.push_str(&hir.interner().canonical(*item)?);
                            output.push(',');
                        }
                        output.push_str(");");
                    }
                    HirVariantPayload::Record(fields) => {
                        output.push('{');
                        for field in fields {
                            let field_member =
                                resolved.member(field.member()).ok_or_else(|| {
                                    ArtifactError::MissingResolvedSymbol(format!(
                                        "member#{}",
                                        field.member().index()
                                    ))
                                })?;
                            output.push_str(field_member.name().as_str());
                            output.push(':');
                            output.push_str(&hir.interner().canonical(field.ty())?);
                            output.push(',');
                        }
                        output.push_str("};");
                    }
                }
            }
        }
    }
    Ok(())
}

fn trait_reference(
    reference: &HirTraitReference,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<String, ArtifactError> {
    let mut output = match reference.constructor() {
        HirTraitConstructor::Symbol(symbol) => resolved
            .symbol(*symbol)
            .ok_or_else(|| {
                ArtifactError::MissingResolvedSymbol(format!("symbol#{}", symbol.index()))
            })?
            .identity()
            .canonical_name(),
        HirTraitConstructor::Prelude(name) => format!("prelude:{}", name.as_str()),
        HirTraitConstructor::External(identity) => identity.canonical_name(),
    };
    output.push('[');
    for argument in reference.arguments() {
        output.push_str(&hir.interner().canonical(*argument)?);
        output.push(',');
    }
    output.push(']');
    Ok(output)
}

fn append_constant(
    output: &mut String,
    value: &HirConstantValue,
    resolved: &ResolvedProgram,
    hir: &HirProgram,
) -> Result<(), ArtifactError> {
    match value.kind() {
        HirConstantValueKind::Unit => output.push_str("unit"),
        HirConstantValueKind::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        HirConstantValueKind::Integer(value) => output.push_str(&value.to_string()),
        HirConstantValueKind::Float(bits) => output.push_str(&format!("float:{bits:016x}")),
        HirConstantValueKind::Char(value) => output.push_str(&format!("char:{value:?}")),
        HirConstantValueKind::String(value) => output.push_str(&format!("string:{value:?}")),
        HirConstantValueKind::Function {
            callable,
            arguments,
        } => {
            output.push_str("fn:");
            output.push_str(&canonical_callable(*callable, resolved)?);
            for argument in arguments {
                output.push(':');
                output.push_str(&hir.interner().canonical(*argument)?);
            }
        }
        HirConstantValueKind::Tuple(items)
        | HirConstantValueKind::Array(items)
        | HirConstantValueKind::Set(items) => {
            output.push('[');
            for item in items {
                append_constant(output, item, resolved, hir)?;
                output.push(',');
            }
            output.push(']');
        }
        HirConstantValueKind::Map(entries) => {
            output.push('[');
            for (key, item) in entries {
                append_constant(output, key, resolved, hir)?;
                output.push(':');
                append_constant(output, item, resolved, hir)?;
                output.push(',');
            }
            output.push(']');
        }
        HirConstantValueKind::Newtype { constructor, value } => {
            output.push_str(&canonical_symbol(*constructor, resolved)?);
            output.push('(');
            append_constant(output, value, resolved, hir)?;
            output.push(')');
        }
        HirConstantValueKind::Record { owner, fields } => {
            output.push_str(&canonical_symbol(*owner, resolved)?);
            output.push('{');
            for field in fields {
                output.push_str(&canonical_member(field.member(), resolved)?);
                output.push(':');
                append_constant(output, field.value(), resolved, hir)?;
                output.push(',');
            }
            output.push('}');
        }
        HirConstantValueKind::Variant { variant, payload } => {
            output.push_str(&canonical_member(*variant, resolved)?);
            match payload {
                crate::hir::HirConstantVariantValue::Unit => output.push_str("()"),
                crate::hir::HirConstantVariantValue::Tuple(items) => {
                    output.push('(');
                    for item in items {
                        append_constant(output, item, resolved, hir)?;
                        output.push(',');
                    }
                    output.push(')');
                }
                crate::hir::HirConstantVariantValue::Record(fields) => {
                    output.push('{');
                    for field in fields {
                        output.push_str(&canonical_member(field.member(), resolved)?);
                        output.push(':');
                        append_constant(output, field.value(), resolved, hir)?;
                        output.push(',');
                    }
                    output.push('}');
                }
            }
        }
        HirConstantValueKind::NumericConversionError(variant) => {
            output.push_str("conversion:");
            output.push_str(variant.as_str());
        }
        HirConstantValueKind::OptionNone => output.push_str("none"),
        HirConstantValueKind::OptionSome(value) => {
            output.push_str("some(");
            append_constant(output, value, resolved, hir)?;
            output.push(')');
        }
        HirConstantValueKind::ResultOk(value) => {
            output.push_str("ok(");
            append_constant(output, value, resolved, hir)?;
            output.push(')');
        }
        HirConstantValueKind::ResultErr(value) => {
            output.push_str("err(");
            append_constant(output, value, resolved, hir)?;
            output.push(')');
        }
        HirConstantValueKind::Range { kind, start, end } => {
            append_constant(output, start, resolved, hir)?;
            output.push_str(if matches!(kind, crate::hir::HirRangeKind::Inclusive) {
                "..="
            } else {
                ".."
            });
            append_constant(output, end, resolved, hir)?;
        }
        HirConstantValueKind::Converted(value) => {
            output.push_str("converted(");
            append_constant(output, value, resolved, hir)?;
            output.push(')');
        }
    }
    Ok(())
}

fn canonical_callable(
    callable: HirCallableId,
    resolved: &ResolvedProgram,
) -> Result<String, ArtifactError> {
    match callable {
        HirCallableId::Symbol(symbol) => canonical_symbol(symbol, resolved),
        HirCallableId::Member(member) => canonical_member(member, resolved),
        HirCallableId::Implementation(HirImplementationMethodId { .. }) => {
            Ok(format!("implementation:{callable:?}"))
        }
        HirCallableId::Host(host) => Ok(format!("host:{}", host.name())),
    }
}

fn canonical_symbol(symbol: SymbolId, resolved: &ResolvedProgram) -> Result<String, ArtifactError> {
    resolved
        .symbol(symbol)
        .map(|symbol| symbol.identity().canonical_name())
        .ok_or_else(|| ArtifactError::MissingResolvedSymbol(format!("symbol#{}", symbol.index())))
}

fn canonical_member(member: MemberId, resolved: &ResolvedProgram) -> Result<String, ArtifactError> {
    let member_info = resolved.member(member).ok_or_else(|| {
        ArtifactError::MissingResolvedSymbol(format!("member#{}", member.index()))
    })?;
    let owner = match member_info.owner() {
        MemberOwner::Type(symbol) => canonical_symbol(symbol, resolved)?,
        MemberOwner::Variant(variant) => canonical_member(variant, resolved)?,
    };
    Ok(format!("{owner}.{}", member_info.name()))
}

fn member_owner_package(owner: MemberOwner, resolved: &ResolvedProgram) -> Option<&PackageId> {
    match owner {
        MemberOwner::Type(symbol) => resolved
            .symbol(symbol)
            .map(|symbol| symbol.identity().package()),
        MemberOwner::Variant(variant) => resolved
            .member(variant)
            .and_then(|member| member_owner_package(member.owner(), resolved)),
    }
}

#[derive(Serialize)]
struct BuildFingerprint<'a> {
    compiler: &'a str,
    edition: &'a str,
    source_form: &'a str,
    package_id: &'a str,
    target: &'a str,
    profile: &'a str,
    capabilities: &'a [String],
    features: &'a [String],
    source_sets: &'a [String],
    manifest_hash: Option<&'a str>,
    lockfile_hash: Option<&'a str>,
    generator_inputs: &'a BTreeMap<String, String>,
    source_hashes: &'a [SourceHash],
    interface_hash: &'a str,
}

pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity("sha256:".len() + digest.len() * 2);
    output.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

pub fn validate_sha256(value: &str) -> Result<(), ArtifactError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(ArtifactError::InvalidHash(value.into()));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ArtifactError::InvalidHash(value.into()));
    }
    Ok(())
}

fn require_sorted_unique(name: &'static str, values: &[String]) -> Result<(), ArtifactError> {
    if values.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(ArtifactError::NonCanonicalList(name))
    }
}

fn is_kebab_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn source_set_parts(value: &str) -> Option<(&str, &str)> {
    if value.contains(['\n', '\r']) {
        return None;
    }
    let (length, remainder) = value.strip_prefix('@')?.split_once(':')?;
    if length.is_empty()
        || !length.bytes().all(|byte| byte.is_ascii_digit())
        || (length.starts_with('0') && length != "0")
    {
        return None;
    }
    let length = length.parse::<usize>().ok()?;
    if length == 0 || length >= remainder.len() || !remainder.is_char_boundary(length) {
        return None;
    }
    let (package, local) = remainder.split_at(length);
    let local = local.strip_prefix('#')?;
    PackageId::new(package.to_owned()).ok()?;
    is_kebab_identifier(local).then_some((package, local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{Edition, PackageAlias, PackageNode};
    use crate::source::{ModulePath, SourceId};

    #[test]
    fn hashes_and_names_have_closed_canonical_forms() {
        assert_eq!(
            sha256(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        validate_sha256(&sha256(b"tondo")).unwrap();
        assert!(validate_sha256("SHA256:00").is_err());
        assert!(FeatureName::new("native-ffi").is_ok());
        assert!(FeatureName::new("Native").is_err());
        assert!(SourceSetId::for_package(&PackageId::new("pkg:app@1").unwrap(), "hosted").is_ok());
        assert_ne!(
            SourceSetId::for_package(&PackageId::new("a").unwrap(), "b-c").unwrap(),
            SourceSetId::for_package(&PackageId::new("a#b").unwrap(), "c").unwrap()
        );
        assert!(SourceSetId::new("@01:a#common").is_err());
        assert!(SourceSetId::new("").is_err());
    }

    #[test]
    fn interfaces_require_canonical_bytes_and_sorted_identity_sets() {
        let interface = CompiledInterface::new(
            "0.1".into(),
            "pkg:app@1".into(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            vec!["console".into(), "process".into()],
            vec!["logging".into()],
            vec!["@9:pkg:app@1#common".into()],
            vec!["main".into()],
            sha256(b"api"),
            Vec::new(),
        )
        .unwrap();
        let bytes = interface.encode().unwrap();
        assert_eq!(CompiledInterface::decode(&bytes).unwrap(), interface);

        let mut pretty = serde_json::to_vec_pretty(&interface).unwrap();
        pretty.push(b'\n');
        assert!(matches!(
            CompiledInterface::decode(&pretty),
            Err(ArtifactError::NonCanonicalInterface)
        ));

        let invalid = CompiledInterface::new(
            "0.1".into(),
            "pkg:app@1".into(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            vec!["process".into(), "console".into()],
            Vec::new(),
            Vec::new(),
            vec!["main".into()],
            sha256(b"api"),
            Vec::new(),
        );
        assert!(matches!(
            invalid,
            Err(ArtifactError::NonCanonicalList("capabilities"))
        ));
    }

    #[test]
    fn dependency_interface_identity_mismatches_are_closed() {
        let package = PackageId::new("registry:dependency@1#content").unwrap();
        let expected_capabilities = vec!["console".into(), "process".into()];
        let expected_features = vec!["fast".into()];
        let interface = CompiledInterface::new(
            "0.1".into(),
            package.to_string(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            expected_capabilities.clone(),
            expected_features.clone(),
            vec!["@29:registry:dependency@1#content#common".into()],
            vec!["api".into()],
            sha256(b"api"),
            Vec::new(),
        )
        .unwrap();
        let mut cases = Vec::new();
        let mut incompatible = interface.clone();
        incompatible.compiler = "another-compiler/1".into();
        cases.push(("compiler identity differs", incompatible));
        let mut incompatible = interface.clone();
        incompatible.edition = "0.2".into();
        cases.push(("language edition differs", incompatible));
        let mut incompatible = interface.clone();
        incompatible.package_id = "registry:other@1#content".into();
        cases.push(("PackageId differs", incompatible));
        let mut incompatible = interface.clone();
        incompatible.target = "another-target".into();
        cases.push(("target differs", incompatible));
        let mut incompatible = interface.clone();
        incompatible.profile = "another-profile".into();
        cases.push(("host profile differs", incompatible));
        let mut incompatible = interface.clone();
        incompatible.capabilities = vec!["console".into()];
        cases.push(("target capability set differs", incompatible));
        let mut incompatible = interface;
        incompatible.features = Vec::new();
        cases.push(("feature set differs", incompatible));

        for (expected_reason, incompatible) in cases {
            assert!(matches!(
                validate_interface_for_build(
                    &incompatible,
                    "0.1",
                    &package,
                    "tondo-vm-hosted",
                    "hosted",
                    &expected_capabilities,
                    &expected_features,
                ),
                Err(ArtifactError::IncompatibleDependencyInterface { reason, .. })
                    if reason == expected_reason
            ));
        }
    }

    #[test]
    fn interface_source_sets_are_scoped_to_their_package() {
        let inputs = DeclaredBuildInputs::new(
            BTreeSet::new(),
            [
                SourceSetId::new("@29:registry:dependency@1#content#common").unwrap(),
                SourceSetId::new("@15:workspace:app@1#common").unwrap(),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            package_source_sets(
                &inputs,
                &PackageId::new("registry:dependency@1#content").unwrap()
            ),
            ["@29:registry:dependency@1#content#common"]
        );
    }

    #[test]
    fn artifacts_require_canonical_bytes_and_exact_compiler_identity() {
        let mut artifact = BuildArtifact {
            format: ARTIFACT_FORMAT.into(),
            compiler: COMPILER_ID.into(),
            edition: "0.1".into(),
            source_form: "module".into(),
            package_id: "workspace:app@1".into(),
            target: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capability_registry: CAPABILITY_REGISTRY.into(),
            capabilities: vec!["console".into(), "process".into()],
            features: vec!["fast".into()],
            source_sets: vec!["@15:workspace:app@1#common".into()],
            manifest_hash: Some(sha256(b"manifest")),
            lockfile_hash: Some(sha256(b"lockfile")),
            generator_inputs: BTreeMap::new(),
            source_hashes: vec![SourceHash {
                source_id: "pkg:15:workspace:app@1".into(),
                module: "main".into(),
                path: "src/main.to".into(),
                sha256: sha256(b"fn main() {}\n"),
            }],
            interface_hash: sha256(b"interface"),
            build_hash: String::new(),
            reproducible: true,
        };
        artifact.build_hash = artifact.calculated_build_hash().unwrap();
        let bytes = artifact.encode().unwrap();
        assert_eq!(BuildArtifact::decode(&bytes).unwrap(), artifact);

        let mut pretty = serde_json::to_vec_pretty(&artifact).unwrap();
        pretty.push(b'\n');
        assert!(matches!(
            BuildArtifact::decode(&pretty),
            Err(ArtifactError::NonCanonicalArtifact)
        ));

        let mut forged = artifact.clone();
        forged.build_hash = sha256(b"forged");
        assert!(matches!(
            forged.encode(),
            Err(ArtifactError::InvalidArtifact(message))
                if message.contains("does not match")
        ));

        let mut incompatible = artifact;
        incompatible.compiler = "another-compiler/1".into();
        assert!(matches!(
            incompatible.encode(),
            Err(ArtifactError::IncompatibleCompiler(_))
        ));
    }

    #[test]
    fn dependency_modules_source_sets_and_transitive_edges_must_match() {
        let root = PackageId::new("workspace:app@1").unwrap();
        let dependency = PackageId::new("registry:util@1#content").unwrap();
        let transitive = PackageId::new("registry:core@1#content").unwrap();
        let standard = PackageId::new("toolchain:std@1").unwrap();
        let graph = PackageGraph::new(
            root.clone(),
            standard.clone(),
            [
                PackageNode::new(
                    root,
                    SourceId::new("pkg:app").unwrap(),
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    [ModulePath::new("main").unwrap()],
                    [(PackageAlias::new("util").unwrap(), dependency.clone())],
                )
                .unwrap(),
                PackageNode::new(
                    dependency.clone(),
                    SourceId::new("pkg:util").unwrap(),
                    PackageAlias::new("utilPackage").unwrap(),
                    Edition::V0_1,
                    [ModulePath::new("util").unwrap()],
                    [(PackageAlias::new("core").unwrap(), transitive.clone())],
                )
                .unwrap(),
                PackageNode::new(
                    transitive.clone(),
                    SourceId::new("pkg:core").unwrap(),
                    PackageAlias::new("corePackage").unwrap(),
                    Edition::V0_1,
                    [ModulePath::new("core").unwrap()],
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    standard,
                    SourceId::new("pkg:std").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let capabilities = vec!["console".into(), "process".into()];
        let features = vec!["fast".into()];
        let core_api = sha256(b"core-api");
        let core = CompiledInterface::new(
            "0.1".into(),
            transitive.to_string(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            capabilities.clone(),
            features.clone(),
            vec!["@23:registry:core@1#content#common".into()],
            vec!["core".into()],
            core_api.clone(),
            Vec::new(),
        )
        .unwrap();
        let util = CompiledInterface::new(
            "0.1".into(),
            dependency.to_string(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            capabilities.clone(),
            features.clone(),
            vec!["@23:registry:util@1#content#common".into()],
            vec!["util".into()],
            sha256(b"util-api"),
            vec![InterfaceDependency {
                alias: "core".into(),
                package_id: transitive.to_string(),
                api_hash: core_api,
            }],
        )
        .unwrap();
        let inputs = DeclaredBuildInputs::new(
            [FeatureName::new("fast").unwrap()].into_iter().collect(),
            [
                SourceSetId::new("@23:registry:core@1#content#common").unwrap(),
                SourceSetId::new("@23:registry:util@1#content#common").unwrap(),
                SourceSetId::new("@15:workspace:app@1#common").unwrap(),
            ]
            .into_iter()
            .collect(),
        )
        .with_dependency_interfaces(
            BTreeMap::from([(dependency.clone(), util), (transitive, core)]),
            true,
        );
        let validate = |inputs: &DeclaredBuildInputs| {
            validate_dependency_interfaces(
                "0.1",
                "tondo-vm-hosted",
                "hosted",
                capabilities.clone(),
                inputs,
                &graph,
            )
        };
        validate(&inputs).unwrap();

        let mut wrong_modules = inputs.clone();
        wrong_modules
            .dependency_interfaces
            .get_mut(&dependency)
            .unwrap()
            .modules = vec!["other".into()];
        assert!(matches!(
            validate(&wrong_modules),
            Err(ArtifactError::IncompatibleDependencyInterface { reason, .. })
                if reason == "module set differs for the selected target"
        ));

        let mut wrong_source_sets = inputs.clone();
        wrong_source_sets
            .dependency_interfaces
            .get_mut(&dependency)
            .unwrap()
            .source_sets = vec!["@23:registry:util@1#content#other".into()];
        assert!(matches!(
            validate(&wrong_source_sets),
            Err(ArtifactError::IncompatibleDependencyInterface { reason, .. })
                if reason == "selected source-set identity differs"
        ));

        let mut wrong_transitive_hash = inputs;
        wrong_transitive_hash
            .dependency_interfaces
            .get_mut(&dependency)
            .unwrap()
            .dependencies[0]
            .api_hash = sha256(b"wrong-core-api");
        assert!(matches!(
            validate(&wrong_transitive_hash),
            Err(ArtifactError::IncompatibleDependencyInterface { reason, .. })
                if reason.contains("API hash differs")
        ));
    }
}
