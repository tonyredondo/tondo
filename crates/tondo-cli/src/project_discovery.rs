//! Convention-first project discovery for the user-facing CLI.
//!
//! The compiler still consumes one closed project graph.  This module is the
//! filesystem boundary that materializes that graph from the conventional
//! source tree and the optional human-maintained `tondo.toml`.  The generated
//! JSON records never leave this process; JSON remains an internal canonical
//! representation for the existing pure project validator.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use tondo_compiler::artifact::{CAPABILITY_REGISTRY, sha256};
use tondo_compiler::driver::BuildTarget;
use tondo_compiler::package::PackageAlias;
use tondo_compiler::project::{
    BOOTSTRAP_STANDARD_PACKAGE, LOCKFILE_FORMAT, MANIFEST_FORMAT, ProjectPlan,
    bootstrap_standard_hash,
};

#[derive(Debug, Clone)]
pub(crate) struct DiscoveredProject {
    pub(crate) root: PathBuf,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) lockfile_bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceSelection {
    Production,
    ProductionAndTests,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TondoConfig {
    package: Option<PackageConfig>,
    target: Option<TargetConfig>,
    #[serde(default)]
    dependencies: BTreeMap<String, DependencyConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageConfig {
    name: Option<String>,
    #[serde(default = "default_edition")]
    edition: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetConfig {
    name: Option<String>,
    profile: Option<String>,
    capability_registry: Option<String>,
    capabilities: Option<Vec<String>>,
    features: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DependencyConfig {
    Package(String),
    Detailed(DependencyDetails),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyDetails {
    package: String,
    #[serde(default)]
    interface: Option<String>,
}

impl DependencyConfig {
    fn package(&self) -> &str {
        match self {
            Self::Package(package) => package,
            Self::Detailed(details) => &details.package,
        }
    }

    fn interface(&self) -> Option<&str> {
        match self {
            Self::Package(_) => None,
            Self::Detailed(details) => details.interface.as_deref(),
        }
    }
}

#[derive(Debug, Clone)]
struct SourceRecord {
    physical_path: String,
    logical_path: String,
    module: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct DependencyFingerprint<'a> {
    alias: &'a str,
    package: &'a str,
}

#[derive(Debug, Serialize)]
struct LockedSourceFingerprint<'a> {
    source_set: &'a str,
    physical_path: &'a str,
    logical_path: &'a str,
    module: &'a str,
    sha256: &'a str,
}

#[derive(Debug, Serialize)]
struct PackageFingerprint<'a> {
    package_id: &'a str,
    dependencies: Vec<DependencyFingerprint<'a>>,
    sources: Vec<LockedSourceFingerprint<'a>>,
    interface_hash: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedDependencyFingerprint {
    alias: String,
    package: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedLockedSourceFingerprint {
    source_set: String,
    physical_path: String,
    logical_path: String,
    module: String,
    sha256: String,
}

#[derive(Debug, Serialize)]
struct OwnedPackageFingerprint<'a> {
    package_id: &'a str,
    dependencies: &'a [OwnedDependencyFingerprint],
    sources: &'a [OwnedLockedSourceFingerprint],
    interface_hash: Option<&'a str>,
}

fn default_edition() -> String {
    "0.1".into()
}

#[allow(clippy::too_many_arguments)]
fn project_manifest(
    package_name: &str,
    edition: &str,
    target_name: &str,
    profile: &str,
    capability_registry: &str,
    capabilities: &[String],
    features: &[String],
    dependencies: &BTreeMap<String, DependencyConfig>,
    sources: &[SourceRecord],
) -> Result<Vec<u8>, String> {
    let root_source = choose_root_source(sources);
    let package_id = format!("workspace:{package_name}@local");
    let dependencies = dependencies
        .iter()
        .map(|(alias, dependency)| json!({"alias": alias, "package": dependency.package()}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "format": MANIFEST_FORMAT,
        "target": {
            "name": target_name,
            "profile": profile,
            "capability_registry": capability_registry,
            "capabilities": capabilities,
            "features": features
        },
        "root": {
            "package": package_id,
            "source": root_source.physical_path,
            "form": "module"
        },
        "standard": BOOTSTRAP_STANDARD_PACKAGE,
        "packages": [{
            "id": package_id,
            "local_name": package_name,
            "edition": edition,
            "dependencies": dependencies,
            "source_sets": [{
                "id": "common",
                "sources": sources.iter().map(|source| json!({
                    "physical_path": source.physical_path,
                    "logical_path": source.logical_path,
                    "module": source.module
                })).collect::<Vec<_>>()
            }]
        }],
        "generator_inputs": [],
        "privileged_units": []
    });
    serde_json::to_vec(&manifest)
        .map_err(|error| format!("cannot encode discovered project: {error}"))
}

/// Discover a project rooted at `root`.
pub(crate) fn discover(root: &Path) -> Result<DiscoveredProject, String> {
    discover_with_selection(root, SourceSelection::Production)
}

/// Discover the closed production and test source sets used only by
/// `tondo test`. Normal compilation deliberately never calls this entrypoint.
pub(crate) fn discover_for_tests(root: &Path) -> Result<DiscoveredProject, String> {
    discover_with_selection(root, SourceSelection::ProductionAndTests)
}

fn discover_with_selection(
    root: &Path,
    selection: SourceSelection,
) -> Result<DiscoveredProject, String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "cannot resolve project directory `{}`: {error}",
            root.display()
        )
    })?;
    if !root.is_dir() {
        return Err(format!(
            "project path `{}` is not a directory",
            root.display()
        ));
    }

    let config_path = root.join("tondo.toml");
    let has_config = config_path.is_file();
    if !has_config && !root.join("src").is_dir() && !root.join("main.to").is_file() {
        return Err(format!(
            "a source file is required: project `{}` has no `src/`, `main.to` or `tondo.toml`",
            root.display()
        ));
    }
    let config: TondoConfig = match fs::read(&config_path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|error| format!("invalid `{}`: {error}", config_path.display()))?;
            toml::from_str(&text)
                .map_err(|error| format!("invalid `{}`: {error}", config_path.display()))?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TondoConfig::default(),
        Err(error) => {
            return Err(format!("cannot read `{}`: {error}", config_path.display()));
        }
    };

    let production_sources = collect_sources(&root, SourceSelection::Production)?;
    if production_sources.is_empty() && selection == SourceSelection::Production {
        return Err(format!(
            "a source file is required: project `{}` contains no production `.to` source files",
            root.display()
        ));
    }
    let sources = match selection {
        SourceSelection::Production => production_sources.clone(),
        SourceSelection::ProductionAndTests => {
            collect_sources(&root, SourceSelection::ProductionAndTests)?
        }
    };
    if sources.is_empty() {
        return Err(format!(
            "a source file is required: project `{}` contains no `.to` source files",
            root.display()
        ));
    }
    let package_name = config
        .package
        .as_ref()
        .and_then(|package| package.name.clone())
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("app")
                .to_owned()
        });
    PackageAlias::new(package_name.clone())
        .map_err(|error| format!("invalid package name `{package_name}`: {error}"))?;
    let edition = config
        .package
        .as_ref()
        .map(|package| package.edition.clone())
        .unwrap_or_else(default_edition);
    let target = config.target.unwrap_or_default();
    let target_name = target.name.unwrap_or_else(|| "tondo-vm-hosted".into());
    let profile = target.profile.unwrap_or_else(|| "hosted".into());
    let capability_registry = target
        .capability_registry
        .unwrap_or_else(|| CAPABILITY_REGISTRY.into());
    let capabilities = target.capabilities.unwrap_or_else(|| {
        BuildTarget::vm_hosted_capabilities()
            .into_iter()
            .map(|capability| capability.to_string())
            .collect()
    });
    let features = target.features.unwrap_or_default();
    let package_id = format!("workspace:{package_name}@local");
    let production_manifest_bytes = (!production_sources.is_empty())
        .then(|| {
            project_manifest(
                &package_name,
                &edition,
                &target_name,
                &profile,
                &capability_registry,
                &capabilities,
                &features,
                &config.dependencies,
                &production_sources,
            )
        })
        .transpose()?;
    let manifest_bytes = match selection {
        SourceSelection::Production => production_manifest_bytes
            .clone()
            .expect("production discovery rejected an empty production source set"),
        SourceSelection::ProductionAndTests => project_manifest(
            &package_name,
            &edition,
            &target_name,
            &profile,
            &capability_registry,
            &capabilities,
            &features,
            &config.dependencies,
            &sources,
        )?,
    };

    let lock_path = root.join("tondo.lock.toml");
    let lockfile_bytes = match fs::read(&lock_path) {
        Ok(bytes) => {
            if config
                .dependencies
                .values()
                .any(|dependency| dependency.interface().is_some())
            {
                for dependency in config.dependencies.values() {
                    if let Some(interface) = dependency.interface() {
                        let path = root.join(interface);
                        if !path.is_file() {
                            return Err(format!(
                                "dependency interface `{}` does not exist",
                                path.display()
                            ));
                        }
                    }
                }
            }
            let production_lock = toml_to_json(&bytes, &lock_path)?;
            let production_manifest = production_manifest_bytes.as_deref().ok_or_else(|| {
                "a persistent lockfile requires at least one production source".to_owned()
            })?;
            ProjectPlan::parse(production_manifest, &production_lock).map_err(|error| {
                format!(
                    "production lockfile `{}` does not match the publishable project: {error}",
                    lock_path.display()
                )
            })?;
            match selection {
                SourceSelection::Production => production_lock,
                SourceSelection::ProductionAndTests => {
                    derive_test_lockfile(&production_lock, &manifest_bytes, &package_id, &sources)?
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !config.dependencies.is_empty() {
                return Err(format!(
                    "external dependencies require `{}`; run the dependency resolver first",
                    lock_path.display()
                ));
            }
            generated_lockfile(&manifest_bytes, &package_id, &sources)?
        }
        Err(error) => {
            return Err(format!("cannot read `{}`: {error}", lock_path.display()));
        }
    };

    Ok(DiscoveredProject {
        root,
        manifest_bytes,
        lockfile_bytes,
    })
}

fn toml_to_json(bytes: &[u8], path: &Path) -> Result<Vec<u8>, String> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("invalid `{}`: {error}", path.display()))?;
    let value = toml::from_str::<toml::Value>(&text)
        .map_err(|error| format!("invalid `{}`: {error}", path.display()))?;
    serde_json::to_vec(&value)
        .map_err(|error| format!("cannot normalize `{}`: {error}", path.display()))
}

fn derive_test_lockfile(
    production_lock: &[u8],
    test_manifest: &[u8],
    package_id: &str,
    sources: &[SourceRecord],
) -> Result<Vec<u8>, String> {
    let mut lock: serde_json::Value = serde_json::from_slice(production_lock)
        .map_err(|error| format!("cannot decode validated production lockfile: {error}"))?;
    let packages = lock
        .get_mut("packages")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "validated production lockfile has no package array".to_owned())?;
    let package = packages
        .iter_mut()
        .find(|package| package.get("id").and_then(serde_json::Value::as_str) == Some(package_id))
        .ok_or_else(|| {
            format!("validated production lockfile omits root package `{package_id}`")
        })?;
    let mut dependencies = serde_json::from_value::<Vec<OwnedDependencyFingerprint>>(
        package
            .get("dependencies")
            .cloned()
            .ok_or_else(|| "validated root package omits dependencies".to_owned())?,
    )
    .map_err(|error| format!("cannot decode validated root dependencies: {error}"))?;
    dependencies
        .sort_by(|left, right| (&left.alias, &left.package).cmp(&(&right.alias, &right.package)));
    let mut locked_sources = sources
        .iter()
        .map(|source| OwnedLockedSourceFingerprint {
            source_set: "common".into(),
            physical_path: source.physical_path.clone(),
            logical_path: source.logical_path.clone(),
            module: source.module.clone(),
            sha256: source.sha256.clone(),
        })
        .collect::<Vec<_>>();
    locked_sources.sort_by(|left, right| left.physical_path.cmp(&right.physical_path));
    let content_hash = sha256(
        &serde_json::to_vec(&OwnedPackageFingerprint {
            package_id,
            dependencies: &dependencies,
            sources: &locked_sources,
            interface_hash: None,
        })
        .map_err(|error| format!("cannot encode test package fingerprint: {error}"))?,
    );
    let package = package
        .as_object_mut()
        .ok_or_else(|| "validated root package is not an object".to_owned())?;
    package.insert("content_hash".into(), content_hash.into());
    package.insert(
        "sources".into(),
        serde_json::to_value(locked_sources)
            .map_err(|error| format!("cannot encode test source lock: {error}"))?,
    );
    lock.as_object_mut()
        .ok_or_else(|| "validated production lockfile is not an object".to_owned())?
        .insert("manifest_hash".into(), sha256(test_manifest).into());
    let bytes = serde_json::to_vec(&lock)
        .map_err(|error| format!("cannot encode derived test lockfile: {error}"))?;
    ProjectPlan::parse(test_manifest, &bytes)
        .map_err(|error| format!("derived test lockfile is inconsistent: {error}"))?;
    Ok(bytes)
}

fn generated_lockfile(
    manifest_bytes: &[u8],
    package_id: &str,
    sources: &[SourceRecord],
) -> Result<Vec<u8>, String> {
    let dependencies = Vec::<DependencyFingerprint<'_>>::new();
    let locked_sources = sources
        .iter()
        .map(|source| LockedSourceFingerprint {
            source_set: "common",
            physical_path: &source.physical_path,
            logical_path: &source.logical_path,
            module: &source.module,
            sha256: &source.sha256,
        })
        .collect::<Vec<_>>();
    let package_hash = sha256(
        &serde_json::to_vec(&PackageFingerprint {
            package_id,
            dependencies,
            sources: locked_sources,
            interface_hash: None,
        })
        .map_err(|error| format!("cannot encode package fingerprint: {error}"))?,
    );
    let lockfile = json!({
        "format": LOCKFILE_FORMAT,
        "manifest_hash": sha256(manifest_bytes),
        "standard": {
            "package_id": BOOTSTRAP_STANDARD_PACKAGE,
            "content_hash": bootstrap_standard_hash()
        },
        "packages": [{
            "id": package_id,
            "content_hash": package_hash,
            "dependencies": [],
            "sources": sources.iter().map(|source| json!({
                "source_set": "common",
                "physical_path": source.physical_path,
                "logical_path": source.logical_path,
                "module": source.module,
                "sha256": source.sha256
            })).collect::<Vec<_>>(),
            "interface": null
        }],
        "generator_inputs": [],
        "privileged_units": []
    });
    serde_json::to_vec(&lockfile).map_err(|error| format!("cannot encode lockfile: {error}"))
}

fn collect_sources(root: &Path, selection: SourceSelection) -> Result<Vec<SourceRecord>, String> {
    let mut files = Vec::new();
    let src = root.join("src");
    if src.is_dir() {
        collect_dir(
            &src,
            &mut files,
            selection == SourceSelection::ProductionAndTests,
        )?;
    } else {
        collect_root_sources(
            root,
            &mut files,
            selection == SourceSelection::ProductionAndTests,
        )?;
    }
    if selection == SourceSelection::ProductionAndTests {
        let tests = root.join("tests");
        if tests.is_dir() {
            collect_dir(&tests, &mut files, true)?;
        }
    }
    files.sort();
    files.dedup();
    files
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| format!("source `{}` escapes project root", path.display()))?;
            let physical_path = slash_path(relative);
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot read source `{}`: {error}", path.display()))?;
            Ok(SourceRecord {
                logical_path: physical_path.clone(),
                module: module_for_path(relative),
                physical_path,
                sha256: sha256(&bytes),
            })
        })
        .collect()
}

fn collect_root_sources(
    root: &Path,
    files: &mut Vec<PathBuf>,
    include_tests: bool,
) -> Result<(), String> {
    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "cannot read project directory `{}`: {error}",
            root.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect project entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect `{}`: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("to")
            && (include_tests || !is_test_source(&path))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_dir(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    include_tests: bool,
) -> Result<(), String> {
    let entries = fs::read_dir(directory).map_err(|error| {
        format!(
            "cannot read project directory `{}`: {error}",
            directory.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect project entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect `{}`: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.starts_with('.') || matches!(name, "target" | "vendor") {
                continue;
            }
            collect_dir(&path, files, include_tests)?;
        } else if metadata.is_file()
            && path.extension().and_then(|extension| extension.to_str()) == Some("to")
            && (include_tests || !is_test_source(&path))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn is_test_source(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("_test"))
}

fn choose_root_source(sources: &[SourceRecord]) -> &SourceRecord {
    sources
        .iter()
        .find(|source| source.physical_path == "src/main.to")
        .or_else(|| {
            sources
                .iter()
                .find(|source| source.physical_path == "main.to")
        })
        .unwrap_or(&sources[0])
}

fn module_for_path(path: &Path) -> String {
    let mut components = path.components().collect::<Vec<_>>();
    if components
        .first()
        .is_some_and(|component| component.as_os_str() == "src")
    {
        components.remove(0);
    }
    components.pop();
    if components.is_empty() {
        return "main".into();
    }
    components
        .into_iter()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_project() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock must be after epoch")
            .as_nanos();
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tondo-discovery-{nonce}-{id}"));
        fs::create_dir_all(root.join("src/models")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/main.to"), b"fn main() {}\n").unwrap();
        fs::write(root.join("src/models/user.to"), b"type User = String\n").unwrap();
        fs::write(
            root.join("tests/smoke.to"),
            b"test smoke { assert(true) }\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn discovers_conventional_sources_and_generates_a_closed_internal_graph() {
        let root = temporary_project();
        fs::write(
            root.join("src/models/user_test.to"),
            b"test user { assert(true) }\n",
        )
        .unwrap();
        fs::write(
            root.join("tondo.toml"),
            "[package]\nname = \"demo\"\nedition = \"0.1\"\n",
        )
        .unwrap();
        let discovered = discover(&root).unwrap();
        let project = tondo_compiler::project::ProjectPlan::parse(
            &discovered.manifest_bytes,
            &discovered.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(project.target_name(), "tondo-vm-hosted");
        assert_eq!(project.selected_source_paths().count(), 2);
        assert!(
            project
                .selected_source_paths()
                .all(|path| !path.starts_with("tests/") && !path.ends_with("_test.to"))
        );
        assert_eq!(project.root_source_path(), "src/main.to");

        let mut lock: serde_json::Value =
            serde_json::from_slice(&discovered.lockfile_bytes).unwrap();
        lock["packages"][0]
            .as_object_mut()
            .unwrap()
            .remove("interface");
        fs::write(
            root.join("tondo.lock.toml"),
            toml::to_string(&toml::Value::try_from(lock).unwrap()).unwrap(),
        )
        .unwrap();

        let discovered = discover_for_tests(&root).unwrap();
        let project = tondo_compiler::project::ProjectPlan::parse(
            &discovered.manifest_bytes,
            &discovered.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(project.selected_source_paths().count(), 4);
        assert!(
            project
                .selected_source_paths()
                .any(|path| path == "src/models/user_test.to")
        );
        assert!(
            project
                .selected_source_paths()
                .any(|path| path == "tests/smoke.to")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_dependencies_without_a_lockfile() {
        let root = temporary_project();
        fs::write(
            root.join("tondo.toml"),
            "[package]\nname = \"demo\"\n\n[dependencies]\nhttp = \"registry:http@1\"\n",
        )
        .unwrap();
        let error = discover(&root).unwrap_err();
        assert!(error.contains("tondo.lock.toml"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn toml_lockfiles_are_normalized_to_the_existing_pure_wire_boundary() {
        let root = temporary_project();
        fs::write(
            root.join("tondo.lock.toml"),
            "format = \"tondo-lock-draft\"\n",
        )
        .unwrap();
        let error = discover(&root).unwrap_err();
        assert!(error.contains("invalid") || error.contains("manifest"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn custom_target_and_dependency_spellings_still_require_a_closed_lock() {
        let root = temporary_project();
        fs::create_dir_all(root.join("interfaces")).unwrap();
        fs::write(root.join("interfaces/http.ti"), b"interface").unwrap();
        fs::write(
            root.join("tondo.toml"),
            "[package]\nname = \"demo\"\n\n[target]\nname = \"tondo-vm-hosted\"\nprofile = \"hosted\"\ncapability_registry = \"tondo-capabilities-draft\"\ncapabilities = [\"console\"]\nfeatures = [\"fast\"]\n\n[dependencies]\nhttp = \"registry:http@1\"\nserde = { package = \"registry:serde@1\", interface = \"interfaces/http.ti\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("tondo.lock.toml"),
            "format = \"tondo-lock-draft\"\n",
        )
        .unwrap();
        let error = discover(&root).unwrap_err();
        assert!(error.contains("production lockfile") && error.contains("manifest_hash"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unrecognized_project_shapes_before_package_validation() {
        let root = std::env::temp_dir().join(format!(
            "tondo-discovery-unmarked-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("nested.to"), b"fn main() {}\n").unwrap();
        let error = discover(&root).unwrap_err();
        assert!(error.contains("src/") && error.contains("tondo.toml"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_invalid_toml_and_unknown_config_fields() {
        let root = temporary_project();
        fs::write(root.join("tondo.toml"), "[package\n").unwrap();
        assert!(discover(&root).unwrap_err().contains("invalid"));
        fs::write(root.join("tondo.toml"), "mystery = true\n").unwrap();
        assert!(discover(&root).unwrap_err().contains("invalid"));
        fs::write(root.join("tondo.toml"), "[package]\nname = \"bad-name\"\n").unwrap();
        assert!(
            discover(&root)
                .unwrap_err()
                .contains("invalid package name")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_sources_and_ignored_directories_follow_conventions() {
        let root = std::env::temp_dir().join(format!(
            "tondo-root-project-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("tests/nested")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("vendor")).unwrap();
        fs::write(root.join("main.to"), b"fn main() {}\n").unwrap();
        fs::write(
            root.join("tests/nested/smoke.to"),
            b"test smoke { assert(true) }\n",
        )
        .unwrap();
        fs::write(root.join(".hidden/ignored.to"), b"not valid").unwrap();
        fs::write(root.join("target/ignored.to"), b"not valid").unwrap();
        fs::write(root.join("vendor/ignored.to"), b"not valid").unwrap();
        fs::write(root.join("tondo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let discovered = discover(&root).unwrap();
        let project = tondo_compiler::project::ProjectPlan::parse(
            &discovered.manifest_bytes,
            &discovered.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(project.selected_source_paths().count(), 1);
        assert_eq!(project.root_source_path(), "main.to");

        let discovered = discover_for_tests(&root).unwrap();
        let project = tondo_compiler::project::ProjectPlan::parse(
            &discovered.manifest_bytes,
            &discovered.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(project.selected_source_paths().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_and_path_normalization_are_stable() {
        assert_eq!(module_for_path(Path::new("src/main.to")), "main");
        assert_eq!(module_for_path(Path::new("src/models/user.to")), "models");
        assert_eq!(
            module_for_path(Path::new("tests/http/client.to")),
            "tests.http"
        );
        assert_eq!(
            slash_path(Path::new("src/models/user.to")),
            "src/models/user.to"
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_sources_and_rejects_invalid_utf8_toml() {
        use std::os::unix::fs::symlink;

        let root = temporary_project();
        symlink(root.join("src/main.to"), root.join("src/linked.to")).unwrap();
        fs::write(root.join("tondo.toml"), vec![0xff, 0xfe]).unwrap();
        assert!(discover(&root).unwrap_err().contains("invalid"));
        fs::remove_file(root.join("tondo.toml")).unwrap();
        fs::write(root.join("tondo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        let discovered = discover(&root).unwrap();
        let project = tondo_compiler::project::ProjectPlan::parse(
            &discovered.manifest_bytes,
            &discovered.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(project.selected_source_paths().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_test_sources_are_visible_only_to_the_test_discovery_entrypoint() {
        let root = std::env::temp_dir().join(format!(
            "tondo-root-tests-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.to"), b"fn main() {}\n").unwrap();
        fs::write(root.join("math_test.to"), b"test math { assert(true) }\n").unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/api.to"), b"test api { assert(true) }\n").unwrap();
        fs::write(
            root.join("tondo.toml"),
            "[package]\nname = \"root_tests\"\n",
        )
        .unwrap();

        let production = discover(&root).unwrap();
        let production = tondo_compiler::project::ProjectPlan::parse(
            &production.manifest_bytes,
            &production.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(
            production.selected_source_paths().collect::<Vec<_>>(),
            ["main.to"]
        );

        let tests = discover_for_tests(&root).unwrap();
        let tests = tondo_compiler::project::ProjectPlan::parse(
            &tests.manifest_bytes,
            &tests.lockfile_bytes,
        )
        .unwrap();
        assert_eq!(
            tests.selected_source_paths().collect::<Vec<_>>(),
            ["main.to", "math_test.to", "tests/api.to"]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
