//! Explicit separation between the immutable checkpoint and the live Tondo 0.1 draft.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::{LoadedSuite, ManifestError, PinnedFile};
use crate::sha256;

pub const LIVE_LINEAGE_FORMAT: &str = "tondo-conformance-live-lineage-0.1/1";
pub const LIVE_LINEAGE_NAME: &str = "tondo-0.1-live";
pub const LIVE_LINEAGE_PATH: &str = "conformance/live/manifest.json";

const CHECKPOINT_TAG: &str = "v0.1.0";
const CHECKPOINT_COMMIT: &str = "2aec7e845ef62582015673c677c2884b97b0b8f9";
const CHECKPOINT_MANIFEST_PATH: &str = "conformance/0.1/manifest.json";
const CHECKPOINT_SPECIFICATION_PATH: &str = "conformance/checkpoints/v0.1.0/TONDO_LANGUAGE_SPEC.md";
const LIVE_SPECIFICATIONS: [&str; 4] = [
    "TONDO_LANGUAGE_SPEC.md",
    "TONDO_STANDARD_LIBRARY_SPEC.md",
    "TONDO_TESTING_SPEC.md",
    "TONDO_TOOLCHAIN_SPEC.md",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveLineageManifest {
    pub format: String,
    pub lineage: String,
    pub edition: String,
    pub revision: u32,
    pub state: String,
    pub parent: Option<PinnedFile>,
    pub checkpoint: CheckpointReference,
    pub specifications: Vec<PinnedFile>,
    pub case_layers: Vec<CaseLayer>,
    pub pending_tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReference {
    pub tag: String,
    pub commit: String,
    pub manifest: PinnedFile,
    pub specification_snapshot: PinnedFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseLayer {
    pub id: String,
    pub manifest: PinnedFile,
    pub tasks: Vec<String>,
    pub requirements: Vec<String>,
}

#[derive(Debug)]
pub enum LineageError {
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
    Checkpoint(ManifestError),
}

impl fmt::Display for LineageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "cannot read `{}`: {message}", path.display())
            }
            Self::Json(message) => write!(formatter, "invalid live lineage JSON: {message}"),
            Self::Invalid(message) => write!(formatter, "invalid live lineage: {message}"),
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "live lineage file `{path}` has SHA-256 `{actual}`, expected `{expected}`"
            ),
            Self::Checkpoint(error) => write!(formatter, "invalid checkpoint lineage: {error}"),
        }
    }
}

impl Error for LineageError {}

impl From<ManifestError> for LineageError {
    fn from(error: ManifestError) -> Self {
        Self::Checkpoint(error)
    }
}

#[derive(Debug)]
pub struct LiveLineage {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest_bytes: Vec<u8>,
    manifest: LiveLineageManifest,
    checkpoint_specification: Vec<u8>,
    checkpoint_suite: LoadedSuite,
    specifications: BTreeMap<String, Vec<u8>>,
}

impl LiveLineage {
    pub fn load(
        root: impl Into<PathBuf>,
        manifest_path: impl AsRef<Path>,
    ) -> Result<Self, LineageError> {
        let root = root.into();
        let manifest_path = manifest_path.as_ref().to_path_buf();
        let absolute_manifest = root.join(&manifest_path);
        let manifest_bytes = fs::read(&absolute_manifest).map_err(|error| LineageError::Io {
            path: absolute_manifest,
            message: error.to_string(),
        })?;
        let manifest: LiveLineageManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        validate_manifest(&manifest)?;

        let mut canonical = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        canonical.push(b'\n');
        if canonical != manifest_bytes {
            return Err(LineageError::Invalid(
                "the live manifest is not in canonical pretty JSON encoding".into(),
            ));
        }

        let checkpoint_manifest_bytes = read_pinned(&root, &manifest.checkpoint.manifest)?;
        let checkpoint_specification =
            read_pinned(&root, &manifest.checkpoint.specification_snapshot)?;
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "TONDO_LANGUAGE_SPEC.md".into(),
            checkpoint_specification.clone(),
        );
        let checkpoint_suite =
            LoadedSuite::load_with_overrides(&root, &manifest.checkpoint.manifest.path, overrides)?;
        if checkpoint_suite.manifest_sha256() != manifest.checkpoint.manifest.sha256 {
            return Err(LineageError::Invalid(
                "checkpoint suite identity differs from the pinned manifest".into(),
            ));
        }
        if checkpoint_suite.manifest().specification.path != "TONDO_LANGUAGE_SPEC.md"
            || checkpoint_suite.manifest().specification.sha256
                != manifest.checkpoint.specification_snapshot.sha256
        {
            return Err(LineageError::Invalid(
                "checkpoint specification snapshot does not match the suite manifest".into(),
            ));
        }
        if sha256(&checkpoint_manifest_bytes) != checkpoint_suite.manifest_sha256() {
            return Err(LineageError::Invalid(
                "checkpoint manifest bytes changed while loading the suite".into(),
            ));
        }

        let mut specifications = BTreeMap::new();
        for specification in &manifest.specifications {
            specifications.insert(
                specification.path.clone(),
                read_pinned(&root, specification)?,
            );
        }
        for layer in &manifest.case_layers {
            read_pinned(&root, &layer.manifest)?;
        }
        validate_history(&root, &manifest)?;

        Ok(Self {
            root,
            manifest_path,
            manifest_bytes,
            manifest,
            checkpoint_specification,
            checkpoint_suite,
            specifications,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn manifest(&self) -> &LiveLineageManifest {
        &self.manifest
    }

    pub fn manifest_sha256(&self) -> String {
        sha256(&self.manifest_bytes)
    }

    pub fn checkpoint_suite(&self) -> &LoadedSuite {
        &self.checkpoint_suite
    }

    pub fn checkpoint_specification(&self) -> &[u8] {
        &self.checkpoint_specification
    }

    pub fn specification(&self, path: &str) -> Option<&[u8]> {
        self.specifications.get(path).map(Vec::as_slice)
    }

    pub fn implemented_requirements(&self) -> BTreeSet<&str> {
        self.manifest
            .case_layers
            .iter()
            .flat_map(|layer| layer.requirements.iter().map(String::as_str))
            .collect()
    }

    pub fn check_sealable(&self) -> Result<(), LineageError> {
        if self.manifest.state != "open" {
            return Err(LineageError::Invalid(format!(
                "only an open lineage can be sealed, found `{}`",
                self.manifest.state
            )));
        }
        if !self.manifest.pending_tasks.is_empty() {
            return Err(LineageError::Invalid(format!(
                "the live lineage still has pending tasks: {}",
                self.manifest.pending_tasks.join(", ")
            )));
        }
        Ok(())
    }
}

fn validate_manifest(manifest: &LiveLineageManifest) -> Result<(), LineageError> {
    if manifest.format != LIVE_LINEAGE_FORMAT
        || manifest.lineage != LIVE_LINEAGE_NAME
        || manifest.edition != "0.1"
        || manifest.revision == 0
        || manifest.state != "open"
    {
        return invalid(
            "format, lineage, edition, revision, and state must identify the open Tondo 0.1 draft",
        );
    }
    if manifest.checkpoint.tag != CHECKPOINT_TAG
        || manifest.checkpoint.commit != CHECKPOINT_COMMIT
        || manifest.checkpoint.manifest.path != CHECKPOINT_MANIFEST_PATH
        || manifest.checkpoint.specification_snapshot.path != CHECKPOINT_SPECIFICATION_PATH
    {
        return invalid(
            "checkpoint tag, commit, manifest, or snapshot path is not the pinned 0.1 baseline",
        );
    }
    validate_pinned(&manifest.checkpoint.manifest)?;
    validate_pinned(&manifest.checkpoint.specification_snapshot)?;

    require_sorted_unique(
        "live specification paths",
        manifest
            .specifications
            .iter()
            .map(|specification| specification.path.as_str()),
    )?;
    if manifest
        .specifications
        .iter()
        .map(|specification| specification.path.as_str())
        .collect::<Vec<_>>()
        != LIVE_SPECIFICATIONS
    {
        return invalid("live specifications must contain the closed four-document set");
    }
    for specification in &manifest.specifications {
        validate_pinned(specification)?;
    }

    require_sorted_unique(
        "case layer IDs",
        manifest.case_layers.iter().map(|layer| layer.id.as_str()),
    )?;
    for layer in &manifest.case_layers {
        if layer.id.is_empty()
            || !layer
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return invalid("case layer IDs must use lowercase ASCII, digits, and hyphens");
        }
        if !layer.manifest.path.starts_with("conformance/live/layers/") {
            return invalid("case layer manifests must live below conformance/live/layers");
        }
        validate_pinned(&layer.manifest)?;
        require_sorted_unique(
            &format!("case layer `{}` tasks", layer.id),
            layer.tasks.iter().map(String::as_str),
        )?;
        require_sorted_unique(
            &format!("case layer `{}` requirements", layer.id),
            layer.requirements.iter().map(String::as_str),
        )?;
        if layer.tasks.is_empty() || layer.requirements.is_empty() {
            return invalid(format!(
                "case layer `{}` must name its implementation tasks and requirements",
                layer.id
            ));
        }
    }

    require_sorted_unique(
        "pending tasks",
        manifest.pending_tasks.iter().map(String::as_str),
    )?;
    if let Some(parent) = &manifest.parent {
        validate_pinned(parent)?;
        let expected = format!("conformance/live/history/{}.json", parent.sha256);
        if parent.path != expected {
            return invalid("a parent manifest path must be content-addressed by its SHA-256");
        }
    } else if manifest.revision != 1 {
        return invalid("only live lineage revision 1 may omit a parent manifest");
    }
    Ok(())
}

fn read_pinned(root: &Path, pinned: &PinnedFile) -> Result<Vec<u8>, LineageError> {
    validate_pinned(pinned)?;
    let physical = root.join(&pinned.path);
    let bytes = fs::read(&physical).map_err(|error| LineageError::Io {
        path: physical,
        message: error.to_string(),
    })?;
    let actual = sha256(&bytes);
    if actual != pinned.sha256 {
        return Err(LineageError::HashMismatch {
            path: pinned.path.clone(),
            expected: pinned.sha256.clone(),
            actual,
        });
    }
    Ok(bytes)
}

fn validate_history(root: &Path, manifest: &LiveLineageManifest) -> Result<(), LineageError> {
    let mut child = manifest.clone();
    while let Some(parent_file) = &child.parent {
        let parent_bytes = read_pinned(root, parent_file)?;
        let parent: LiveLineageManifest = serde_json::from_slice(&parent_bytes)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        validate_manifest(&parent)?;
        let mut canonical = serde_json::to_vec_pretty(&parent)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        canonical.push(b'\n');
        if canonical != parent_bytes {
            return invalid("a parent live manifest is not canonical pretty JSON");
        }
        if parent.revision.checked_add(1) != Some(child.revision)
            || parent.lineage != child.lineage
            || parent.edition != child.edition
            || parent.checkpoint != child.checkpoint
        {
            return invalid(
                "live history must link the immediately preceding revision in the same checkpoint lineage",
            );
        }
        child = parent;
    }
    if child.revision != 1 {
        return invalid("live history must terminate at revision 1");
    }
    Ok(())
}

fn validate_pinned(pinned: &PinnedFile) -> Result<(), LineageError> {
    if !valid_logical_path(&pinned.path) {
        return invalid(format!("`{}` is not a portable logical path", pinned.path));
    }
    if pinned.sha256.len() != 64
        || !pinned
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(format!("`{}` has an invalid SHA-256", pinned.path));
    }
    Ok(())
}

fn valid_logical_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains('\\')
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn require_sorted_unique<'a>(
    name: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), LineageError> {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invalid(format!("{name} must be sorted and unique"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> Result<T, LineageError> {
    Err(LineageError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn repository_lineage_separates_checkpoint_and_live_specifications() {
        let first = LiveLineage::load(repository_root(), LIVE_LINEAGE_PATH).unwrap();
        let second = LiveLineage::load(repository_root(), LIVE_LINEAGE_PATH).unwrap();

        assert_eq!(first.manifest_sha256(), second.manifest_sha256());
        assert_eq!(
            sha256(first.checkpoint_specification()),
            "ded4e17ab57836d032e5fb9e5be5dba03fc83ac6ff74cee90ab1bb7f8e5c7084"
        );
        assert_eq!(
            sha256(first.specification("TONDO_LANGUAGE_SPEC.md").unwrap()),
            "e24f1fd09b9d9096d0ade955e84ebc5dc89a4f2544ad517f5e01ab8eb0966266"
        );
        assert_ne!(
            first.checkpoint_specification(),
            first.specification("TONDO_LANGUAGE_SPEC.md").unwrap()
        );
        assert_eq!(
            first.checkpoint_suite().manifest_sha256(),
            "67f12434001d5d9d17b0f2181afe3ec38cb07d6207e431cca164ec4854f0148b"
        );
        assert!(first.implemented_requirements().is_empty());
        assert!(
            first
                .check_sealable()
                .unwrap_err()
                .to_string()
                .contains("pending tasks")
        );
    }

    #[test]
    fn manifest_validation_rejects_implicit_or_incomplete_layers() {
        let lineage = LiveLineage::load(repository_root(), LIVE_LINEAGE_PATH).unwrap();
        let mut unsorted = lineage.manifest().clone();
        unsorted.specifications.swap(0, 1);
        assert!(validate_manifest(&unsorted).is_err());

        let mut incomplete = lineage.manifest().clone();
        incomplete.case_layers.push(CaseLayer {
            id: "meta".into(),
            manifest: PinnedFile {
                path: "conformance/live/layers/meta.json".into(),
                sha256: "a".repeat(64),
            },
            tasks: Vec::new(),
            requirements: vec!["TL01-27-1-R001".into()],
        });
        assert!(validate_manifest(&incomplete).is_err());

        let mut broken_history = lineage.manifest().clone();
        broken_history.revision = 2;
        assert!(validate_manifest(&broken_history).is_err());
    }

    #[test]
    fn history_links_exactly_the_previous_canonical_revision() {
        let lineage = LiveLineage::load(repository_root(), LIVE_LINEAGE_PATH).unwrap();
        let parent = lineage.manifest().clone();
        let mut parent_bytes = serde_json::to_vec_pretty(&parent).unwrap();
        parent_bytes.push(b'\n');
        let parent_hash = sha256(&parent_bytes);
        let relative_path = format!("conformance/live/history/{parent_hash}.json");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "tondo-lineage-history-{}-{unique}",
            std::process::id()
        ));
        let physical = root.join(&relative_path);
        fs::create_dir_all(physical.parent().unwrap()).unwrap();
        fs::write(&physical, &parent_bytes).unwrap();

        let mut child = parent;
        child.revision = 2;
        child.parent = Some(PinnedFile {
            path: relative_path,
            sha256: parent_hash,
        });
        validate_manifest(&child).unwrap();
        validate_history(&root, &child).unwrap();

        child.revision = 3;
        assert!(
            validate_history(&root, &child)
                .unwrap_err()
                .to_string()
                .contains("immediately preceding revision")
        );
        fs::remove_dir_all(root).unwrap();
    }
}
