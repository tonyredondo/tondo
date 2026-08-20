//! The single current conformance draft plus its bootstrap regression corpus.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::{LoadedSuite, ManifestError, PinnedFile};
use crate::sha256;

pub const DRAFT_LINEAGE_FORMAT: &str = "tondo-conformance-draft-lineage";
pub const DRAFT_LINEAGE_NAME: &str = "tondo-draft";
pub const DRAFT_LINEAGE_PATH: &str = "conformance/draft/manifest.json";
pub const CASE_LAYER_FORMAT: &str = "tondo-conformance-case-layer/1";

const LIVE_SUITE_MANIFEST_PATH: &str = "conformance/0.1/manifest.json";
const ACTIVE_SPECIFICATIONS: [&str; 3] = [
    "TONDO_LANGUAGE_SPEC.md",
    "TONDO_TESTING_SPEC.md",
    "TONDO_TOOLCHAIN_SPEC.md",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftLineageManifest {
    pub format: String,
    pub lineage: String,
    pub edition: String,
    pub state: String,
    pub suite: PinnedFile,
    pub specifications: Vec<PinnedFile>,
    pub case_layers: Vec<CaseLayer>,
    pub pending_tasks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseLayer {
    pub id: String,
    pub manifest: PinnedFile,
    pub tasks: Vec<String>,
    pub requirements: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftCaseLayerManifest {
    pub format: String,
    pub layer: String,
    pub edition: String,
    pub tasks: Vec<String>,
    pub cases: Vec<DraftContractCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftContractCase {
    pub id: String,
    pub surface: String,
    pub requirements: Vec<String>,
    pub evidence: Vec<String>,
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
    Suite(ManifestError),
}

impl fmt::Display for LineageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "cannot read `{}`: {message}", path.display())
            }
            Self::Json(message) => write!(formatter, "invalid draft lineage JSON: {message}"),
            Self::Invalid(message) => write!(formatter, "invalid draft lineage: {message}"),
            Self::HashMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "draft lineage file `{path}` has SHA-256 `{actual}`, expected `{expected}`"
            ),
            Self::Suite(error) => write!(formatter, "invalid live suite: {error}"),
        }
    }
}

impl Error for LineageError {}

impl From<ManifestError> for LineageError {
    fn from(error: ManifestError) -> Self {
        Self::Suite(error)
    }
}

#[derive(Debug)]
pub struct DraftLineage {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest_bytes: Vec<u8>,
    manifest: DraftLineageManifest,
    suite: LoadedSuite,
    specifications: BTreeMap<String, Vec<u8>>,
    case_layers: Vec<DraftCaseLayerManifest>,
}

impl DraftLineage {
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
        let manifest: DraftLineageManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        validate_active_manifest(&manifest)?;

        let mut canonical = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| LineageError::Json(error.to_string()))?;
        canonical.push(b'\n');
        if canonical != manifest_bytes {
            return Err(LineageError::Invalid(
                "the draft manifest is not in canonical pretty JSON encoding".into(),
            ));
        }

        let suite_manifest_bytes = read_pinned(&root, &manifest.suite)?;
        let suite = LoadedSuite::load(&root, &manifest.suite.path)?;
        if suite.manifest_sha256() != manifest.suite.sha256 {
            return Err(LineageError::Invalid(
                "live suite identity differs from the pinned manifest".into(),
            ));
        }
        if sha256(&suite_manifest_bytes) != suite.manifest_sha256() {
            return Err(LineageError::Invalid(
                "live manifest bytes changed while loading the suite".into(),
            ));
        }

        let mut specifications = BTreeMap::new();
        for specification in &manifest.specifications {
            specifications.insert(
                specification.path.clone(),
                read_pinned(&root, specification)?,
            );
        }
        if specifications
            .get(&suite.manifest().specification.path)
            .is_none_or(|bytes| sha256(bytes) != suite.manifest().specification.sha256)
        {
            return Err(LineageError::Invalid(
                "the live suite specification is not the current language specification".into(),
            ));
        }
        let mut case_layers = Vec::with_capacity(manifest.case_layers.len());
        for layer in &manifest.case_layers {
            let bytes = read_pinned(&root, &layer.manifest)?;
            let layer_manifest: DraftCaseLayerManifest = serde_json::from_slice(&bytes)
                .map_err(|error| LineageError::Json(error.to_string()))?;
            validate_case_layer(layer, &layer_manifest)?;
            let mut canonical = serde_json::to_vec_pretty(&layer_manifest)
                .map_err(|error| LineageError::Json(error.to_string()))?;
            canonical.push(b'\n');
            if canonical != bytes {
                return invalid(format!(
                    "case layer `{}` is not canonical pretty JSON",
                    layer.id
                ));
            }
            case_layers.push(layer_manifest);
        }
        Ok(Self {
            root,
            manifest_path,
            manifest_bytes,
            manifest,
            suite,
            specifications,
            case_layers,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn manifest(&self) -> &DraftLineageManifest {
        &self.manifest
    }

    pub fn manifest_sha256(&self) -> String {
        sha256(&self.manifest_bytes)
    }

    pub fn suite(&self) -> &LoadedSuite {
        &self.suite
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

    pub fn case_layers(&self) -> &[DraftCaseLayerManifest] {
        &self.case_layers
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
                "the draft still has pending tasks: {}",
                self.manifest.pending_tasks.join(", ")
            )));
        }
        Ok(())
    }
}

pub(crate) fn validate_case_layer(
    descriptor: &CaseLayer,
    manifest: &DraftCaseLayerManifest,
) -> Result<(), LineageError> {
    if manifest.format != CASE_LAYER_FORMAT
        || manifest.layer != descriptor.id
        || manifest.edition != "0.1"
        || manifest.cases.is_empty()
    {
        return invalid(format!(
            "case layer `{}` has an invalid format, identity, edition, or empty case set",
            descriptor.id
        ));
    }
    require_sorted_unique(
        &format!("case layer `{}` implementation tasks", descriptor.id),
        manifest.tasks.iter().map(String::as_str),
    )?;
    if manifest.tasks != descriptor.tasks {
        return invalid(format!(
            "case layer `{}` task set differs from its lineage descriptor",
            descriptor.id
        ));
    }
    require_sorted_unique(
        &format!("case layer `{}` case IDs", descriptor.id),
        manifest.cases.iter().map(|case| case.id.as_str()),
    )?;
    let mut requirements = BTreeSet::new();
    for case in &manifest.cases {
        for (name, value) in [("case ID", &case.id), ("surface", &case.surface)] {
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return invalid(format!(
                    "case layer `{}` {name} must use lowercase ASCII, digits, and hyphens",
                    descriptor.id
                ));
            }
        }
        require_sorted_unique(
            &format!("case layer `{}` case requirements", descriptor.id),
            case.requirements.iter().map(String::as_str),
        )?;
        require_sorted_unique(
            &format!("case layer `{}` case evidence", descriptor.id),
            case.evidence.iter().map(String::as_str),
        )?;
        if case.requirements.is_empty()
            || case.evidence.is_empty()
            || case
                .evidence
                .iter()
                .any(|id| !(id.starts_with("rust:") || id.starts_with("fuzz:")))
        {
            return invalid(format!(
                "case layer `{}` cases require requirements and executable inventory evidence",
                descriptor.id
            ));
        }
        requirements.extend(case.requirements.iter().cloned());
    }
    if requirements.into_iter().collect::<Vec<_>>() != descriptor.requirements {
        return invalid(format!(
            "case layer `{}` requirement set differs from its lineage descriptor",
            descriptor.id
        ));
    }
    Ok(())
}

pub(crate) fn validate_active_manifest(
    manifest: &DraftLineageManifest,
) -> Result<(), LineageError> {
    validate_manifest(manifest)?;
    if manifest
        .specifications
        .iter()
        .map(|specification| specification.path.as_str())
        .collect::<Vec<_>>()
        != ACTIVE_SPECIFICATIONS
    {
        return invalid("the active draft must contain the current contract set");
    }
    Ok(())
}

pub(crate) fn validate_manifest(manifest: &DraftLineageManifest) -> Result<(), LineageError> {
    if manifest.format != DRAFT_LINEAGE_FORMAT
        || manifest.lineage != DRAFT_LINEAGE_NAME
        || manifest.edition != "0.1"
        || manifest.state != "open"
    {
        return invalid(
            "format, lineage, edition, and state must identify the open Tondo 0.1 draft",
        );
    }
    if manifest.suite.path != LIVE_SUITE_MANIFEST_PATH {
        return invalid("the live suite path is not the current conformance corpus");
    }
    validate_pinned(&manifest.suite)?;

    require_sorted_unique(
        "draft specification paths",
        manifest
            .specifications
            .iter()
            .map(|specification| specification.path.as_str()),
    )?;
    let specification_paths = manifest
        .specifications
        .iter()
        .map(|specification| specification.path.as_str())
        .collect::<Vec<_>>();
    if specification_paths != ACTIVE_SPECIFICATIONS {
        return invalid("draft specifications differ from the current contract set");
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
        if !layer.manifest.path.starts_with("conformance/draft/layers/") {
            return invalid("case layer manifests must be below conformance/draft/layers");
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
    use super::*;

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn repository_draft_uses_one_live_suite_and_specification() {
        let first = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();
        let second = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();

        assert_eq!(first.manifest_sha256(), second.manifest_sha256());
        assert_eq!(
            first.suite().manifest().specification.sha256,
            sha256(first.specification("TONDO_LANGUAGE_SPEC.md").unwrap())
        );
        assert_eq!(first.case_layers().len(), 4);
        assert_eq!(first.case_layers()[0].layer, "finalization");
        assert_eq!(first.case_layers()[0].cases.len(), 6);
        assert_eq!(first.case_layers()[1].layer, "language-closure");
        assert_eq!(first.case_layers()[1].cases.len(), 2);
        assert_eq!(first.case_layers()[2].layer, "meta");
        assert_eq!(first.case_layers()[2].cases.len(), 6);
        assert_eq!(first.case_layers()[3].layer, "testing");
        assert_eq!(first.case_layers()[3].cases.len(), 53);
        assert_eq!(first.implemented_requirements().len(), 56);
        assert_eq!(first.manifest().pending_tasks, ["CONF-SEAL-FINAL-001"]);
        assert!(first.check_sealable().is_err());
    }

    #[test]
    fn lineage_errors_and_accessors_have_stable_contracts() {
        assert_eq!(
            LineageError::Io {
                path: PathBuf::from("missing.json"),
                message: "not found".into(),
            }
            .to_string(),
            "cannot read `missing.json`: not found"
        );
        assert_eq!(
            LineageError::Json("broken".into()).to_string(),
            "invalid draft lineage JSON: broken"
        );
        assert_eq!(
            LineageError::Invalid("bad state".into()).to_string(),
            "invalid draft lineage: bad state"
        );
        assert_eq!(
            LineageError::HashMismatch {
                path: "manifest.json".into(),
                expected: "a".repeat(64),
                actual: "b".repeat(64),
            }
            .to_string(),
            format!(
                "draft lineage file `manifest.json` has SHA-256 `{}`, expected `{}`",
                "b".repeat(64),
                "a".repeat(64)
            )
        );
        let suite = LineageError::from(ManifestError::Invalid("bad suite".into()));
        assert_eq!(
            suite.to_string(),
            "invalid live suite: invalid suite manifest: bad suite"
        );

        let lineage = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();
        assert_eq!(lineage.root(), repository_root());
        assert_eq!(lineage.manifest_path(), Path::new(DRAFT_LINEAGE_PATH));
        assert!(lineage.specification("missing.md").is_none());
        let missing =
            DraftLineage::load(repository_root(), "conformance/draft/missing.json").unwrap_err();
        assert!(missing.to_string().contains("cannot read"));
    }

    #[test]
    fn manifest_validation_closes_identity_paths_and_layers() {
        let lineage = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();
        let assert_invalid = |manifest: DraftLineageManifest| {
            assert!(validate_manifest(&manifest).is_err(), "{manifest:?}");
        };

        for mutate in [
            |manifest: &mut DraftLineageManifest| manifest.format = "future".into(),
            |manifest: &mut DraftLineageManifest| manifest.lineage = "other".into(),
            |manifest: &mut DraftLineageManifest| manifest.edition = "0.2".into(),
            |manifest: &mut DraftLineageManifest| manifest.state = "sealed".into(),
            |manifest: &mut DraftLineageManifest| manifest.suite.path = "other.json".into(),
            |manifest: &mut DraftLineageManifest| {
                let _ = manifest.specifications.pop();
            },
        ] {
            let mut invalid = lineage.manifest().clone();
            mutate(&mut invalid);
            assert_invalid(invalid);
        }

        let mut invalid_layer = lineage.manifest().clone();
        invalid_layer.case_layers.push(CaseLayer {
            id: "Meta".into(),
            manifest: PinnedFile {
                path: "conformance/draft/layers/meta.json".into(),
                sha256: "a".repeat(64),
            },
            tasks: vec!["M".into()],
            requirements: vec!["R".into()],
        });
        assert_invalid(invalid_layer);

        let mut invalid_layer_path = lineage.manifest().clone();
        invalid_layer_path.case_layers.push(CaseLayer {
            id: "meta".into(),
            manifest: PinnedFile {
                path: "conformance/other/meta.json".into(),
                sha256: "a".repeat(64),
            },
            tasks: vec!["M".into()],
            requirements: vec!["R".into()],
        });
        assert_invalid(invalid_layer_path);
    }

    #[test]
    fn manifest_validation_rejects_implicit_or_incomplete_layers() {
        let lineage = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();
        let mut unsorted = lineage.manifest().clone();
        unsorted.specifications.swap(0, 1);
        assert!(validate_manifest(&unsorted).is_err());

        let mut incomplete = lineage.manifest().clone();
        incomplete.case_layers.push(CaseLayer {
            id: "meta".into(),
            manifest: PinnedFile {
                path: "conformance/draft/layers/meta.json".into(),
                sha256: "a".repeat(64),
            },
            tasks: Vec::new(),
            requirements: vec!["TL01-27-1-R001".into()],
        });
        assert!(validate_manifest(&incomplete).is_err());
    }

    #[test]
    fn case_layer_validation_rejects_each_invalid_contract_dimension() {
        let lineage = DraftLineage::load(repository_root(), DRAFT_LINEAGE_PATH).unwrap();
        let descriptor = &lineage.manifest().case_layers[0];
        let valid = &lineage.case_layers()[0];
        let assert_invalid = |manifest: DraftCaseLayerManifest| {
            assert!(validate_case_layer(descriptor, &manifest).is_err());
        };

        let mut invalid = valid.clone();
        invalid.format = "future".into();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.layer = "other".into();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.edition = "0.2".into();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases.clear();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.tasks.push("META-UNDECLARED-001".into());
        invalid.tasks.sort();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].id.clear();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].surface = "Invalid".into();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].requirements.clear();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].evidence.clear();
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].evidence = vec!["documentation:meta".into()];
        assert_invalid(invalid);

        let mut invalid = valid.clone();
        invalid.cases[0].requirements.push("TL01-27-99-R999".into());
        invalid.cases[0].requirements.sort();
        assert_invalid(invalid);

        validate_case_layer(descriptor, valid).unwrap();
    }
}
