//! Local, explicit source discovery for the human `[meta]` configuration.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tondo_compiler::artifact::sha256;
use tondo_compiler::driver::ResourceLimits;
use tondo_compiler::meta::META_MODEL;
use tondo_compiler::meta_vm::MetaEntryKind;
use tondo_compiler::package::{PackageAlias, PackageId};
use tondo_compiler::project::BOOTSTRAP_STANDARD_PACKAGE;
use tondo_compiler::project_meta::{bootstrap_meta_descriptor, compile_source_provider};
use tondo_compiler::toolchain::{
    Dependency, DeriveProvider, Generator, Limits, LockedDeriveProvider, LockedGenerator,
    LockedMetaPackage, LockedMetaSource, LockedNamedInput, MetaPackage, ModelRoot, NamedPath,
    Output, Provider, Source, TraitIdentity,
};

use super::{PackageConfig, SourceSelection, collect_sources, default_edition, slash_path};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MetaConfig {
    #[serde(default)]
    dependencies: BTreeMap<String, MetaDependency>,
    #[serde(default)]
    inputs: Vec<NamedPath>,
    #[serde(default)]
    generators: Vec<GeneratorConfig>,
    #[serde(default)]
    derive_providers: Vec<DeriveConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaDependency {
    package: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratorConfig {
    id: String,
    provider: Provider,
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    model_roots: Vec<ModelRoot>,
    outputs: Vec<Output>,
    limits: Limits,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeriveConfig {
    #[serde(rename = "trait")]
    trait_: TraitIdentity,
    provider: Provider,
    limits: Limits,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetaPackageConfig {
    package: Option<PackageConfig>,
    #[serde(default)]
    dependencies: BTreeMap<String, MetaDependency>,
}

#[derive(Default)]
pub(super) struct DiscoveredMeta {
    packages: Vec<MetaPackage>,
    locked_packages: Vec<LockedMetaPackage>,
    inputs: Vec<NamedPath>,
    generators: Vec<Generator>,
    derives: Vec<DeriveProvider>,
    supplied: BTreeMap<String, Arc<[u8]>>,
}

impl DiscoveredMeta {
    pub fn discover(
        root: &Path,
        config: &MetaConfig,
        owner: &str,
        owner_name: &str,
        runtime_dependencies: &BTreeMap<String, super::DependencyConfig>,
    ) -> Result<Self, String> {
        let mut discovered = Self::default();
        let mut locations = BTreeMap::new();
        for (alias, dependency) in &config.dependencies {
            PackageAlias::new(alias).map_err(display)?;
            discovered.visit(root, root, dependency, &mut locations, &mut BTreeSet::new())?;
        }
        discovered.packages.sort_by(|a, b| a.id.cmp(&b.id));
        discovered.locked_packages.sort_by(|a, b| a.id.cmp(&b.id));
        let provider = |value: &Provider| -> Result<Provider, String> {
            let dependency = config.dependencies.get(&value.package).ok_or_else(|| {
                format!(
                    "meta provider alias `{}` is not declared in [meta.dependencies]",
                    value.package
                )
            })?;
            Ok(Provider {
                package: dependency.package.clone(),
                entry: value.entry.clone(),
            })
        };
        let runtime_package = |name: &str| -> Result<String, String> {
            if name == owner_name {
                return Ok(owner.into());
            }
            if name == "std" {
                return Ok(BOOTSTRAP_STANDARD_PACKAGE.into());
            }
            runtime_dependencies
                .get(name)
                .map(|dependency| dependency.package().into())
                .ok_or_else(|| {
                    format!("unknown runtime package alias `{name}` in meta declaration")
                })
        };
        for generator in &config.generators {
            discovered.generators.push(Generator {
                id: generator.id.clone(),
                owner_package: owner.into(),
                provider: provider(&generator.provider)?,
                meta_model: META_MODEL.into(),
                inputs: generator.inputs.clone(),
                model_roots: generator
                    .model_roots
                    .iter()
                    .map(|root| {
                        Ok(ModelRoot {
                            package: runtime_package(&root.package)?,
                            module: root.module.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                outputs: generator.outputs.clone(),
                limits: generator.limits,
            });
        }
        for derive in &config.derive_providers {
            discovered.derives.push(DeriveProvider {
                trait_: TraitIdentity {
                    package: runtime_package(&derive.trait_.package)?,
                    module: derive.trait_.module.clone(),
                    name: derive.trait_.name.clone(),
                },
                provider: provider(&derive.provider)?,
                meta_model: META_MODEL.into(),
                limits: derive.limits,
            });
        }
        for input in &config.inputs {
            let path = admitted_path(root, root, &input.path)?;
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot read meta input `{}`: {error}", path.display()))?;
            let physical = slash_path(path.strip_prefix(root).map_err(display)?);
            if physical != input.path {
                return Err(format!(
                    "meta input path `{}` must be relative and canonical",
                    input.path
                ));
            }
            if discovered
                .supplied
                .insert(physical.clone(), Arc::from(bytes))
                .is_some()
            {
                return Err(format!(
                    "meta input `{physical}` duplicates another input or source"
                ));
            }
            discovered.inputs.push(input.clone());
        }
        Ok(discovered)
    }

    fn visit(
        &mut self,
        root: &Path,
        parent: &Path,
        dependency: &MetaDependency,
        locations: &mut BTreeMap<String, PathBuf>,
        active: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        PackageId::new(&dependency.package).map_err(display)?;
        if !active.insert(dependency.package.clone()) {
            return Err(format!("meta dependency cycle at `{}`", dependency.package));
        }
        let directory = admitted_path(root, parent, &dependency.path)?;
        if let Some(previous) = locations.get(&dependency.package) {
            if previous != &directory {
                return Err(format!(
                    "meta package `{}` has conflicting source directories",
                    dependency.package
                ));
            }
            active.remove(&dependency.package);
            return Ok(());
        }
        if locations.values().any(|path| path == &directory) {
            return Err(format!(
                "meta source directory `{}` has multiple package identities",
                directory.display()
            ));
        }
        let limits = ResourceLimits::default();
        if active.len() > limits.max_syntax_depth as usize {
            return Err("meta dependency nesting exceeds the compiler depth budget".into());
        }
        locations.insert(dependency.package.clone(), directory.clone());
        let config_path = directory.join("tondo.toml");
        let config = match fs::read_to_string(&config_path) {
            Ok(text) => toml::from_str::<MetaPackageConfig>(&text).map_err(|error| {
                format!("invalid meta package `{}`: {error}", config_path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                MetaPackageConfig::default()
            }
            Err(error) => return Err(format!("cannot read `{}`: {error}", config_path.display())),
        };
        let name = config
            .package
            .as_ref()
            .and_then(|package| package.name.clone())
            .or_else(|| {
                directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .ok_or_else(|| "meta package directory needs a UTF-8 name".to_owned())?;
        PackageAlias::new(&name).map_err(display)?;
        let edition = config
            .package
            .as_ref()
            .map(|package| package.edition.clone())
            .unwrap_or_else(default_edition);
        let mut edges = Vec::new();
        for (alias, child) in &config.dependencies {
            PackageAlias::new(alias).map_err(display)?;
            self.visit(root, &directory, child, locations, active)?;
            edges.push(Dependency {
                alias: alias.clone(),
                package: child.package.clone(),
            });
        }
        let records = collect_sources(&directory, SourceSelection::Production)?;
        if records.is_empty() {
            return Err(format!(
                "meta package `{}` has no sources",
                dependency.package
            ));
        }
        let mut sources = Vec::new();
        let mut locked_sources = Vec::new();
        for record in records {
            let path = admitted_path(root, &directory, &record.physical_path)?;
            let physical = slash_path(path.strip_prefix(root).map_err(display)?);
            let bytes = fs::read(&path).map_err(display)?;
            if sha256(&bytes) != record.sha256 {
                return Err(format!("meta source `{physical}` changed during discovery"));
            }
            if self
                .supplied
                .insert(physical.clone(), Arc::from(bytes))
                .is_some()
            {
                return Err(format!(
                    "meta source `{physical}` belongs to multiple packages"
                ));
            }
            sources.push(Source {
                physical_path: physical.clone(),
                logical_path: record.logical_path.clone(),
                module: record.module.clone(),
            });
            locked_sources.push(LockedMetaSource {
                physical_path: physical,
                logical_path: record.logical_path,
                module: record.module,
                sha256: record.sha256,
            });
        }
        sources.sort_by(|a, b| {
            (&a.module, &a.logical_path, &a.physical_path).cmp(&(
                &b.module,
                &b.logical_path,
                &b.physical_path,
            ))
        });
        locked_sources.sort_by(|a, b| {
            (&a.module, &a.logical_path, &a.physical_path).cmp(&(
                &b.module,
                &b.logical_path,
                &b.physical_path,
            ))
        });
        let mut locked = LockedMetaPackage {
            id: dependency.package.clone(),
            content_hash: String::new(),
            dependencies: edges.clone(),
            sources: locked_sources,
        };
        locked.content_hash = locked.computed_content_hash().map_err(display)?;
        self.packages.push(MetaPackage {
            id: dependency.package.clone(),
            local_name: name,
            edition,
            dependencies: edges,
            sources,
        });
        self.locked_packages.push(locked);
        active.remove(&dependency.package);
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
            && self.inputs.is_empty()
            && self.generators.is_empty()
            && self.derives.is_empty()
    }

    pub fn extend_manifest(&self, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        if self.is_empty() {
            return Ok(bytes);
        }
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(display)?;
        value["meta_packages"] = json!(self.packages);
        value["generator_inputs"] = json!(self.inputs);
        value["generators"] = json!(self.generators);
        value["derive_providers"] = json!(self.derives);
        let bytes = serde_json::to_vec(&value).map_err(display)?;
        tondo_compiler::toolchain::Manifest::decode(&bytes).map_err(display)?;
        Ok(bytes)
    }

    /// Produce locked executable hashes without running any provider. Normal
    /// builds consume an existing lock and never call this resolver path.
    pub fn extend_lock(&self, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
        if self.is_empty() {
            return Ok(bytes);
        }
        let descriptor = bootstrap_meta_descriptor().map_err(display)?;
        let mut generators = Vec::new();
        let mut derives = descriptor.derive_providers;
        for generator in &self.generators {
            let compiled = compile_source_provider(
                &self.packages,
                &self.supplied,
                &generator.provider.package,
                &generator.provider.entry,
                MetaEntryKind::Generate,
                ResourceLimits::default(),
            )
            .map_err(display)?;
            let mut inputs = generator.inputs.clone();
            inputs.sort();
            let mut roots = generator.model_roots.clone();
            roots.sort_by(|a, b| (&a.package, &a.module).cmp(&(&b.package, &b.module)));
            let mut outputs = generator.outputs.clone();
            outputs.sort_by(|a, b| (&a.logical_path, &a.module).cmp(&(&b.logical_path, &b.module)));
            generators.push(LockedGenerator {
                id: generator.id.clone(),
                owner_package: generator.owner_package.clone(),
                provider_package: generator.provider.package.clone(),
                entry: generator.provider.entry.clone(),
                meta_model: META_MODEL.into(),
                provider_hash: compiled.hash().map_err(display)?,
                inputs,
                model_roots: roots,
                outputs,
                limits: generator.limits,
            });
        }
        for derive in &self.derives {
            let compiled = compile_source_provider(
                &self.packages,
                &self.supplied,
                &derive.provider.package,
                &derive.provider.entry,
                MetaEntryKind::Derive,
                ResourceLimits::default(),
            )
            .map_err(display)?;
            derives.push(LockedDeriveProvider {
                origin: "manifest".into(),
                trait_package: derive.trait_.package.clone(),
                trait_module: derive.trait_.module.clone(),
                trait_name: derive.trait_.name.clone(),
                provider_package: derive.provider.package.clone(),
                entry: derive.provider.entry.clone(),
                meta_model: META_MODEL.into(),
                provider_hash: compiled.hash().map_err(display)?,
                limits: derive.limits,
            });
        }
        generators.sort_by(|a, b| a.id.cmp(&b.id));
        derives.sort_by(|a, b| {
            (&a.trait_package, &a.trait_module, &a.trait_name).cmp(&(
                &b.trait_package,
                &b.trait_module,
                &b.trait_name,
            ))
        });
        let mut inputs = self
            .inputs
            .iter()
            .map(|input| LockedNamedInput {
                name: input.name.clone(),
                sha256: sha256(&self.supplied[&input.path]),
            })
            .collect::<Vec<_>>();
        inputs.sort_by(|a, b| a.name.cmp(&b.name));
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).map_err(display)?;
        value["meta_standard"] = json!(descriptor.meta);
        value["meta_packages"] = json!(self.locked_packages);
        value["generator_inputs"] = json!(inputs);
        value["generators"] = json!(generators);
        value["derive_providers"] = json!(derives);
        serde_json::to_vec(&value).map_err(display)
    }
}

fn admitted_path(root: &Path, parent: &Path, relative: &str) -> Result<PathBuf, String> {
    if Path::new(relative).is_absolute() {
        return Err("meta paths must be project-relative".into());
    }
    let path = parent
        .join(relative)
        .canonicalize()
        .map_err(|error| format!("cannot resolve meta path `{relative}`: {error}"))?;
    if !path.starts_with(root) {
        return Err(format!("meta path `{relative}` is outside the project"));
    }
    Ok(path)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}
