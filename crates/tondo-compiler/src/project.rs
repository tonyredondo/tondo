//! Pure, closed project planning for the bootstrap toolchain.
//!
//! This module deliberately has no filesystem, environment, clock, process, or
//! network API. Tooling parses a manifest and lockfile, asks the plan for the
//! exact byte inputs it needs, and then resolves only those supplied bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::artifact::{
    CAPABILITY_REGISTRY, COMPILER_ID, CompiledInterface, DeclaredBuildInputs, FeatureName,
    SourceSetId, sha256, validate_sha256,
};
use crate::driver::{
    BuildTarget, CapabilityName, CompilationRequest, DiagnosticFormat, DriverError, HostProfile,
    Operation, ResourceLimits, SourceForm,
};
use crate::package::{
    Edition, PackageAlias, PackageGraph, PackageGraphError, PackageId, PackageNode,
};
use crate::source::{
    FileId, LogicalPath, ModulePath, SourceDatabase, SourceError, SourceId, SourceInput,
    SourceOrigin,
};

pub const MANIFEST_FORMAT: &str = "tondo-manifest-draft";
pub const LOCKFILE_FORMAT: &str = "tondo-lock-draft";
pub const PRIVILEGED_UNIT_FORMAT: &str = "tondo-privileged-unit-draft";
pub const BOOTSTRAP_STANDARD_PACKAGE: &str = "toolchain:std:0.1-bootstrap";

const BOOTSTRAP_STANDARD_FINGERPRINT: &[u8] =
    b"tondo-bootstrap-standard/0.1;modules=console,process;compiler-owned";

pub fn bootstrap_standard_hash() -> String {
    sha256(BOOTSTRAP_STANDARD_FINGERPRINT)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectInputKind {
    Source,
    DependencyInterface,
    GeneratorInput,
    PrivilegedUnit,
}

impl ProjectInputKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::DependencyInterface => "dependency-interface",
            Self::GeneratorInput => "generator-input",
            Self::PrivilegedUnit => "privileged-unit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredProjectInput {
    path: String,
    kind: ProjectInputKind,
    sha256: String,
}

impl RequiredProjectInput {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn kind(&self) -> ProjectInputKind {
        self.kind
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivilegedExposure {
    UnsafeFunction,
    SafeWrapper,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegedBinding {
    canonical_name: String,
    exposure: PrivilegedExposure,
    signature_hash: String,
    safety_contract_hash: String,
    implementation_hash: String,
}

impl PrivilegedBinding {
    pub fn canonical_name(&self) -> &str {
        &self.canonical_name
    }

    pub fn exposure(&self) -> &PrivilegedExposure {
        &self.exposure
    }

    pub fn signature_hash(&self) -> &str {
        &self.signature_hash
    }

    pub fn safety_contract_hash(&self) -> &str {
        &self.safety_contract_hash
    }

    pub fn implementation_hash(&self) -> &str {
        &self.implementation_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivilegedUnit {
    format: String,
    id: String,
    provider: String,
    compiler: String,
    target: String,
    profile: String,
    capability_registry: String,
    required_capabilities: Vec<String>,
    bindings: Vec<PrivilegedBinding>,
}

impl PrivilegedUnit {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProjectError> {
        let unit: Self = serde_json::from_slice(bytes)
            .map_err(|error| ProjectError::InvalidPrivilegedUnit(error.to_string()))?;
        unit.validate()?;
        if unit.encode()? != bytes {
            return Err(ProjectError::NonCanonicalPrivilegedUnit(unit.id));
        }
        Ok(unit)
    }

    pub fn encode(&self) -> Result<Vec<u8>, ProjectError> {
        self.validate()?;
        serde_json::to_vec(self)
            .map_err(|error| ProjectError::InvalidPrivilegedUnit(error.to_string()))
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn required_capabilities(&self) -> &[String] {
        &self.required_capabilities
    }

    pub fn bindings(&self) -> &[PrivilegedBinding] {
        &self.bindings
    }

    fn validate(&self) -> Result<(), ProjectError> {
        if self.format != PRIVILEGED_UNIT_FORMAT {
            return Err(ProjectError::InvalidPrivilegedUnit(format!(
                "unsupported format `{}`",
                self.format
            )));
        }
        if self.compiler != COMPILER_ID {
            return Err(ProjectError::InvalidPrivilegedUnit(format!(
                "unit `{}` targets compiler `{}`, expected `{COMPILER_ID}`",
                self.id, self.compiler
            )));
        }
        if self.capability_registry != CAPABILITY_REGISTRY {
            return Err(ProjectError::InvalidPrivilegedUnit(format!(
                "unit `{}` uses unsupported capability registry `{}`",
                self.id, self.capability_registry
            )));
        }
        validate_unit_id(&self.id)?;
        PackageId::new(self.provider.clone())?;
        validate_identity_field("target", &self.target)?;
        validate_identity_field("profile", &self.profile)?;
        require_sorted_unique("privileged-unit capabilities", &self.required_capabilities)?;
        for capability in &self.required_capabilities {
            CapabilityName::new(capability.clone())?;
        }
        let binding_names = self
            .bindings
            .iter()
            .map(|binding| binding.canonical_name.clone())
            .collect::<Vec<_>>();
        require_sorted_unique("privileged-unit bindings", &binding_names)?;
        for binding in &self.bindings {
            validate_identity_field("binding canonical name", &binding.canonical_name)?;
            validate_sha256(&binding.signature_hash)?;
            validate_sha256(&binding.safety_contract_hash)?;
            validate_sha256(&binding.implementation_hash)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ProjectPlan {
    manifest_hash: String,
    lockfile_hash: String,
    target: PlannedTarget,
    root: PlannedRoot,
    standard: PackageId,
    packages: BTreeMap<PackageId, PlannedPackage>,
    selected_sources: Vec<PlannedSource>,
    selected_source_sets: BTreeSet<SourceSetId>,
    required_inputs: BTreeMap<String, PlannedInput>,
    generator_names: BTreeMap<String, String>,
    privileged_ids: BTreeMap<String, String>,
}

impl ProjectPlan {
    pub fn parse(manifest_bytes: &[u8], lockfile_bytes: &[u8]) -> Result<Self, ProjectError> {
        let manifest: ManifestWire = serde_json::from_slice(manifest_bytes)
            .map_err(|error| ProjectError::InvalidManifest(error.to_string()))?;
        let lockfile: LockfileWire = serde_json::from_slice(lockfile_bytes)
            .map_err(|error| ProjectError::InvalidLockfile(error.to_string()))?;
        Self::from_wire(manifest_bytes, lockfile_bytes, manifest, lockfile)
    }

    /// Parses the current draft toolchain records. The returned plan is pure:
    /// it only validates supplied bytes and enumerates required inputs.
    pub fn parse_draft(
        manifest_bytes: &[u8],
        lockfile_bytes: &[u8],
        descriptor_bytes: &[u8],
    ) -> Result<crate::toolchain::ProjectPlanDraft, crate::toolchain::FormatError> {
        crate::toolchain::ProjectPlanDraft::parse(manifest_bytes, lockfile_bytes, descriptor_bytes)
    }

    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    pub fn lockfile_hash(&self) -> &str {
        &self.lockfile_hash
    }

    pub fn target_name(&self) -> &str {
        &self.target.name
    }

    /// Canonical source path selected as the project entry point.
    pub fn root_source_path(&self) -> &str {
        &self.root.physical_path
    }

    pub fn profile(&self) -> HostProfile {
        self.target.profile
    }

    pub fn capabilities(&self) -> &BTreeSet<CapabilityName> {
        &self.target.capabilities
    }

    pub fn features(&self) -> &BTreeSet<FeatureName> {
        &self.target.features
    }

    /// PackageIds admitted by this closed project graph. Test planning uses
    /// this read-only view to reject source records that invent a package;
    /// dependency resolution remains owned by the production project plan.
    pub fn package_ids(&self) -> impl ExactSizeIterator<Item = &str> {
        self.packages.keys().map(PackageId::as_str)
    }

    /// Parse the test-plan extension against this already validated project.
    /// The extension is pure and does not alter the production resolution.
    pub fn parse_test_plan(
        &self,
        bytes: &[u8],
    ) -> Result<crate::test_plan::TestProjectPlan, crate::test_plan::TestPlanError> {
        crate::test_plan::TestProjectPlan::parse(self, bytes)
    }

    pub fn selected_source_sets(&self) -> &BTreeSet<SourceSetId> {
        &self.selected_source_sets
    }

    /// Physical source paths in the canonical production insertion order.
    pub fn selected_source_paths(&self) -> impl ExactSizeIterator<Item = &str> {
        self.selected_sources
            .iter()
            .map(|source| source.physical_path.as_str())
    }

    /// Canonical metadata for the sources selected by the closed project.
    ///
    /// Test defaults use this view to materialize an in-memory test plan
    /// without asking callers to repeat the production source graph in a
    /// sidecar file.
    pub fn selected_source_records(
        &self,
    ) -> impl ExactSizeIterator<Item = (&str, &str, &str, &str)> + '_ {
        self.selected_sources.iter().map(|source| {
            (
                source.package.as_str(),
                source.physical_path.as_str(),
                source.logical_path.as_str(),
                source.module.as_str(),
            )
        })
    }

    pub fn required_inputs(&self) -> impl ExactSizeIterator<Item = RequiredProjectInput> + '_ {
        self.required_inputs
            .iter()
            .map(|(path, input)| RequiredProjectInput {
                path: path.clone(),
                kind: input.kind,
                sha256: input.sha256.clone(),
            })
    }

    pub fn resolve(
        &self,
        supplied: &BTreeMap<String, Arc<[u8]>>,
    ) -> Result<ResolvedProject, ProjectError> {
        let source_order = self.selected_sources.iter().collect::<Vec<_>>();
        self.resolve_in_source_order(supplied, &source_order)
    }

    /// Resolves the same closed project while deliberately perturbing only
    /// `SourceDatabase` insertion order.
    ///
    /// This hook exists for reproducibility audits. Production callers use
    /// [`Self::resolve`], whose order is canonical. The supplied permutation
    /// must contain every selected physical source path exactly once; it
    /// cannot add, remove, or substitute an input.
    pub fn resolve_with_source_order(
        &self,
        supplied: &BTreeMap<String, Arc<[u8]>>,
        source_order: &[String],
    ) -> Result<ResolvedProject, ProjectError> {
        let expected = self
            .selected_sources
            .iter()
            .map(|source| source.physical_path.as_str())
            .collect::<BTreeSet<_>>();
        let actual = source_order
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if source_order.len() != expected.len() || actual != expected {
            return Err(ProjectError::InvalidSourceOrder(
                "source order must be an exact permutation of selected project sources".into(),
            ));
        }
        let by_path = self
            .selected_sources
            .iter()
            .map(|source| (source.physical_path.as_str(), source))
            .collect::<BTreeMap<_, _>>();
        let source_order = source_order
            .iter()
            .map(|path| by_path[path.as_str()])
            .collect::<Vec<_>>();
        self.resolve_in_source_order(supplied, &source_order)
    }

    fn resolve_in_source_order(
        &self,
        supplied: &BTreeMap<String, Arc<[u8]>>,
        source_order: &[&PlannedSource],
    ) -> Result<ResolvedProject, ProjectError> {
        for path in supplied.keys() {
            if !self.required_inputs.contains_key(path) {
                return Err(ProjectError::UndeclaredInput(path.clone()));
            }
        }
        for (path, expected) in &self.required_inputs {
            let bytes = supplied
                .get(path)
                .ok_or_else(|| ProjectError::MissingInput(path.clone()))?;
            let actual = sha256(bytes);
            if actual != expected.sha256 {
                return Err(ProjectError::InputHashMismatch {
                    path: path.clone(),
                    expected: expected.sha256.clone(),
                    actual,
                });
            }
        }

        let mut interfaces = BTreeMap::new();
        for (package, planned) in &self.packages {
            let Some(path) = &planned.interface_path else {
                continue;
            };
            let bytes = supplied
                .get(path)
                .expect("required interface bytes were checked above");
            let interface = CompiledInterface::decode(bytes)?;
            if interface.package_id() != package.as_str() {
                return Err(ProjectError::InterfacePackageMismatch {
                    expected: package.to_string(),
                    actual: interface.package_id().to_owned(),
                });
            }
            interfaces.insert(package.clone(), interface);
        }

        let mut privileged_units = BTreeMap::new();
        for (id, path) in &self.privileged_ids {
            let bytes = supplied
                .get(path)
                .expect("required privileged-unit bytes were checked above");
            let unit = PrivilegedUnit::decode(bytes)?;
            if unit.id() != id {
                return Err(ProjectError::PrivilegedUnitIdMismatch {
                    expected: id.clone(),
                    actual: unit.id().to_owned(),
                });
            }
            if unit.target() != self.target.name {
                return Err(ProjectError::InvalidPrivilegedUnit(format!(
                    "unit `{id}` targets `{}`, expected `{}`",
                    unit.target(),
                    self.target.name
                )));
            }
            if unit.profile() != self.target.profile.as_str() {
                return Err(ProjectError::InvalidPrivilegedUnit(format!(
                    "unit `{id}` targets profile `{}`, expected `{}`",
                    unit.profile(),
                    self.target.profile.as_str()
                )));
            }
            for required in unit.required_capabilities() {
                let required = CapabilityName::new(required.clone())?;
                if !self.target.capabilities.contains(&required) {
                    return Err(ProjectError::InvalidPrivilegedUnit(format!(
                        "unit `{id}` requires missing capability `{required}`"
                    )));
                }
            }
            privileged_units.insert(id.clone(), unit);
        }

        let mut sources = SourceDatabase::new();
        let mut source_files = BTreeMap::new();
        for source in source_order {
            let bytes = supplied
                .get(&source.physical_path)
                .expect("required source bytes were checked above");
            let file = sources.add(SourceInput::new(
                package_source_id(&source.package)?,
                source.module.clone(),
                source.logical_path.clone(),
                SourceOrigin::Physical,
                Arc::clone(bytes),
            ))?;
            source_files.insert((source.package.clone(), source.physical_path.clone()), file);
        }
        let root = source_files
            .get(&(self.root.package.clone(), self.root.physical_path.clone()))
            .copied()
            .ok_or_else(|| ProjectError::InactiveRootSource {
                package: self.root.package.to_string(),
                path: self.root.physical_path.clone(),
            })?;

        let mut nodes = Vec::with_capacity(self.packages.len().saturating_add(1));
        for package in self.packages.values() {
            let modules = self
                .selected_sources
                .iter()
                .filter(|source| source.package == package.id)
                .map(|source| source.module.clone())
                .collect::<BTreeSet<_>>();
            nodes.push(PackageNode::new(
                package.id.clone(),
                package_source_id(&package.id)?,
                package.local_name.clone(),
                package.edition,
                modules,
                package
                    .dependencies
                    .iter()
                    .map(|(alias, package)| (alias.clone(), package.clone())),
            )?);
        }
        nodes.push(PackageNode::new(
            self.standard.clone(),
            package_source_id(&self.standard)?,
            PackageAlias::new("tondoStd")?,
            Edition::V0_1,
            [
                ModulePath::new("bytes")?,
                ModulePath::new("console")?,
                ModulePath::new("env")?,
                ModulePath::new("process")?,
                ModulePath::new("time")?,
            ],
            [],
        )?);
        let packages = PackageGraph::new(self.root.package.clone(), self.standard.clone(), nodes)?;

        let generator_inputs = self
            .generator_names
            .iter()
            .map(|(name, path)| (name.clone(), self.required_inputs[path].sha256.clone()))
            .chain(self.privileged_ids.iter().map(|(id, path)| {
                (
                    format!("privileged:{id}"),
                    self.required_inputs[path].sha256.clone(),
                )
            }))
            .collect::<BTreeMap<_, _>>();
        let build_inputs = DeclaredBuildInputs::new(
            self.target.features.clone(),
            self.selected_source_sets.clone(),
        )
        .with_manifest_hash(self.manifest_hash.clone())?
        .with_lockfile_hash(self.lockfile_hash.clone())?
        .with_generator_inputs(generator_inputs)?
        .with_dependency_interfaces(interfaces, true);

        Ok(ResolvedProject {
            edition: self.packages[&self.root.package].edition,
            target: BuildTarget::vm_hosted(),
            profile: self.target.profile,
            capabilities: self.target.capabilities.clone(),
            source_form: self.root.form,
            packages,
            sources,
            root,
            build_inputs,
            privileged_units,
        })
    }

    fn from_wire(
        manifest_bytes: &[u8],
        lockfile_bytes: &[u8],
        manifest: ManifestWire,
        lockfile: LockfileWire,
    ) -> Result<Self, ProjectError> {
        if manifest.format != MANIFEST_FORMAT {
            return Err(ProjectError::UnsupportedManifestFormat(manifest.format));
        }
        if lockfile.format != LOCKFILE_FORMAT {
            return Err(ProjectError::UnsupportedLockfileFormat(lockfile.format));
        }
        let manifest_hash = sha256(manifest_bytes);
        if lockfile.manifest_hash != manifest_hash {
            return Err(ProjectError::ManifestHashMismatch {
                expected: lockfile.manifest_hash,
                actual: manifest_hash,
            });
        }
        let lockfile_hash = sha256(lockfile_bytes);
        let target = PlannedTarget::from_wire(manifest.target)?;
        let standard = PackageId::new(manifest.standard)?;
        if standard.as_str() != BOOTSTRAP_STANDARD_PACKAGE {
            return Err(ProjectError::UnsupportedStandardPackage(
                standard.to_string(),
            ));
        }
        if lockfile.standard.package_id != standard.as_str() {
            return Err(ProjectError::LockGraphMismatch(format!(
                "standard PackageId is `{}`, expected `{standard}`",
                lockfile.standard.package_id
            )));
        }
        if lockfile.standard.content_hash != bootstrap_standard_hash() {
            return Err(ProjectError::LockGraphMismatch(format!(
                "standard package `{standard}` has an unexpected content hash"
            )));
        }

        let root_package = PackageId::new(manifest.root.package)?;
        let root_physical_path = canonical_tondo_source_path(&manifest.root.source)?;
        let root_form = parse_source_form(&manifest.root.form)?;
        let mut packages = BTreeMap::new();
        let mut manifest_sources = BTreeMap::<(PackageId, String), ManifestSourceRecord>::new();
        let mut selected_sources = Vec::new();
        let mut selected_source_sets = BTreeSet::new();
        let mut physical_paths = BTreeSet::new();

        for package in manifest.packages {
            let id = PackageId::new(package.id)?;
            if id == standard {
                return Err(ProjectError::InvalidManifest(
                    "the compiler-owned standard package must not appear in `packages`".into(),
                ));
            }
            if package.source_sets.is_empty() {
                return Err(ProjectError::InvalidManifest(format!(
                    "package `{id}` must declare at least one source set"
                )));
            }
            let local_name = PackageAlias::new(package.local_name)?;
            let edition = parse_edition(&package.edition)?;
            let dependencies = validated_dependencies(package.dependencies)?;
            let mut source_set_names = BTreeSet::new();
            for source_set in package.source_sets {
                let local_id = validate_source_set_name(&source_set.id)?;
                if !source_set_names.insert(local_id.clone()) {
                    return Err(ProjectError::DuplicateSourceSet {
                        package: id.to_string(),
                        source_set: local_id,
                    });
                }
                let selected = source_set.when.matches(&target)?;
                let global_id = SourceSetId::for_package(&id, &local_id)?;
                if selected {
                    selected_source_sets.insert(global_id);
                }
                for source in source_set.sources {
                    let physical_path = canonical_tondo_source_path(&source.physical_path)?;
                    let logical_path = LogicalPath::new(&source.logical_path)?;
                    require_tondo_source_extension(logical_path.as_str())?;
                    let module = ModulePath::new(&source.module)?;
                    if !physical_paths.insert(physical_path.clone()) {
                        return Err(ProjectError::DuplicatePhysicalInput(physical_path));
                    }
                    let key = (id.clone(), physical_path.clone());
                    let record = ManifestSourceRecord {
                        source_set: local_id.clone(),
                        logical_path: logical_path.clone(),
                        module: module.clone(),
                    };
                    if manifest_sources.insert(key, record).is_some() {
                        return Err(ProjectError::DuplicatePhysicalInput(physical_path));
                    }
                    if selected {
                        selected_sources.push(PlannedSource {
                            package: id.clone(),
                            physical_path,
                            logical_path,
                            module,
                        });
                    }
                }
            }
            let planned = PlannedPackage {
                id: id.clone(),
                local_name,
                edition,
                dependencies,
                interface_path: None,
            };
            if packages.insert(id.clone(), planned).is_some() {
                return Err(ProjectError::DuplicatePackage(id.to_string()));
            }
        }
        if !packages.contains_key(&root_package) {
            return Err(ProjectError::UnknownRootPackage(root_package.to_string()));
        }
        if !manifest_sources.contains_key(&(root_package.clone(), root_physical_path.clone())) {
            return Err(ProjectError::UnknownRootSource {
                package: root_package.to_string(),
                path: root_physical_path,
            });
        }

        selected_sources.sort_by(|left, right| {
            (
                left.package.as_str(),
                left.module.as_str(),
                left.logical_path.as_str(),
                left.physical_path.as_str(),
            )
                .cmp(&(
                    right.package.as_str(),
                    right.module.as_str(),
                    right.logical_path.as_str(),
                    right.physical_path.as_str(),
                ))
        });
        validate_selected_source_uniqueness(&selected_sources)?;

        let lock_packages = validated_lock_packages(lockfile.packages)?;
        let manifest_ids = packages.keys().cloned().collect::<BTreeSet<_>>();
        let lock_ids = lock_packages.keys().cloned().collect::<BTreeSet<_>>();
        if manifest_ids != lock_ids {
            return Err(ProjectError::LockGraphMismatch(
                "manifest and lockfile package sets differ".into(),
            ));
        }

        let mut required_inputs = BTreeMap::new();
        for (id, package) in &mut packages {
            let locked = &lock_packages[id];
            let expected_dependencies = package
                .dependencies
                .iter()
                .map(|(alias, package)| (alias.as_str(), package.as_str()))
                .collect::<BTreeMap<_, _>>();
            let actual_dependencies = locked
                .dependencies
                .iter()
                .map(|dependency| (dependency.alias.as_str(), dependency.package.as_str()))
                .collect::<BTreeMap<_, _>>();
            if expected_dependencies != actual_dependencies {
                return Err(ProjectError::LockGraphMismatch(format!(
                    "dependency aliases or PackageIds differ for `{id}`"
                )));
            }

            let locked_sources = locked
                .sources
                .iter()
                .map(|source| (source.physical_path.clone(), source))
                .collect::<BTreeMap<_, _>>();
            let declared_sources = manifest_sources
                .iter()
                .filter(|((package, _), _)| package == id)
                .map(|((_, path), _)| path.clone())
                .collect::<BTreeSet<_>>();
            let locked_source_paths = locked_sources.keys().cloned().collect::<BTreeSet<_>>();
            if declared_sources != locked_source_paths {
                return Err(ProjectError::LockGraphMismatch(format!(
                    "source sets and locked source paths differ for `{id}`"
                )));
            }
            for path in &declared_sources {
                let source = &locked_sources[path];
                validate_sha256(&source.sha256)?;
                let declared = &manifest_sources[&(id.clone(), path.clone())];
                if source.logical_path != declared.logical_path.as_str()
                    || source.module != declared.module.as_str()
                    || source.source_set != declared.source_set
                {
                    return Err(ProjectError::LockGraphMismatch(format!(
                        "locked source metadata differs for `{id}` input `{path}`"
                    )));
                }
            }

            let interface_hash = match (&locked.interface, id == &root_package) {
                (None, true) => None,
                (Some(_), true) => {
                    return Err(ProjectError::LockGraphMismatch(
                        "the root package must not consume its own compiled interface".into(),
                    ));
                }
                (None, false) => {
                    return Err(ProjectError::LockGraphMismatch(format!(
                        "dependency `{id}` has no compiled interface"
                    )));
                }
                (Some(interface), false) => {
                    let path = canonical_input_path(&interface.path)?;
                    validate_sha256(&interface.sha256)?;
                    insert_required_input(
                        &mut required_inputs,
                        path.clone(),
                        ProjectInputKind::DependencyInterface,
                        interface.sha256.clone(),
                    )?;
                    package.interface_path = Some(path);
                    Some(interface.sha256.as_str())
                }
            };
            let calculated =
                package_content_hash(id, &locked.dependencies, &locked.sources, interface_hash)?;
            if locked.content_hash != calculated {
                return Err(ProjectError::PackageContentHashMismatch {
                    package: id.to_string(),
                    expected: locked.content_hash.clone(),
                    actual: calculated,
                });
            }
        }

        for source in &selected_sources {
            let hash = lock_packages[&source.package]
                .sources
                .iter()
                .find(|locked| locked.physical_path == source.physical_path)
                .expect("manifest and lock source sets were matched above")
                .sha256
                .clone();
            insert_required_input(
                &mut required_inputs,
                source.physical_path.clone(),
                ProjectInputKind::Source,
                hash,
            )?;
        }

        for input in &manifest.generator_inputs {
            validate_generator_input_name(&input.name)?;
        }
        let (generator_names, locked_generators) = validate_named_inputs(
            manifest.generator_inputs,
            lockfile.generator_inputs,
            "generator",
        )?;
        for (name, path) in &generator_names {
            let hash = locked_generators[name].clone();
            insert_required_input(
                &mut required_inputs,
                path.clone(),
                ProjectInputKind::GeneratorInput,
                hash,
            )?;
        }
        let (privileged_ids, locked_units) = validate_named_inputs(
            manifest.privileged_units,
            lockfile.privileged_units,
            "privileged unit",
        )?;
        for (id, path) in &privileged_ids {
            validate_unit_id(id)?;
            let hash = locked_units[id].clone();
            insert_required_input(
                &mut required_inputs,
                path.clone(),
                ProjectInputKind::PrivilegedUnit,
                hash,
            )?;
        }

        let root = PlannedRoot {
            package: root_package,
            physical_path: canonical_tondo_source_path(&manifest.root.source)?,
            form: root_form,
        };
        Ok(Self {
            manifest_hash: sha256(manifest_bytes),
            lockfile_hash,
            target,
            root,
            standard,
            packages,
            selected_sources,
            selected_source_sets,
            required_inputs,
            generator_names,
            privileged_ids,
        })
    }
}

#[derive(Debug)]
pub struct ResolvedProject {
    edition: Edition,
    target: BuildTarget,
    profile: HostProfile,
    capabilities: BTreeSet<CapabilityName>,
    source_form: SourceForm,
    packages: PackageGraph,
    sources: SourceDatabase,
    root: FileId,
    build_inputs: DeclaredBuildInputs,
    privileged_units: BTreeMap<String, PrivilegedUnit>,
}

impl ResolvedProject {
    pub fn privileged_units(&self) -> &BTreeMap<String, PrivilegedUnit> {
        &self.privileged_units
    }

    pub fn into_compilation_request(
        self,
        operation: Operation,
        diagnostic_format: DiagnosticFormat,
        limits: ResourceLimits,
    ) -> Result<CompilationRequest, ProjectError> {
        Ok(CompilationRequest::new(
            operation,
            self.edition,
            self.target,
            self.profile,
            self.capabilities,
            diagnostic_format,
            self.source_form,
            limits,
            self.packages,
            self.sources,
            self.root,
        )?
        .with_declared_build_inputs(self.build_inputs))
    }
}

#[derive(Debug)]
pub enum ProjectError {
    InvalidManifest(String),
    InvalidLockfile(String),
    InvalidPrivilegedUnit(String),
    UnsupportedManifestFormat(String),
    UnsupportedLockfileFormat(String),
    UnsupportedTarget(String),
    UnsupportedProfile(String),
    UnsupportedEdition(String),
    UnsupportedStandardPackage(String),
    InvalidIdentity {
        field: &'static str,
        value: String,
    },
    InvalidSourceSetName(String),
    InvalidSourceOrder(String),
    InvalidPrivilegedUnitId(String),
    DuplicatePackage(String),
    DuplicateSourceSet {
        package: String,
        source_set: String,
    },
    DuplicatePhysicalInput(String),
    DuplicateLogicalSource {
        package: String,
        logical_path: String,
    },
    DuplicateNamedInput {
        kind: &'static str,
        name: String,
    },
    ConflictingSourceSetCondition(String),
    UnknownRootPackage(String),
    UnknownRootSource {
        package: String,
        path: String,
    },
    InactiveRootSource {
        package: String,
        path: String,
    },
    ManifestHashMismatch {
        expected: String,
        actual: String,
    },
    LockGraphMismatch(String),
    PackageContentHashMismatch {
        package: String,
        expected: String,
        actual: String,
    },
    MissingInput(String),
    UndeclaredInput(String),
    InputHashMismatch {
        path: String,
        expected: String,
        actual: String,
    },
    InterfacePackageMismatch {
        expected: String,
        actual: String,
    },
    PrivilegedUnitIdMismatch {
        expected: String,
        actual: String,
    },
    NonCanonicalPrivilegedUnit(String),
    NonCanonicalList(&'static str),
    Artifact(crate::artifact::ArtifactError),
    Driver(DriverError),
    Package(PackageGraphError),
    Source(SourceError),
    Serialization(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(message) => write!(formatter, "invalid manifest: {message}"),
            Self::InvalidLockfile(message) => write!(formatter, "invalid lockfile: {message}"),
            Self::InvalidPrivilegedUnit(message) => {
                write!(formatter, "invalid privileged unit: {message}")
            }
            Self::UnsupportedManifestFormat(format) => {
                write!(formatter, "unsupported manifest format `{format}`")
            }
            Self::UnsupportedLockfileFormat(format) => {
                write!(formatter, "unsupported lockfile format `{format}`")
            }
            Self::UnsupportedTarget(target) => write!(formatter, "unsupported target `{target}`"),
            Self::UnsupportedProfile(profile) => {
                write!(formatter, "unsupported host profile `{profile}`")
            }
            Self::UnsupportedEdition(edition) => {
                write!(formatter, "unsupported language edition `{edition}`")
            }
            Self::UnsupportedStandardPackage(package) => {
                write!(formatter, "unsupported standard package `{package}`")
            }
            Self::InvalidIdentity { field, value } => {
                write!(formatter, "invalid {field} identity `{value}`")
            }
            Self::InvalidSourceSetName(name) => {
                write!(formatter, "invalid source-set name `{name}`")
            }
            Self::InvalidSourceOrder(message) => {
                write!(formatter, "invalid source order: {message}")
            }
            Self::InvalidPrivilegedUnitId(id) => {
                write!(formatter, "invalid privileged-unit ID `{id}`")
            }
            Self::DuplicatePackage(package) => write!(formatter, "duplicate package `{package}`"),
            Self::DuplicateSourceSet {
                package,
                source_set,
            } => write!(
                formatter,
                "duplicate source set `{source_set}` in package `{package}`"
            ),
            Self::DuplicatePhysicalInput(path) => {
                write!(
                    formatter,
                    "physical input `{path}` is declared more than once"
                )
            }
            Self::DuplicateLogicalSource {
                package,
                logical_path,
            } => write!(
                formatter,
                "active package `{package}` maps more than one source to `{logical_path}`"
            ),
            Self::DuplicateNamedInput { kind, name } => {
                write!(formatter, "duplicate {kind} `{name}`")
            }
            Self::ConflictingSourceSetCondition(message) => {
                write!(formatter, "conflicting source-set condition: {message}")
            }
            Self::UnknownRootPackage(package) => {
                write!(formatter, "unknown root package `{package}`")
            }
            Self::UnknownRootSource { package, path } => {
                write!(formatter, "package `{package}` has no root source `{path}`")
            }
            Self::InactiveRootSource { package, path } => write!(
                formatter,
                "root source `{path}` of package `{package}` is inactive for this target"
            ),
            Self::ManifestHashMismatch { expected, actual } => write!(
                formatter,
                "lockfile pins manifest hash `{expected}`, found `{actual}`"
            ),
            Self::LockGraphMismatch(message) => {
                write!(formatter, "manifest and lockfile graph differ: {message}")
            }
            Self::PackageContentHashMismatch {
                package,
                expected,
                actual,
            } => write!(
                formatter,
                "package `{package}` content hash is `{actual}`, lockfile records `{expected}`"
            ),
            Self::MissingInput(path) => write!(formatter, "required input `{path}` is missing"),
            Self::UndeclaredInput(path) => {
                write!(
                    formatter,
                    "input `{path}` was not declared by the build plan"
                )
            }
            Self::InputHashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "input `{path}` has hash `{actual}`, expected `{expected}`"
            ),
            Self::InterfacePackageMismatch { expected, actual } => write!(
                formatter,
                "compiled interface identifies package `{actual}`, expected `{expected}`"
            ),
            Self::PrivilegedUnitIdMismatch { expected, actual } => write!(
                formatter,
                "privileged unit identifies `{actual}`, expected `{expected}`"
            ),
            Self::NonCanonicalPrivilegedUnit(id) => {
                write!(
                    formatter,
                    "privileged unit `{id}` is not canonically encoded"
                )
            }
            Self::NonCanonicalList(name) => {
                write!(formatter, "{name} are not sorted and unique")
            }
            Self::Artifact(error) => error.fmt(formatter),
            Self::Driver(error) => error.fmt(formatter),
            Self::Package(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Serialization(message) => formatter.write_str(message),
        }
    }
}

impl Error for ProjectError {}

impl From<crate::artifact::ArtifactError> for ProjectError {
    fn from(error: crate::artifact::ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

impl From<DriverError> for ProjectError {
    fn from(error: DriverError) -> Self {
        Self::Driver(error)
    }
}

impl From<PackageGraphError> for ProjectError {
    fn from(error: PackageGraphError) -> Self {
        Self::Package(error)
    }
}

impl From<SourceError> for ProjectError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

#[derive(Debug)]
struct PlannedTarget {
    name: String,
    profile: HostProfile,
    capabilities: BTreeSet<CapabilityName>,
    features: BTreeSet<FeatureName>,
}

impl PlannedTarget {
    fn from_wire(target: TargetWire) -> Result<Self, ProjectError> {
        if target.name != BuildTarget::vm_hosted().name() {
            return Err(ProjectError::UnsupportedTarget(target.name));
        }
        let profile = parse_profile(&target.profile)?;
        if target.capability_registry != CAPABILITY_REGISTRY {
            return Err(ProjectError::InvalidManifest(format!(
                "unsupported capability registry `{}`",
                target.capability_registry
            )));
        }
        let capabilities = target
            .capabilities
            .into_iter()
            .map(CapabilityName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let supported = BuildTarget::vm_hosted().supported_capabilities().clone();
        if let Some(capability) = capabilities
            .iter()
            .find(|capability| !supported.contains(*capability))
        {
            return Err(ProjectError::InvalidManifest(format!(
                "target `{}` does not support capability `{capability}`",
                target.name
            )));
        }
        let features = target
            .features
            .into_iter()
            .map(FeatureName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self {
            name: target.name,
            profile,
            capabilities,
            features,
        })
    }
}

#[derive(Debug)]
struct PlannedRoot {
    package: PackageId,
    physical_path: String,
    form: SourceForm,
}

#[derive(Debug)]
struct PlannedPackage {
    id: PackageId,
    local_name: PackageAlias,
    edition: Edition,
    dependencies: BTreeMap<PackageAlias, PackageId>,
    interface_path: Option<String>,
}

#[derive(Debug)]
struct PlannedSource {
    package: PackageId,
    physical_path: String,
    logical_path: LogicalPath,
    module: ModulePath,
}

#[derive(Debug)]
struct PlannedInput {
    kind: ProjectInputKind,
    sha256: String,
}

#[derive(Debug)]
struct ManifestSourceRecord {
    source_set: String,
    logical_path: LogicalPath,
    module: ModulePath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestWire {
    format: String,
    target: TargetWire,
    root: RootWire,
    standard: String,
    packages: Vec<ManifestPackageWire>,
    #[serde(default)]
    generator_inputs: Vec<NamedPathWire>,
    #[serde(default)]
    privileged_units: Vec<NamedPathWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetWire {
    name: String,
    profile: String,
    capability_registry: String,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    features: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootWire {
    package: String,
    source: String,
    form: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestPackageWire {
    id: String,
    local_name: String,
    edition: String,
    #[serde(default)]
    dependencies: Vec<DependencyWire>,
    source_sets: Vec<SourceSetWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependencyWire {
    pub(crate) alias: String,
    pub(crate) package: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSetWire {
    id: String,
    #[serde(default)]
    when: SourceSetConditionWire,
    sources: Vec<ManifestSourceWire>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSetConditionWire {
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    profiles: Vec<String>,
    #[serde(default)]
    requires_capabilities: Vec<String>,
    #[serde(default)]
    excludes_capabilities: Vec<String>,
    #[serde(default)]
    requires_features: Vec<String>,
    #[serde(default)]
    excludes_features: Vec<String>,
}

impl SourceSetConditionWire {
    fn matches(self, target: &PlannedTarget) -> Result<bool, ProjectError> {
        for candidate in &self.targets {
            if candidate != BuildTarget::vm_hosted().name() {
                return Err(ProjectError::UnsupportedTarget(candidate.clone()));
            }
        }
        for candidate in &self.profiles {
            parse_profile(candidate)?;
        }
        let requires_capabilities = self
            .requires_capabilities
            .into_iter()
            .map(CapabilityName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let excludes_capabilities = self
            .excludes_capabilities
            .into_iter()
            .map(CapabilityName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(conflict) = requires_capabilities
            .intersection(&excludes_capabilities)
            .next()
        {
            return Err(ProjectError::ConflictingSourceSetCondition(format!(
                "capability `{conflict}` is both required and excluded"
            )));
        }
        let requires_features = self
            .requires_features
            .into_iter()
            .map(FeatureName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        let excludes_features = self
            .excludes_features
            .into_iter()
            .map(FeatureName::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if let Some(conflict) = requires_features.intersection(&excludes_features).next() {
            return Err(ProjectError::ConflictingSourceSetCondition(format!(
                "feature `{conflict}` is both required and excluded"
            )));
        }
        Ok(
            (self.targets.is_empty() || self.targets.iter().any(|name| name == &target.name))
                && (self.profiles.is_empty()
                    || self
                        .profiles
                        .iter()
                        .any(|profile| profile == target.profile.as_str()))
                && requires_capabilities.is_subset(&target.capabilities)
                && excludes_capabilities.is_disjoint(&target.capabilities)
                && requires_features.is_subset(&target.features)
                && excludes_features.is_disjoint(&target.features),
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestSourceWire {
    physical_path: String,
    logical_path: String,
    module: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedPathWire {
    name: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockfileWire {
    format: String,
    manifest_hash: String,
    standard: LockedStandardWire,
    packages: Vec<LockedPackageWire>,
    #[serde(default)]
    generator_inputs: Vec<LockedNamedInputWire>,
    #[serde(default)]
    privileged_units: Vec<LockedNamedInputWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedStandardWire {
    package_id: String,
    content_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedPackageWire {
    id: String,
    content_hash: String,
    #[serde(default)]
    dependencies: Vec<DependencyWire>,
    sources: Vec<LockedSourceWire>,
    interface: Option<LockedFileWire>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockedSourceWire {
    pub(crate) source_set: String,
    pub(crate) physical_path: String,
    pub(crate) logical_path: String,
    pub(crate) module: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedFileWire {
    path: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockedNamedInputWire {
    name: String,
    sha256: String,
}

type NamedInputMaps = (BTreeMap<String, String>, BTreeMap<String, String>);

fn parse_edition(value: &str) -> Result<Edition, ProjectError> {
    match value {
        "0.1" => Ok(Edition::V0_1),
        _ => Err(ProjectError::UnsupportedEdition(value.into())),
    }
}

fn parse_profile(value: &str) -> Result<HostProfile, ProjectError> {
    match value {
        "hosted" => Ok(HostProfile::Hosted),
        _ => Err(ProjectError::UnsupportedProfile(value.into())),
    }
}

fn parse_source_form(value: &str) -> Result<SourceForm, ProjectError> {
    match value {
        "module" => Ok(SourceForm::Module),
        "script" => Ok(SourceForm::Script),
        "fragment" => Ok(SourceForm::Fragment),
        _ => Err(ProjectError::InvalidManifest(format!(
            "unknown root form `{value}`"
        ))),
    }
}

fn canonical_input_path(value: &str) -> Result<String, ProjectError> {
    Ok(LogicalPath::new(value)?.to_string())
}

fn canonical_tondo_source_path(value: &str) -> Result<String, ProjectError> {
    let path = canonical_input_path(value)?;
    require_tondo_source_extension(&path)?;
    Ok(path)
}

fn require_tondo_source_extension(path: &str) -> Result<(), ProjectError> {
    if path.ends_with(".to") {
        Ok(())
    } else {
        Err(ProjectError::InvalidManifest(format!(
            "Tondo source path `{path}` must use the `.to` extension"
        )))
    }
}

fn package_source_id(package: &PackageId) -> Result<SourceId, SourceError> {
    SourceId::new(format!(
        "pkg:{}:{}",
        package.as_str().len(),
        package.as_str()
    ))
}

fn validate_identity_field(field: &'static str, value: &str) -> Result<(), ProjectError> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        Err(ProjectError::InvalidIdentity {
            field,
            value: value.into(),
        })
    } else {
        Ok(())
    }
}

fn validate_source_set_name(value: &str) -> Result<String, ProjectError> {
    if is_kebab_identifier(value) {
        Ok(value.into())
    } else {
        Err(ProjectError::InvalidSourceSetName(value.into()))
    }
}

fn validate_unit_id(value: &str) -> Result<(), ProjectError> {
    if !value.is_empty()
        && value.split('.').all(is_kebab_identifier)
        && !value.contains(['\n', '\r'])
    {
        Ok(())
    } else {
        Err(ProjectError::InvalidPrivilegedUnitId(value.into()))
    }
}

fn validate_generator_input_name(value: &str) -> Result<(), ProjectError> {
    validate_identity_field("generator input name", value)?;
    if value.starts_with("privileged:") {
        return Err(ProjectError::InvalidManifest(format!(
            "generator input name `{value}` uses the reserved `privileged:` prefix"
        )));
    }
    Ok(())
}

fn is_kebab_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validated_dependencies(
    dependencies: Vec<DependencyWire>,
) -> Result<BTreeMap<PackageAlias, PackageId>, ProjectError> {
    let mut result = BTreeMap::new();
    for dependency in dependencies {
        let alias = PackageAlias::new(dependency.alias)?;
        let package = PackageId::new(dependency.package)?;
        if result.insert(alias.clone(), package).is_some() {
            return Err(ProjectError::LockGraphMismatch(format!(
                "dependency alias `{alias}` is duplicated"
            )));
        }
    }
    Ok(result)
}

fn validate_selected_source_uniqueness(sources: &[PlannedSource]) -> Result<(), ProjectError> {
    let mut logical = BTreeSet::new();
    for source in sources {
        if !logical.insert((source.package.clone(), source.logical_path.clone())) {
            return Err(ProjectError::DuplicateLogicalSource {
                package: source.package.to_string(),
                logical_path: source.logical_path.to_string(),
            });
        }
    }
    Ok(())
}

fn validated_lock_packages(
    packages: Vec<LockedPackageWire>,
) -> Result<BTreeMap<PackageId, LockedPackageWire>, ProjectError> {
    let mut result = BTreeMap::new();
    for mut package in packages {
        let id = PackageId::new(package.id.clone())?;
        validate_sha256(&package.content_hash)?;
        package.dependencies.sort_by(|left, right| {
            (&left.alias, &left.package).cmp(&(&right.alias, &right.package))
        });
        if package
            .dependencies
            .windows(2)
            .any(|pair| pair[0].alias == pair[1].alias)
        {
            return Err(ProjectError::LockGraphMismatch(format!(
                "package `{id}` repeats a dependency alias"
            )));
        }
        for source in &mut package.sources {
            source.physical_path = canonical_tondo_source_path(&source.physical_path)?;
            source.logical_path = LogicalPath::new(&source.logical_path)?.to_string();
            require_tondo_source_extension(&source.logical_path)?;
            source.module = ModulePath::new(&source.module)?.to_string();
            validate_source_set_name(&source.source_set)?;
            validate_sha256(&source.sha256)?;
        }
        package
            .sources
            .sort_by(|left, right| left.physical_path.cmp(&right.physical_path));
        if package
            .sources
            .windows(2)
            .any(|pair| pair[0].physical_path == pair[1].physical_path)
        {
            return Err(ProjectError::LockGraphMismatch(format!(
                "package `{id}` repeats a locked source path"
            )));
        }
        if result.insert(id.clone(), package).is_some() {
            return Err(ProjectError::DuplicatePackage(id.to_string()));
        }
    }
    Ok(result)
}

fn validate_named_inputs(
    manifest: Vec<NamedPathWire>,
    locked: Vec<LockedNamedInputWire>,
    kind: &'static str,
) -> Result<NamedInputMaps, ProjectError> {
    let mut manifest_inputs = BTreeMap::new();
    for input in manifest {
        validate_identity_field("declared input name", &input.name)?;
        let path = canonical_input_path(&input.path)?;
        if manifest_inputs.insert(input.name.clone(), path).is_some() {
            return Err(ProjectError::DuplicateNamedInput {
                kind,
                name: input.name,
            });
        }
    }
    let mut locked_inputs = BTreeMap::new();
    for input in locked {
        validate_sha256(&input.sha256)?;
        if locked_inputs
            .insert(input.name.clone(), input.sha256)
            .is_some()
        {
            return Err(ProjectError::DuplicateNamedInput {
                kind,
                name: input.name,
            });
        }
    }
    if manifest_inputs.keys().ne(locked_inputs.keys()) {
        return Err(ProjectError::LockGraphMismatch(format!(
            "manifest and lockfile {kind} sets differ"
        )));
    }
    Ok((manifest_inputs, locked_inputs))
}

fn insert_required_input(
    inputs: &mut BTreeMap<String, PlannedInput>,
    path: String,
    kind: ProjectInputKind,
    sha256: String,
) -> Result<(), ProjectError> {
    if inputs
        .insert(path.clone(), PlannedInput { kind, sha256 })
        .is_some()
    {
        return Err(ProjectError::DuplicatePhysicalInput(path));
    }
    Ok(())
}

pub(crate) fn package_content_hash(
    package: &PackageId,
    dependencies: &[DependencyWire],
    sources: &[LockedSourceWire],
    interface_hash: Option<&str>,
) -> Result<String, ProjectError> {
    #[derive(Serialize)]
    struct Fingerprint<'a> {
        package_id: &'a str,
        dependencies: &'a [DependencyWire],
        sources: &'a [LockedSourceWire],
        interface_hash: Option<&'a str>,
    }

    let encoded = serde_json::to_vec(&Fingerprint {
        package_id: package.as_str(),
        dependencies,
        sources,
        interface_hash,
    })
    .map_err(|error| ProjectError::Serialization(error.to_string()))?;
    Ok(sha256(&encoded))
}

fn require_sorted_unique(name: &'static str, values: &[String]) -> Result<(), ProjectError> {
    if values.windows(2).all(|pair| pair[0] < pair[1]) {
        Ok(())
    } else {
        Err(ProjectError::NonCanonicalList(name))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::driver::{CompilationStatus, execute};

    type ProjectFixture = (Vec<u8>, Vec<u8>, BTreeMap<String, Arc<[u8]>>);

    fn root_project(source: &[u8], inactive_source: &[u8]) -> ProjectFixture {
        let package = PackageId::new("workspace:app@1").unwrap();
        let manifest = serde_json::to_vec(&json!({
            "format": MANIFEST_FORMAT,
            "target": {
                "name": "tondo-vm-hosted",
                "profile": "hosted",
                "capability_registry": CAPABILITY_REGISTRY,
                "capabilities": ["console", "process"],
                "features": ["fast"]
            },
            "root": {
                "package": package.as_str(),
                "source": "app/src/main.to",
                "form": "module"
            },
            "standard": BOOTSTRAP_STANDARD_PACKAGE,
            "packages": [{
                "id": package.as_str(),
                "local_name": "app",
                "edition": "0.1",
                "dependencies": [],
                "source_sets": [
                    {
                        "id": "common",
                        "sources": [{
                            "physical_path": "app/src/main.to",
                            "logical_path": "src/main.to",
                            "module": "main"
                        }]
                    },
                    {
                        "id": "slow",
                        "when": {"requires_features": ["slow"]},
                        "sources": [{
                            "physical_path": "app/src/slow.to",
                            "logical_path": "src/slow.to",
                            "module": "main"
                        }]
                    }
                ]
            }],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let sources = vec![
            LockedSourceWire {
                source_set: "common".into(),
                physical_path: "app/src/main.to".into(),
                logical_path: "src/main.to".into(),
                module: "main".into(),
                sha256: sha256(source),
            },
            LockedSourceWire {
                source_set: "slow".into(),
                physical_path: "app/src/slow.to".into(),
                logical_path: "src/slow.to".into(),
                module: "main".into(),
                sha256: sha256(inactive_source),
            },
        ];
        let content_hash = package_content_hash(&package, &[], &sources, None).unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format": LOCKFILE_FORMAT,
            "manifest_hash": sha256(&manifest),
            "standard": {
                "package_id": BOOTSTRAP_STANDARD_PACKAGE,
                "content_hash": bootstrap_standard_hash()
            },
            "packages": [{
                "id": package.as_str(),
                "content_hash": content_hash,
                "dependencies": [],
                "sources": sources,
                "interface": null
            }],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let supplied = BTreeMap::from([("app/src/main.to".into(), Arc::<[u8]>::from(source))]);
        (manifest, lockfile, supplied)
    }

    #[test]
    fn root_forms_and_privileged_unit_ids_use_closed_parsers() {
        assert_eq!(parse_edition("0.1").unwrap(), Edition::V0_1);
        assert_eq!(parse_profile("hosted").unwrap(), HostProfile::Hosted);
        assert!(parse_edition("0.2").is_err());
        assert!(parse_profile("embedded").is_err());

        assert_eq!(parse_source_form("module").unwrap(), SourceForm::Module);
        assert_eq!(parse_source_form("script").unwrap(), SourceForm::Script);
        assert_eq!(parse_source_form("fragment").unwrap(), SourceForm::Fragment);
        assert!(parse_source_form("unknown").is_err());

        require_tondo_source_extension("src/main.to").unwrap();
        assert!(require_tondo_source_extension("src/main.txt").is_err());
        validate_identity_field("name", "stable").unwrap();
        assert!(validate_identity_field("name", "bad\nname").is_err());
        assert_eq!(validate_source_set_name("fast").unwrap(), "fast");
        assert!(validate_source_set_name("Fast").is_err());

        validate_unit_id("services.console").unwrap();
        assert!(validate_unit_id("").is_err());
        assert!(validate_unit_id("Services.console").is_err());
        assert!(validate_unit_id("services\nconsole").is_err());
    }

    #[test]
    fn inactive_source_sets_are_resolved_before_lexing() {
        let (manifest, lockfile, supplied) = root_project(b"fn main() {}\n", b"\xff{{{{");
        let plan = ProjectPlan::parse(&manifest, &lockfile).unwrap();
        assert_eq!(
            plan.required_inputs()
                .map(|input| (input.path, input.kind))
                .collect::<Vec<_>>(),
            [("app/src/main.to".into(), ProjectInputKind::Source)]
        );
        assert_eq!(
            plan.selected_source_sets()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["@15:workspace:app@1#common"]
        );
        let request = plan
            .resolve(&supplied)
            .unwrap()
            .into_compilation_request(
                Operation::Check,
                DiagnosticFormat::Json,
                ResourceLimits::default(),
            )
            .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        assert_eq!(
            output.interface().unwrap().source_sets(),
            ["@15:workspace:app@1#common"]
        );
        assert_eq!(output.artifact().unwrap().features(), ["fast"]);
        assert!(output.artifact().unwrap().reproducible());
    }

    #[test]
    fn closed_project_graph_exposes_all_capability_selected_bootstrap_modules() {
        let source = b"import std.env\nimport std.time\nfn main() {}\n";
        let (manifest, lockfile, supplied) = root_project(source, b"unused");
        let mut manifest: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        manifest["target"]["capabilities"] = json!(["clock", "console", "environment", "process"]);
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let mut lockfile: serde_json::Value = serde_json::from_slice(&lockfile).unwrap();
        lockfile["manifest_hash"] = json!(sha256(&manifest));
        let lockfile = serde_json::to_vec(&lockfile).unwrap();

        let request = ProjectPlan::parse(&manifest, &lockfile)
            .unwrap()
            .resolve(&supplied)
            .unwrap()
            .into_compilation_request(
                Operation::Check,
                DiagnosticFormat::Json,
                ResourceLimits::default(),
            )
            .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
    }

    #[test]
    fn project_plans_accept_only_declared_exact_bytes() {
        let (manifest, lockfile, supplied) = root_project(b"fn main() {}\n", b"broken");
        let plan = ProjectPlan::parse(&manifest, &lockfile).unwrap();

        let mut missing = supplied.clone();
        missing.clear();
        assert!(matches!(
            plan.resolve(&missing),
            Err(ProjectError::MissingInput(path)) if path == "app/src/main.to"
        ));

        let wrong = BTreeMap::from([(
            "app/src/main.to".into(),
            Arc::<[u8]>::from(&b"fn main() { panic(\"changed\") }\n"[..]),
        )]);
        assert!(matches!(
            plan.resolve(&wrong),
            Err(ProjectError::InputHashMismatch { .. })
        ));

        let mut extra = supplied;
        extra.insert("ambient.txt".into(), Arc::<[u8]>::from(&b"ambient"[..]));
        assert!(matches!(
            plan.resolve(&extra),
            Err(ProjectError::UndeclaredInput(path)) if path == "ambient.txt"
        ));
    }

    #[test]
    fn audit_source_order_must_be_an_exact_project_permutation() {
        let (manifest, lockfile, supplied) = root_project(b"fn main() {}\n", b"ignored");
        let plan = ProjectPlan::parse(&manifest, &lockfile).unwrap();
        assert_eq!(
            plan.selected_source_paths().collect::<Vec<_>>(),
            ["app/src/main.to"]
        );
        assert!(matches!(
            plan.resolve_with_source_order(&supplied, &[]),
            Err(ProjectError::InvalidSourceOrder(message))
                if message.contains("exact permutation")
        ));
        assert!(matches!(
            plan.resolve_with_source_order(&supplied, &["other.to".into()]),
            Err(ProjectError::InvalidSourceOrder(_))
        ));
        plan.resolve_with_source_order(&supplied, &["app/src/main.to".into()])
            .unwrap();
    }

    #[test]
    fn project_error_vocabulary_and_scalar_parsers_are_closed() {
        let hash = sha256(b"value");
        let errors = vec![
            ProjectError::InvalidManifest("manifest".into()),
            ProjectError::InvalidLockfile("lockfile".into()),
            ProjectError::InvalidPrivilegedUnit("unit".into()),
            ProjectError::UnsupportedManifestFormat("manifest/9".into()),
            ProjectError::UnsupportedLockfileFormat("lockfile/9".into()),
            ProjectError::UnsupportedTarget("unknown".into()),
            ProjectError::UnsupportedProfile("unknown".into()),
            ProjectError::UnsupportedEdition("9".into()),
            ProjectError::UnsupportedStandardPackage("pkg:std@9".into()),
            ProjectError::InvalidIdentity {
                field: "source",
                value: "bad".into(),
            },
            ProjectError::InvalidSourceSetName("Bad".into()),
            ProjectError::InvalidSourceOrder("order".into()),
            ProjectError::InvalidPrivilegedUnitId("Bad".into()),
            ProjectError::DuplicatePackage("pkg:item".into()),
            ProjectError::DuplicateSourceSet {
                package: "pkg:item".into(),
                source_set: "common".into(),
            },
            ProjectError::DuplicatePhysicalInput("src/main.to".into()),
            ProjectError::DuplicateLogicalSource {
                package: "pkg:item".into(),
                logical_path: "main.to".into(),
            },
            ProjectError::DuplicateNamedInput {
                kind: "generator",
                name: "schema".into(),
            },
            ProjectError::ConflictingSourceSetCondition("condition".into()),
            ProjectError::UnknownRootPackage("pkg:missing".into()),
            ProjectError::UnknownRootSource {
                package: "pkg:item".into(),
                path: "src/main.to".into(),
            },
            ProjectError::InactiveRootSource {
                package: "pkg:item".into(),
                path: "src/main.to".into(),
            },
            ProjectError::ManifestHashMismatch {
                expected: hash.clone(),
                actual: sha256(b"other"),
            },
            ProjectError::LockGraphMismatch("graph".into()),
            ProjectError::PackageContentHashMismatch {
                package: "pkg:item".into(),
                expected: hash.clone(),
                actual: sha256(b"other"),
            },
            ProjectError::MissingInput("src/main.to".into()),
            ProjectError::UndeclaredInput("ambient.txt".into()),
            ProjectError::InputHashMismatch {
                path: "src/main.to".into(),
                expected: hash.clone(),
                actual: sha256(b"other"),
            },
            ProjectError::InterfacePackageMismatch {
                expected: "pkg:item".into(),
                actual: "pkg:other".into(),
            },
            ProjectError::PrivilegedUnitIdMismatch {
                expected: "vendor.native".into(),
                actual: "vendor.other".into(),
            },
            ProjectError::NonCanonicalPrivilegedUnit("vendor.native".into()),
            ProjectError::NonCanonicalList("features"),
            ProjectError::from(crate::artifact::ArtifactError::InvalidHash("bad".into())),
            ProjectError::from(DriverError::InvalidCapability("telepathy".into())),
            ProjectError::from(PackageGraphError::EmptyPackageId),
            ProjectError::from(SourceId::new("").unwrap_err()),
            ProjectError::Serialization("serialization".into()),
        ];
        assert_eq!(errors.len(), 37);
        for error in errors {
            assert!(!error.to_string().is_empty(), "{error:?}");
        }

        assert_eq!(parse_edition("0.1").unwrap(), Edition::V0_1);
        assert!(parse_edition("0.2").is_err());
        assert_eq!(parse_profile("hosted").unwrap(), HostProfile::Hosted);
        assert!(parse_profile("native").is_err());
        assert_eq!(parse_source_form("module").unwrap(), SourceForm::Module);
        assert_eq!(parse_source_form("script").unwrap(), SourceForm::Script);
        assert_eq!(parse_source_form("fragment").unwrap(), SourceForm::Fragment);
        assert!(parse_source_form("unknown").is_err());
        assert!(validate_identity_field("source", "valid").is_ok());
        assert!(validate_identity_field("source", "").is_err());
        assert!(validate_identity_field("source", "bad\nvalue").is_err());
    }

    #[test]
    fn identical_declared_inputs_produce_identical_interface_and_artifact_bytes() {
        let (manifest, lockfile, supplied) = root_project(b"fn main() {}\n", b"ignored");
        let plan = ProjectPlan::parse(&manifest, &lockfile).unwrap();
        let compile = || {
            let request = plan
                .resolve(&supplied)
                .unwrap()
                .into_compilation_request(
                    Operation::Check,
                    DiagnosticFormat::Json,
                    ResourceLimits::default(),
                )
                .unwrap();
            let output = execute(request).unwrap();
            (
                output.interface().unwrap().encode().unwrap(),
                output.artifact().unwrap().encode().unwrap(),
            )
        };
        let first = compile();
        let second = compile();
        assert_eq!(first, second);
        assert_eq!(
            CompiledInterface::decode(&first.0)
                .unwrap()
                .encode()
                .unwrap(),
            first.0
        );
        assert_eq!(
            crate::artifact::BuildArtifact::decode(&first.1)
                .unwrap()
                .encode()
                .unwrap(),
            first.1
        );
    }

    #[test]
    fn manifest_schema_target_registry_and_active_logical_paths_are_closed() {
        let (manifest, lockfile, _) = root_project(b"fn main() {}\n", b"ignored");
        let mut unknown: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        unknown["ambient"] = json!(true);
        assert!(matches!(
            ProjectPlan::parse(&serde_json::to_vec(&unknown).unwrap(), &lockfile),
            Err(ProjectError::InvalidManifest(_)) | Err(ProjectError::ManifestHashMismatch { .. })
        ));

        let mut capability: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        capability["target"]["capabilities"] = json!(["telepathy"]);
        let capability = serde_json::to_vec(&capability).unwrap();
        let mut lock: serde_json::Value = serde_json::from_slice(&lockfile).unwrap();
        lock["manifest_hash"] = json!(sha256(&capability));
        assert!(matches!(
            ProjectPlan::parse(&capability, &serde_json::to_vec(&lock).unwrap()),
            Err(ProjectError::Driver(DriverError::InvalidCapability(name))) if name == "telepathy"
        ));

        let mut duplicate: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        duplicate["packages"][0]["source_sets"][0]["sources"] = json!([
            {
                "physical_path": "app/src/main.to",
                "logical_path": "src/main.to",
                "module": "main"
            },
            {
                "physical_path": "app/src/other.to",
                "logical_path": "src/main.to",
                "module": "main"
            }
        ]);
        let duplicate = serde_json::to_vec(&duplicate).unwrap();
        let mut lock: serde_json::Value = serde_json::from_slice(&lockfile).unwrap();
        lock["manifest_hash"] = json!(sha256(&duplicate));
        assert!(matches!(
            ProjectPlan::parse(&duplicate, &serde_json::to_vec(&lock).unwrap()),
            Err(ProjectError::DuplicateLogicalSource { .. })
                | Err(ProjectError::LockGraphMismatch(_))
        ));

        let mut empty_sets: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        empty_sets["packages"][0]["source_sets"] = json!([]);
        let empty_sets = serde_json::to_vec(&empty_sets).unwrap();
        let mut lock: serde_json::Value = serde_json::from_slice(&lockfile).unwrap();
        lock["manifest_hash"] = json!(sha256(&empty_sets));
        assert!(matches!(
            ProjectPlan::parse(&empty_sets, &serde_json::to_vec(&lock).unwrap()),
            Err(ProjectError::InvalidManifest(message))
                if message.contains("at least one source set")
        ));
    }

    #[test]
    fn dependency_interfaces_are_compatible_before_sources_are_lexed() {
        let app = PackageId::new("workspace:app@1").unwrap();
        let dependency = PackageId::new("registry:util@1#sha256-demo").unwrap();
        let source_sets = vec!["@27:registry:util@1#sha256-demo#common".to_owned()];
        let interface = CompiledInterface::new(
            "0.1".into(),
            dependency.to_string(),
            "wrong-target".into(),
            "hosted".into(),
            vec!["console".into(), "process".into()],
            Vec::new(),
            source_sets,
            vec!["util".into()],
            sha256(b""),
            Vec::new(),
        )
        .unwrap();
        let interface_bytes = interface.encode().unwrap();
        let app_source = b"import util.util\nfn main() {}\n";
        let dependency_source = b"\xff this must never reach the lexer";
        let manifest = serde_json::to_vec(&json!({
            "format": MANIFEST_FORMAT,
            "target": {
                "name": "tondo-vm-hosted",
                "profile": "hosted",
                "capability_registry": CAPABILITY_REGISTRY,
                "capabilities": ["console", "process"],
                "features": []
            },
            "root": {
                "package": app.as_str(),
                "source": "app/main.to",
                "form": "module"
            },
            "standard": BOOTSTRAP_STANDARD_PACKAGE,
            "packages": [
                {
                    "id": app.as_str(),
                    "local_name": "app",
                    "edition": "0.1",
                    "dependencies": [{
                        "alias": "util",
                        "package": dependency.as_str()
                    }],
                    "source_sets": [{
                        "id": "common",
                        "sources": [{
                            "physical_path": "app/main.to",
                            "logical_path": "main.to",
                            "module": "main"
                        }]
                    }]
                },
                {
                    "id": dependency.as_str(),
                    "local_name": "utilPackage",
                    "edition": "0.1",
                    "dependencies": [],
                    "source_sets": [{
                        "id": "common",
                        "sources": [{
                            "physical_path": "util/util.to",
                            "logical_path": "util.to",
                            "module": "util"
                        }]
                    }]
                }
            ],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let app_dependencies = vec![DependencyWire {
            alias: "util".into(),
            package: dependency.to_string(),
        }];
        let app_sources = vec![LockedSourceWire {
            source_set: "common".into(),
            physical_path: "app/main.to".into(),
            logical_path: "main.to".into(),
            module: "main".into(),
            sha256: sha256(app_source),
        }];
        let dependency_sources = vec![LockedSourceWire {
            source_set: "common".into(),
            physical_path: "util/util.to".into(),
            logical_path: "util.to".into(),
            module: "util".into(),
            sha256: sha256(dependency_source),
        }];
        let app_hash = package_content_hash(&app, &app_dependencies, &app_sources, None).unwrap();
        let interface_hash = sha256(&interface_bytes);
        let dependency_hash =
            package_content_hash(&dependency, &[], &dependency_sources, Some(&interface_hash))
                .unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format": LOCKFILE_FORMAT,
            "manifest_hash": sha256(&manifest),
            "standard": {
                "package_id": BOOTSTRAP_STANDARD_PACKAGE,
                "content_hash": bootstrap_standard_hash()
            },
            "packages": [
                {
                    "id": app.as_str(),
                    "content_hash": app_hash,
                    "dependencies": app_dependencies,
                    "sources": app_sources,
                    "interface": null
                },
                {
                    "id": dependency.as_str(),
                    "content_hash": dependency_hash,
                    "dependencies": [],
                    "sources": dependency_sources,
                    "interface": {
                        "path": "interfaces/util.ti",
                        "sha256": interface_hash
                    }
                }
            ],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let supplied = BTreeMap::from([
            ("app/main.to".into(), Arc::<[u8]>::from(&app_source[..])),
            (
                "util/util.to".into(),
                Arc::<[u8]>::from(&dependency_source[..]),
            ),
            (
                "interfaces/util.ti".into(),
                Arc::<[u8]>::from(interface_bytes),
            ),
        ]);
        let request = ProjectPlan::parse(&manifest, &lockfile)
            .unwrap()
            .resolve(&supplied)
            .unwrap()
            .into_compilation_request(
                Operation::Check,
                DiagnosticFormat::Json,
                ResourceLimits::default(),
            )
            .unwrap();
        assert!(matches!(
            execute(request),
            Err(DriverError::Artifact(
                crate::artifact::ArtifactError::IncompatibleDependencyInterface { reason, .. }
            )) if reason == "target differs"
        ));
    }

    #[test]
    fn exact_dependency_interfaces_link_and_pin_api_hashes() {
        let app = PackageId::new("workspace:app@1").unwrap();
        let dependency = PackageId::new("registry:util@1#content").unwrap();
        let source_sets = vec!["@23:registry:util@1#content#common".to_owned()];
        let interface = CompiledInterface::new(
            "0.1".into(),
            dependency.to_string(),
            "tondo-vm-hosted".into(),
            "hosted".into(),
            vec!["console".into(), "process".into()],
            Vec::new(),
            source_sets,
            vec!["util".into()],
            sha256(b""),
            Vec::new(),
        )
        .unwrap();
        let interface_bytes = interface.encode().unwrap();
        let app_source = b"import util.util\nfn main() {}\n";
        let dependency_source = b"fn hidden(): Int { 1 }\n";
        let (manifest, lockfile) = two_package_project(
            &app,
            &dependency,
            app_source,
            dependency_source,
            &interface_bytes,
        );
        let supplied = BTreeMap::from([
            ("app/main.to".into(), Arc::<[u8]>::from(&app_source[..])),
            (
                "util/util.to".into(),
                Arc::<[u8]>::from(&dependency_source[..]),
            ),
            (
                "interfaces/util.ti".into(),
                Arc::<[u8]>::from(interface_bytes),
            ),
        ]);
        let request = ProjectPlan::parse(&manifest, &lockfile)
            .unwrap()
            .resolve(&supplied)
            .unwrap()
            .into_compilation_request(
                Operation::Check,
                DiagnosticFormat::Json,
                ResourceLimits::default(),
            )
            .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:#?}",
            output.diagnostics().diagnostics()
        );
        let dependencies = output.interface().unwrap().dependencies();
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].alias(), "util");
        assert_eq!(dependencies[0].package_id(), dependency.as_str());
        assert_eq!(dependencies[0].api_hash(), sha256(b""));
        assert_eq!(
            output.interface().unwrap().source_sets(),
            ["@15:workspace:app@1#common"]
        );
        assert_eq!(
            output.artifact().unwrap().source_sets(),
            [
                "@15:workspace:app@1#common",
                "@23:registry:util@1#content#common"
            ]
        );
    }

    #[test]
    fn privileged_units_are_canonical_pinned_target_inputs() {
        let unit = PrivilegedUnit {
            format: PRIVILEGED_UNIT_FORMAT.into(),
            id: "vendor.native".into(),
            provider: "registry:vendor-native@1".into(),
            compiler: COMPILER_ID.into(),
            target: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capability_registry: CAPABILITY_REGISTRY.into(),
            required_capabilities: vec!["process".into()],
            bindings: vec![
                PrivilegedBinding {
                    canonical_name: "vendor.native.checkedHandle".into(),
                    exposure: PrivilegedExposure::SafeWrapper,
                    signature_hash: sha256(b"safe signature"),
                    safety_contract_hash: sha256(b"safe proof"),
                    implementation_hash: sha256(b"implementation"),
                },
                PrivilegedBinding {
                    canonical_name: "vendor.native.rawHandle".into(),
                    exposure: PrivilegedExposure::UnsafeFunction,
                    signature_hash: sha256(b"unsafe signature"),
                    safety_contract_hash: sha256(b"caller obligations"),
                    implementation_hash: sha256(b"implementation"),
                },
            ],
        };
        let bytes = unit.encode().unwrap();
        assert_eq!(PrivilegedUnit::decode(&bytes).unwrap(), unit);

        let mut invalid_unit = unit.clone();
        invalid_unit.format = "tondo-privileged-unit/unsupported".into();
        assert!(matches!(
            invalid_unit.encode(),
            Err(ProjectError::InvalidPrivilegedUnit(message))
                if message.contains("unsupported format")
        ));

        let (manifest, lockfile, mut supplied) = root_project_with_unit(b"fn main() {}\n", &bytes);
        supplied.insert("units/vendor-native.tpu".into(), Arc::<[u8]>::from(bytes));
        let resolved = ProjectPlan::parse(&manifest, &lockfile)
            .unwrap()
            .resolve(&supplied)
            .unwrap();
        assert_eq!(
            resolved
                .privileged_units()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["vendor.native"]
        );
    }

    #[test]
    fn project_planner_has_no_ambient_io_surface() {
        let source = include_str!("project.rs");
        for suffix in ["env", "fs", "net", "process", "time"] {
            let forbidden = ["std::", suffix].concat();
            assert!(
                !source.contains(&forbidden),
                "project planner imports ambient API `{forbidden}`"
            );
        }
    }

    #[test]
    fn generator_inputs_cannot_collide_with_privileged_artifact_identities() {
        assert!(validate_generator_input_name("schema").is_ok());
        assert!(matches!(
            validate_generator_input_name("privileged:vendor.native"),
            Err(ProjectError::InvalidManifest(message))
                if message.contains("reserved `privileged:` prefix")
        ));
    }

    #[test]
    fn project_source_paths_require_the_language_extension() {
        assert_eq!(
            canonical_tondo_source_path("src/main.to").unwrap(),
            "src/main.to"
        );
        assert!(matches!(
            canonical_tondo_source_path("src/main.txt"),
            Err(ProjectError::InvalidManifest(message))
                if message.contains("must use the `.to` extension")
        ));
    }

    fn two_package_project(
        app: &PackageId,
        dependency: &PackageId,
        app_source: &[u8],
        dependency_source: &[u8],
        interface: &[u8],
    ) -> (Vec<u8>, Vec<u8>) {
        let manifest = serde_json::to_vec(&json!({
            "format": MANIFEST_FORMAT,
            "target": {
                "name": "tondo-vm-hosted",
                "profile": "hosted",
                "capability_registry": CAPABILITY_REGISTRY,
                "capabilities": ["console", "process"],
                "features": []
            },
            "root": {
                "package": app.as_str(),
                "source": "app/main.to",
                "form": "module"
            },
            "standard": BOOTSTRAP_STANDARD_PACKAGE,
            "packages": [
                {
                    "id": app.as_str(),
                    "local_name": "app",
                    "edition": "0.1",
                    "dependencies": [{
                        "alias": "util",
                        "package": dependency.as_str()
                    }],
                    "source_sets": [{
                        "id": "common",
                        "sources": [{
                            "physical_path": "app/main.to",
                            "logical_path": "main.to",
                            "module": "main"
                        }]
                    }]
                },
                {
                    "id": dependency.as_str(),
                    "local_name": "utilPackage",
                    "edition": "0.1",
                    "dependencies": [],
                    "source_sets": [{
                        "id": "common",
                        "sources": [{
                            "physical_path": "util/util.to",
                            "logical_path": "util.to",
                            "module": "util"
                        }]
                    }]
                }
            ],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let app_dependencies = vec![DependencyWire {
            alias: "util".into(),
            package: dependency.to_string(),
        }];
        let app_sources = vec![LockedSourceWire {
            source_set: "common".into(),
            physical_path: "app/main.to".into(),
            logical_path: "main.to".into(),
            module: "main".into(),
            sha256: sha256(app_source),
        }];
        let dependency_sources = vec![LockedSourceWire {
            source_set: "common".into(),
            physical_path: "util/util.to".into(),
            logical_path: "util.to".into(),
            module: "util".into(),
            sha256: sha256(dependency_source),
        }];
        let interface_hash = sha256(interface);
        let app_hash = package_content_hash(app, &app_dependencies, &app_sources, None).unwrap();
        let dependency_hash =
            package_content_hash(dependency, &[], &dependency_sources, Some(&interface_hash))
                .unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format": LOCKFILE_FORMAT,
            "manifest_hash": sha256(&manifest),
            "standard": {
                "package_id": BOOTSTRAP_STANDARD_PACKAGE,
                "content_hash": bootstrap_standard_hash()
            },
            "packages": [
                {
                    "id": app.as_str(),
                    "content_hash": app_hash,
                    "dependencies": app_dependencies,
                    "sources": app_sources,
                    "interface": null
                },
                {
                    "id": dependency.as_str(),
                    "content_hash": dependency_hash,
                    "dependencies": [],
                    "sources": dependency_sources,
                    "interface": {
                        "path": "interfaces/util.ti",
                        "sha256": interface_hash
                    }
                }
            ],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        (manifest, lockfile)
    }

    fn root_project_with_unit(source: &[u8], unit: &[u8]) -> ProjectFixture {
        let package = PackageId::new("workspace:app@1").unwrap();
        let manifest = serde_json::to_vec(&json!({
            "format": MANIFEST_FORMAT,
            "target": {
                "name": "tondo-vm-hosted",
                "profile": "hosted",
                "capability_registry": CAPABILITY_REGISTRY,
                "capabilities": ["console", "process"],
                "features": []
            },
            "root": {
                "package": package.as_str(),
                "source": "app/main.to",
                "form": "module"
            },
            "standard": BOOTSTRAP_STANDARD_PACKAGE,
            "packages": [{
                "id": package.as_str(),
                "local_name": "app",
                "edition": "0.1",
                "dependencies": [],
                "source_sets": [{
                    "id": "common",
                    "sources": [{
                        "physical_path": "app/main.to",
                        "logical_path": "main.to",
                        "module": "main"
                    }]
                }]
            }],
            "generator_inputs": [],
            "privileged_units": [{
                "name": "vendor.native",
                "path": "units/vendor-native.tpu"
            }]
        }))
        .unwrap();
        let sources = vec![LockedSourceWire {
            source_set: "common".into(),
            physical_path: "app/main.to".into(),
            logical_path: "main.to".into(),
            module: "main".into(),
            sha256: sha256(source),
        }];
        let content_hash = package_content_hash(&package, &[], &sources, None).unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format": LOCKFILE_FORMAT,
            "manifest_hash": sha256(&manifest),
            "standard": {
                "package_id": BOOTSTRAP_STANDARD_PACKAGE,
                "content_hash": bootstrap_standard_hash()
            },
            "packages": [{
                "id": package.as_str(),
                "content_hash": content_hash,
                "dependencies": [],
                "sources": sources,
                "interface": null
            }],
            "generator_inputs": [],
            "privileged_units": [{
                "name": "vendor.native",
                "sha256": sha256(unit)
            }]
        }))
        .unwrap();
        let supplied = BTreeMap::from([("app/main.to".into(), Arc::<[u8]>::from(source))]);
        (manifest, lockfile, supplied)
    }
}
