//! Closed source graph and executable identities for ordinary meta packages.
//!
//! The CLI discovers bytes; this module validates the existing toolchain
//! records and compiles only the explicitly selected meta dependency closure.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::driver::{
    BuildTarget, CompilationRequest, DiagnosticFormat, HostProfile, Operation, ResourceLimits,
    SourceForm,
};
use crate::meta_provider::{SourceDeriveProviders, SourceDeriveRegistration, SourceMetaProvider};
use crate::meta_vm::{MetaEntryKind, MetaVmLimits};
use crate::package::{Edition, PackageAlias, PackageGraph, PackageId, PackageNode};
use crate::project::{BOOTSTRAP_STANDARD_PACKAGE, ProjectError, bootstrap_standard_hash};
use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use crate::toolchain::{
    Limits, LockedDeriveProvider, Lockfile, Manifest, MetaPackage, StandardDescriptor, StandardRef,
};

/// The distribution owns these implicit mappings. Their fingerprints include
/// the complete embedded compiler/stdlib/VM source bundle and entry identity.
pub fn bootstrap_meta_descriptor() -> Result<StandardDescriptor, ProjectError> {
    let companion = crate::std_meta::StdMetaPackage::load_candidate().map_err(meta_error)?;
    let runtime_hash = bootstrap_standard_hash();
    let limits = MetaVmLimits::default();
    let mut providers = [
        ("Decode", crate::serialization_derive::DECODE_PROVIDER),
        ("Encode", crate::serialization_derive::ENCODE_PROVIDER),
    ]
    .into_iter()
    .map(|(name, entry)| LockedDeriveProvider {
        origin: "standard".into(),
        trait_package: BOOTSTRAP_STANDARD_PACKAGE.into(),
        trait_module: "serialization".into(),
        trait_name: name.into(),
        provider_package: crate::std_meta::STD_META_PACKAGE.into(),
        entry: entry.into(),
        meta_model: crate::meta::META_MODEL.into(),
        provider_hash: crate::artifact::sha256(
            format!("tondo-standard-derive/1\0{runtime_hash}\0{entry}").as_bytes(),
        ),
        limits: Limits {
            steps: limits.max_steps,
            memory_bytes: limits.max_live_bytes,
            output_bytes: limits.max_output_bytes,
        },
    })
    .collect::<Vec<_>>();
    providers.sort_by(|a, b| {
        (&a.trait_package, &a.trait_module, &a.trait_name).cmp(&(
            &b.trait_package,
            &b.trait_module,
            &b.trait_name,
        ))
    });
    Ok(StandardDescriptor {
        format: crate::toolchain::STANDARD_DESCRIPTOR_FORMAT.into(),
        runtime: StandardRef {
            package_id: BOOTSTRAP_STANDARD_PACKAGE.into(),
            content_hash: runtime_hash,
        },
        meta: StandardRef {
            package_id: crate::std_meta::STD_META_PACKAGE.into(),
            content_hash: companion.content_hash().into(),
        },
        derive_providers: providers,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectMetaPlan {
    pub manifest: Manifest,
    pub lock: Lockfile,
}

impl ProjectMetaPlan {
    /// Extract meta records before the existing runtime wire decoder. The full
    /// toolchain validator still sees every original field and exact manifest
    /// bytes; only runtime interface records need their existing path/hash
    /// envelope reduced to the hash expected by that validator.
    pub fn extract(
        manifest_bytes: &[u8],
        lockfile_bytes: &[u8],
    ) -> Result<(serde_json::Value, serde_json::Value, Option<Self>), ProjectError> {
        let mut manifest: serde_json::Value =
            serde_json::from_slice(manifest_bytes).map_err(meta_error)?;
        let mut lock: serde_json::Value =
            serde_json::from_slice(lockfile_bytes).map_err(meta_error)?;
        let fields = ["meta_packages", "generators", "derive_providers"];
        let has_meta = fields
            .iter()
            .any(|field| manifest.get(field).is_some() || lock.get(field).is_some())
            || lock.get("meta_standard").is_some();
        let plan = if has_meta {
            let model = Manifest::decode(manifest_bytes).map_err(meta_error)?;
            let mut normalized_lock = lock.clone();
            if let Some(packages) = normalized_lock
                .get_mut("packages")
                .and_then(serde_json::Value::as_array_mut)
            {
                for package in packages {
                    if let Some(interface) = package.get_mut("interface")
                        && let Some(hash) = interface.get("sha256")
                    {
                        *interface = hash.clone();
                    }
                }
            }
            let normalized_lock = serde_json::to_vec(&normalized_lock).map_err(meta_error)?;
            let locked = Lockfile::decode(&normalized_lock).map_err(meta_error)?;
            locked
                .validate_against(&model, manifest_bytes, &bootstrap_meta_descriptor()?)
                .map_err(meta_error)?;
            Some(Self {
                manifest: model,
                lock: locked,
            })
        } else {
            None
        };
        if let Some(object) = manifest.as_object_mut() {
            for field in fields {
                object.remove(field);
            }
        }
        if let Some(object) = lock.as_object_mut() {
            for field in fields.into_iter().chain(["meta_standard"]) {
                object.remove(field);
            }
        }
        Ok((manifest, lock, plan))
    }

    pub fn close(&self, supplied: &BTreeMap<String, Arc<[u8]>>) -> ClosedSourceMeta {
        ClosedSourceMeta {
            plan: self.clone(),
            supplied: self
                .lock
                .meta_packages
                .iter()
                .flat_map(|package| &package.sources)
                .map(|source| {
                    (
                        source.physical_path.clone(),
                        Arc::clone(&supplied[&source.physical_path]),
                    )
                })
                .chain(
                    self.manifest
                        .generator_inputs
                        .iter()
                        .map(|input| (input.path.clone(), Arc::clone(&supplied[&input.path]))),
                )
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClosedSourceMeta {
    pub plan: ProjectMetaPlan,
    pub supplied: BTreeMap<String, Arc<[u8]>>,
}

impl ClosedSourceMeta {
    pub fn compile_generators(
        &self,
        limits: ResourceLimits,
    ) -> Result<crate::meta_generate::GeneratorProviderRegistry, ProjectError> {
        let mut registry = crate::meta_generate::GeneratorProviderRegistry::default();
        let mut registered = BTreeSet::new();
        for locked in &self.plan.lock.generators {
            let key = (
                &locked.provider_package,
                &locked.entry,
                &locked.provider_hash,
            );
            if !registered.insert(key) {
                continue;
            }
            let provider = compile_source_provider(
                &self.plan.manifest.meta_packages,
                &self.supplied,
                &locked.provider_package,
                &locked.entry,
                MetaEntryKind::Generate,
                limits,
            )?;
            if provider.hash().map_err(meta_error)? != locked.provider_hash {
                return Err(meta_error(format!(
                    "generator `{}` executable hash differs from its lockfile",
                    locked.id
                )));
            }
            registry
                .insert_for(locked, Arc::new(provider))
                .map_err(meta_error)?;
        }
        Ok(registry)
    }

    pub fn compile_derives(
        &self,
        limits: ResourceLimits,
    ) -> Result<SourceDeriveProviders, ProjectError> {
        let mut providers = SourceDeriveProviders::default();
        for locked in self
            .plan
            .lock
            .derive_providers
            .iter()
            .filter(|provider| provider.origin == "manifest")
        {
            let provider = compile_source_provider(
                &self.plan.manifest.meta_packages,
                &self.supplied,
                &locked.provider_package,
                &locked.entry,
                MetaEntryKind::Derive,
                limits,
            )?;
            providers
                .insert(SourceDeriveRegistration {
                    locked: locked.clone(),
                    provider: Arc::new(provider),
                })
                .map_err(meta_error)?;
        }
        Ok(providers)
    }
}

/// Compile a discovered provider to produce or verify its lockfile hash. This
/// performs no I/O and no provider execution. The lock producer hashes supplied
/// files separately; a build must first admit them through `ProjectPlan`.
pub fn compile_source_provider(
    packages: &[MetaPackage],
    supplied: &BTreeMap<String, Arc<[u8]>>,
    package: &str,
    entry: &str,
    kind: MetaEntryKind,
    limits: ResourceLimits,
) -> Result<SourceMetaProvider, ProjectError> {
    let by_id = packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != packages.len() {
        return Err(meta_error("duplicate meta package identity"));
    }
    let mut active = BTreeSet::new();
    let mut pending = vec![package];
    while let Some(id) = pending.pop() {
        if !active.insert(id) {
            continue;
        }
        let node = by_id
            .get(id)
            .ok_or_else(|| meta_error(format!("unknown meta package `{id}`")))?;
        pending.extend(
            node.dependencies
                .iter()
                .map(|dependency| dependency.package.as_str()),
        );
    }
    let mut sources = SourceDatabase::new();
    let mut root = None;
    let entry_module = entry
        .rsplit_once('.')
        .map(|(module, _)| module)
        .ok_or_else(|| meta_error("meta entry requires a module and function name"))?;
    let mut nodes = Vec::new();
    for id in active {
        let node = by_id[id];
        if node.edition != "0.1" {
            return Err(meta_error("unsupported meta package edition"));
        }
        let source_id = SourceId::new(format!("pkg:{}:{}", id.len(), id))?;
        let mut source_order = node.sources.iter().collect::<Vec<_>>();
        source_order
            .sort_by_key(|source| (&source.module, &source.logical_path, &source.physical_path));
        for source in source_order {
            let bytes = supplied.get(&source.physical_path).ok_or_else(|| {
                meta_error(format!("missing meta source `{}`", source.physical_path))
            })?;
            let file = sources.add(SourceInput::virtual_file(
                source_id.clone(),
                ModulePath::new(&source.module)?,
                LogicalPath::new(&source.logical_path)?,
                Arc::clone(bytes),
            ))?;
            if id == package && source.module == entry_module && root.is_none() {
                root = Some(file);
            }
        }
        nodes.push(PackageNode::new(
            PackageId::new(id)?,
            source_id,
            PackageAlias::new(&node.local_name)?,
            Edition::V0_1,
            node.sources
                .iter()
                .map(|source| ModulePath::new(&source.module))
                .collect::<Result<BTreeSet<_>, _>>()?,
            node.dependencies
                .iter()
                .map(|dependency| {
                    Ok((
                        PackageAlias::new(&dependency.alias)?,
                        PackageId::new(&dependency.package)?,
                    ))
                })
                .collect::<Result<Vec<_>, crate::package::PackageGraphError>>()?,
        )?);
    }
    let standard = PackageId::new(BOOTSTRAP_STANDARD_PACKAGE)?;
    nodes.push(PackageNode::new(
        standard.clone(),
        SourceId::new(BOOTSTRAP_STANDARD_PACKAGE)?,
        PackageAlias::new("tondoStd")?,
        Edition::V0_1,
        crate::package::bootstrap_standard_modules()?,
        [],
    )?);
    let graph = PackageGraph::new(PackageId::new(package)?, standard, nodes)?;
    let root = root.ok_or_else(|| meta_error("meta provider entry module is absent"))?;
    let request = CompilationRequest::new(
        Operation::Check,
        Edition::V0_1,
        BuildTarget::tondo_meta(),
        HostProfile::Meta,
        Default::default(),
        DiagnosticFormat::Json,
        SourceForm::Module,
        limits,
        graph,
        sources,
        root,
    )?;
    SourceMetaProvider::compile(request, entry, kind).map_err(meta_error)
}

fn meta_error(error: impl std::fmt::Display) -> ProjectError {
    ProjectError::InvalidManifest(format!("meta project: {error}"))
}
