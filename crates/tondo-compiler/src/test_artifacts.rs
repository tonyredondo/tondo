//! Content-addressed test attachments.
//!
//! Attachments are staged under an attempt identity and published exactly
//! once as a canonical manifest. Blobs are immutable and keyed only by their
//! SHA-256 digest, so identical data is deduplicated without putting host
//! paths, timestamps or Base64 payloads in the report.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::artifact::sha256;

pub const TEST_ARTIFACT_FORMAT: &str = "tondo-test-artifacts-0.1/1";
pub const ARTIFACT_OBJECT_ALGORITHM: &str = "sha256-v1";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactLimits {
    max_bytes: u64,
    max_items: u32,
}

impl ArtifactLimits {
    pub const fn new(max_bytes: u64, max_items: u32) -> Self {
        Self {
            max_bytes,
            max_items,
        }
    }

    pub const fn max_bytes(self) -> u64 {
        self.max_bytes
    }

    pub const fn max_items(self) -> u32 {
        self.max_items
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub sha256: String,
    pub object: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifest {
    pub format: String,
    pub attempt: String,
    pub algorithm: String,
    pub total_bytes: u64,
    pub artifacts: Vec<ArtifactDescriptor>,
}

impl ArtifactManifest {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ArtifactError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| ArtifactError::Serialization(error.to_string()))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ArtifactError> {
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| ArtifactError::Serialization(error.to_string()))?;
        manifest.validate()?;
        if manifest.canonical_bytes()? != bytes {
            return Err(ArtifactError::NonCanonicalManifest);
        }
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), ArtifactError> {
        if self.format != TEST_ARTIFACT_FORMAT || self.algorithm != ARTIFACT_OBJECT_ALGORITHM {
            return Err(ArtifactError::InvalidManifest);
        }
        validate_attempt(&self.attempt)?;
        let mut names = BTreeSet::new();
        let mut total = 0_u64;
        let mut previous = None;
        for descriptor in &self.artifacts {
            validate_name(&descriptor.name)?;
            validate_media_type(&descriptor.media_type)?;
            validate_hash(&descriptor.sha256)?;
            if descriptor.object != object_name(&descriptor.sha256) {
                return Err(ArtifactError::InvalidManifest);
            }
            if !names.insert(descriptor.name.clone()) {
                return Err(ArtifactError::DuplicateName(descriptor.name.clone()));
            }
            if previous.is_some_and(|value: &str| value.as_bytes() >= descriptor.name.as_bytes()) {
                return Err(ArtifactError::NonCanonicalManifest);
            }
            previous = Some(descriptor.name.as_str());
            total = total
                .checked_add(descriptor.size)
                .ok_or(ArtifactError::Limit)?;
        }
        if total != self.total_bytes {
            return Err(ArtifactError::InvalidManifest);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    InvalidAttempt,
    InvalidName(String),
    InvalidMediaType(String),
    InvalidHash(String),
    DuplicateName(String),
    ItemLimit,
    Limit,
    InvalidManifest,
    NonCanonicalManifest,
    ManifestConflict,
    PathEscape,
    Symlink(PathBuf),
    ObjectCollision(String),
    Published,
    NotPublished,
    Serialization(String),
    Io(String),
}

impl ArtifactError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::DuplicateName(_)
            | Self::InvalidName(_)
            | Self::InvalidMediaType(_)
            | Self::InvalidHash(_)
            | Self::ItemLimit
            | Self::Limit
            | Self::ManifestConflict
            | Self::ObjectCollision(_)
            | Self::PathEscape
            | Self::Symlink(_)
            | Self::InvalidAttempt => "P2006",
            Self::InvalidManifest | Self::NonCanonicalManifest | Self::Serialization(_) => "E2101",
            Self::Published | Self::NotPublished | Self::Io(_) => "E2102",
        }
    }
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAttempt => formatter.write_str("artifact attempt identity is invalid"),
            Self::InvalidName(name) => write!(formatter, "artifact name `{name}` is invalid"),
            Self::InvalidMediaType(media) => write!(formatter, "media type `{media}` is invalid"),
            Self::InvalidHash(hash) => write!(formatter, "artifact hash `{hash}` is invalid"),
            Self::DuplicateName(name) => write!(formatter, "artifact `{name}` is duplicated"),
            Self::ItemLimit => formatter.write_str("artifact item limit is exhausted"),
            Self::Limit => formatter.write_str("artifact byte limit is exhausted"),
            Self::InvalidManifest => formatter.write_str("artifact manifest is invalid"),
            Self::NonCanonicalManifest => formatter.write_str("artifact manifest is not canonical"),
            Self::ManifestConflict => formatter.write_str("artifact manifest already exists"),
            Self::PathEscape => formatter.write_str("artifact path escapes the store root"),
            Self::Symlink(path) => write!(
                formatter,
                "artifact path contains a symlink: {}",
                path.display()
            ),
            Self::ObjectCollision(hash) => {
                write!(formatter, "artifact object collides for hash `{hash}`")
            }
            Self::Published => formatter.write_str("artifact attempt is already published"),
            Self::NotPublished => formatter.write_str("artifact manifest has not been published"),
            Self::Serialization(message) => write!(
                formatter,
                "artifact manifest serialization failed: {message}"
            ),
            Self::Io(message) => write!(formatter, "artifact store I/O failed: {message}"),
        }
    }
}

impl Error for ArtifactError {}

#[derive(Debug)]
pub struct ArtifactStore {
    root: PathBuf,
    attempt: String,
    limits: ArtifactLimits,
    staged: BTreeMap<String, ArtifactDescriptor>,
    total_bytes: u64,
    published: bool,
}

impl ArtifactStore {
    pub fn new(
        root: impl Into<PathBuf>,
        attempt: impl Into<String>,
        limits: ArtifactLimits,
    ) -> Result<Self, ArtifactError> {
        let root = root.into();
        let attempt = attempt.into();
        validate_attempt(&attempt)?;
        ensure_store_dirs(&root)?;
        Ok(Self {
            root,
            attempt,
            limits,
            staged: BTreeMap::new(),
            total_bytes: 0,
            published: false,
        })
    }

    pub fn attempt(&self) -> &str {
        &self.attempt
    }

    pub const fn limits(&self) -> ArtifactLimits {
        self.limits
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &ArtifactDescriptor> {
        self.staged.values()
    }

    pub fn attach(
        &mut self,
        name: impl Into<String>,
        media_type: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ArtifactDescriptor, ArtifactError> {
        if self.published {
            return Err(ArtifactError::Published);
        }
        let name = name.into();
        let media_type = media_type.into();
        validate_name(&name)?;
        validate_media_type(&media_type)?;
        if self.staged.contains_key(&name) {
            return Err(ArtifactError::DuplicateName(name));
        }
        if self.staged.len() >= self.limits.max_items as usize {
            return Err(ArtifactError::ItemLimit);
        }
        let bytes = bytes.as_ref();
        let size = bytes.len() as u64;
        let total = self
            .total_bytes
            .checked_add(size)
            .ok_or(ArtifactError::Limit)?;
        if total > self.limits.max_bytes {
            return Err(ArtifactError::Limit);
        }
        let digest = sha256(bytes);
        let descriptor = ArtifactDescriptor {
            name: name.clone(),
            media_type,
            size,
            sha256: digest.clone(),
            object: object_name(&digest),
        };
        self.write_object(&digest, bytes)?;
        self.total_bytes = total;
        self.staged.insert(name, descriptor.clone());
        Ok(descriptor)
    }

    pub fn manifest(&self) -> Result<ArtifactManifest, ArtifactError> {
        if !self.published {
            return Err(ArtifactError::NotPublished);
        }
        let manifest = self.make_manifest();
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn publish(&mut self) -> Result<ArtifactManifest, ArtifactError> {
        if self.published {
            return Err(ArtifactError::Published);
        }
        ensure_store_dirs(&self.root)?;
        let manifest = self.make_manifest();
        let bytes = manifest.canonical_bytes()?;
        let manifest_path = self.manifest_path();
        ensure_safe_path(&manifest_path, true)?;
        if manifest_path.exists() {
            return Err(ArtifactError::ManifestConflict);
        }
        let temp = temp_path(manifest_path.parent().unwrap_or(&self.root), "manifest");
        write_new_file(&temp, &bytes)?;
        fs::rename(&temp, &manifest_path).map_err(io_error)?;
        self.published = true;
        Ok(manifest)
    }

    pub fn canonical_manifest_bytes(&self) -> Result<Vec<u8>, ArtifactError> {
        self.manifest()?.canonical_bytes()
    }

    pub fn orphan_objects(&self) -> Result<Vec<String>, ArtifactError> {
        ensure_store_dirs(&self.root)?;
        let referenced = referenced_objects(&self.root)?;
        let mut orphans = Vec::new();
        let objects = self.root.join("objects");
        for prefix in fs::read_dir(&objects).map_err(io_error)? {
            let prefix = prefix.map_err(io_error)?.path();
            reject_symlink(&prefix)?;
            if !prefix.is_dir() {
                return Err(ArtifactError::PathEscape);
            }
            for entry in fs::read_dir(&prefix).map_err(io_error)? {
                let path = entry.map_err(io_error)?.path();
                reject_symlink(&path)?;
                if !path.is_file() {
                    return Err(ArtifactError::PathEscape);
                }
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .ok_or(ArtifactError::PathEscape)?;
                let object = format!("sha256/{name}");
                if !referenced.contains(&object) {
                    orphans.push(object);
                }
            }
        }
        orphans.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(orphans)
    }

    pub fn reclaim_orphans(&self) -> Result<Vec<String>, ArtifactError> {
        let orphans = self.orphan_objects()?;
        for object in &orphans {
            let hash = object
                .strip_prefix("sha256/")
                .ok_or(ArtifactError::PathEscape)?;
            if hash.len() < 2 {
                return Err(ArtifactError::PathEscape);
            }
            let path = self.root.join("objects").join(&hash[..2]).join(hash);
            reject_symlink(&path)?;
            fs::remove_file(path).map_err(io_error)?;
        }
        Ok(orphans)
    }

    fn make_manifest(&self) -> ArtifactManifest {
        ArtifactManifest {
            format: TEST_ARTIFACT_FORMAT.into(),
            attempt: self.attempt.clone(),
            algorithm: ARTIFACT_OBJECT_ALGORITHM.into(),
            total_bytes: self.total_bytes,
            artifacts: self.staged.values().cloned().collect(),
        }
    }

    fn write_object(&self, digest: &str, bytes: &[u8]) -> Result<(), ArtifactError> {
        ensure_store_dirs(&self.root)?;
        let hash = digest
            .strip_prefix("sha256:")
            .ok_or_else(|| ArtifactError::InvalidHash(digest.into()))?;
        let path = self.root.join("objects").join(&hash[..2]).join(hash);
        ensure_safe_path(&path, true)?;
        if path.exists() {
            reject_symlink(&path)?;
            let existing = fs::read(&path).map_err(io_error)?;
            if existing == bytes {
                return Ok(());
            }
            return Err(ArtifactError::ObjectCollision(digest.into()));
        }
        let parent = path.parent().ok_or(ArtifactError::PathEscape)?;
        fs::create_dir_all(parent).map_err(io_error)?;
        ensure_safe_path(parent, false)?;
        let temp = temp_path(parent, "object");
        write_new_file(&temp, bytes)?;
        fs::rename(&temp, &path).map_err(io_error)?;
        Ok(())
    }

    fn manifest_path(&self) -> PathBuf {
        let digest = sha256(self.attempt.as_bytes());
        let filename = digest.strip_prefix("sha256:").unwrap_or(&digest);
        self.root.join("manifests").join(format!("{filename}.json"))
    }
}

fn object_name(digest: &str) -> String {
    format!(
        "sha256/{}",
        digest.strip_prefix("sha256:").unwrap_or(digest)
    )
}

fn validate_attempt(value: &str) -> Result<(), ArtifactError> {
    if value.trim().is_empty() || value.contains(['\n', '\r', '/', '\\']) {
        Err(ArtifactError::InvalidAttempt)
    } else {
        Ok(())
    }
}

fn validate_name(value: &str) -> Result<(), ArtifactError> {
    if value.is_empty()
        || value.contains(['/', '\\', '\n', '\r'])
        || value.chars().any(char::is_control)
    {
        Err(ArtifactError::InvalidName(value.into()))
    } else {
        Ok(())
    }
}

fn validate_media_type(value: &str) -> Result<(), ArtifactError> {
    let mut parts = value.split('/');
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return Err(ArtifactError::InvalidMediaType(value.into()));
    };
    if parts.next().is_some()
        || major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(media_token)
        || !minor.bytes().all(media_token)
    {
        return Err(ArtifactError::InvalidMediaType(value.into()));
    }
    Ok(())
}

fn media_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'\x60'
                | b'|'
                | b'~'
        )
}

fn validate_hash(value: &str) -> Result<(), ArtifactError> {
    let Some(hash) = value.strip_prefix("sha256:") else {
        return Err(ArtifactError::InvalidHash(value.into()));
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ArtifactError::InvalidHash(value.into()));
    }
    Ok(())
}

fn ensure_store_dirs(root: &Path) -> Result<(), ArtifactError> {
    ensure_safe_path(root, true)?;
    fs::create_dir_all(root).map_err(io_error)?;
    ensure_safe_path(root, false)?;
    let objects = root.join("objects");
    let manifests = root.join("manifests");
    for path in [&objects, &manifests] {
        ensure_safe_path(path, true)?;
        fs::create_dir_all(path).map_err(io_error)?;
        ensure_safe_path(path, false)?;
    }
    Ok(())
}

fn ensure_safe_path(path: &Path, allow_missing: bool) -> Result<(), ArtifactError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err(ArtifactError::PathEscape),
            Component::Normal(value) => current.push(value),
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ArtifactError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), ArtifactError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ArtifactError::Symlink(path.into()))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn temp_path(parent: &Path, kind: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{kind}-{}-{id}.tmp", std::process::id()))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ArtifactError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    Ok(())
}

fn referenced_objects(root: &Path) -> Result<BTreeSet<String>, ArtifactError> {
    let mut referenced = BTreeSet::new();
    let manifests = root.join("manifests");
    for entry in fs::read_dir(manifests).map_err(io_error)? {
        let path = entry.map_err(io_error)?.path();
        reject_symlink(&path)?;
        if !path.is_file() {
            return Err(ArtifactError::PathEscape);
        }
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(io_error)?
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        for artifact in ArtifactManifest::parse(&bytes)?.artifacts {
            referenced.insert(artifact.object);
        }
    }
    Ok(referenced)
}

fn io_error(error: io::Error) -> ArtifactError {
    ArtifactError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        #[cfg(unix)]
        {
            std::fs::canonicalize(std::env::temp_dir()).unwrap()
        }
        #[cfg(not(unix))]
        {
            std::env::temp_dir()
        }
    }

    fn root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        temp_root().join(format!(
            "tondo-artifacts-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn store(label: &str) -> (PathBuf, ArtifactStore) {
        let root = root(label);
        let store = ArtifactStore::new(&root, "attempt-1", ArtifactLimits::new(1_000, 10)).unwrap();
        (root, store)
    }

    #[test]
    fn attachments_are_content_addressed_and_manifest_is_canonical() {
        let (root, mut store) = store("dedupe");
        let first = store.attach("trace", "text/plain", b"same").unwrap();
        let second = store.attach("copy", "text/plain", b"same").unwrap();
        assert_eq!(first.sha256, second.sha256);
        assert_eq!(first.object, object_name(&first.sha256));
        let manifest = store.publish().unwrap();
        let bytes = manifest.canonical_bytes().unwrap();
        assert_eq!(ArtifactManifest::parse(&bytes).unwrap(), manifest);
        assert_eq!(
            manifest
                .artifacts
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["copy", "trace"]
        );
        assert_eq!(store.manifest().unwrap(), manifest);
        assert!(store.canonical_manifest_bytes().unwrap().starts_with(b"{"));
        assert_eq!(store.total_bytes(), 8);
        assert!(
            !bytes
                .windows(root.to_string_lossy().len())
                .any(|window| window == root.to_string_lossy().as_bytes())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn names_media_limits_duplicates_and_lifecycle_are_closed() {
        let (root, mut store) = store("limits");
        assert_eq!(
            store.attach("bad/name", "text/plain", b"x"),
            Err(ArtifactError::InvalidName("bad/name".into()))
        );
        assert_eq!(
            store.attach("bad", "plain", b"x"),
            Err(ArtifactError::InvalidMediaType("plain".into()))
        );
        assert_eq!(store.attach("a", "text/plain", b"x").unwrap().size, 1);
        assert_eq!(
            store.attach("a", "text/plain", b"x"),
            Err(ArtifactError::DuplicateName("a".into()))
        );
        let mut capped = ArtifactStore::new(&root, "other", ArtifactLimits::new(1, 1)).unwrap();
        assert_eq!(
            capped.attach("a", "text/plain", b"xx"),
            Err(ArtifactError::Limit)
        );
        capped.attach("a", "text/plain", b"x").unwrap();
        assert_eq!(
            capped.attach("b", "text/plain", b"x"),
            Err(ArtifactError::ItemLimit)
        );
        let manifest = capped.publish().unwrap();
        assert_eq!(
            capped.attach("c", "text/plain", b"x"),
            Err(ArtifactError::Published)
        );
        assert!(matches!(
            ArtifactStore::new(&root, "bad/name", ArtifactLimits::new(1, 1)),
            Err(ArtifactError::InvalidAttempt)
        ));
        assert_eq!(store.manifest(), Err(ArtifactError::NotPublished));
        fs::remove_dir_all(root).unwrap();
        let _ = manifest;
    }

    #[test]
    fn publish_is_atomic_and_attempt_collision_is_rejected() {
        let (root, mut store) = store("collision");
        store
            .attach("a", "application/octet-stream", b"one")
            .unwrap();
        let first = store.publish().unwrap();
        assert!(
            !store
                .manifest_path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(':')
        );
        assert_eq!(store.publish(), Err(ArtifactError::Published));
        let mut second =
            ArtifactStore::new(&root, "attempt-1", ArtifactLimits::new(10, 2)).unwrap();
        second
            .attach("a", "application/octet-stream", b"one")
            .unwrap();
        assert_eq!(second.publish(), Err(ArtifactError::ManifestConflict));
        assert_eq!(first.total_bytes, 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn orphan_scan_and_reclamation_are_safe_and_deterministic() {
        let (root, mut store) = store("orphans");
        store.attach("live", "text/plain", b"live").unwrap();
        let live = store.publish().unwrap();
        let mut orphan = ArtifactStore::new(&root, "orphan", ArtifactLimits::new(10, 2)).unwrap();
        orphan.attach("orphan", "text/plain", b"orphan").unwrap();
        let orphan_hash = orphan.descriptors().next().unwrap().object.clone();
        let orphans_before = store.orphan_objects().unwrap();
        assert_eq!(orphans_before, std::slice::from_ref(&orphan_hash));
        assert_eq!(
            store.reclaim_orphans().unwrap(),
            std::slice::from_ref(&orphan_hash)
        );
        assert!(store.orphan_objects().unwrap().is_empty());
        assert_eq!(live.artifacts.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_validation_rejects_drift_and_noncanonical_order() {
        let descriptor = ArtifactDescriptor {
            name: "a".into(),
            media_type: "text/plain".into(),
            size: 1,
            sha256: sha256(b"x"),
            object: object_name(&sha256(b"x")),
        };
        let mut manifest = ArtifactManifest {
            format: TEST_ARTIFACT_FORMAT.into(),
            attempt: "attempt".into(),
            algorithm: ARTIFACT_OBJECT_ALGORITHM.into(),
            total_bytes: 1,
            artifacts: vec![descriptor.clone()],
        };
        let bytes = manifest.canonical_bytes().unwrap();
        assert!(ArtifactManifest::parse(&bytes).is_ok());
        manifest.total_bytes = 2;
        assert!(matches!(
            manifest.canonical_bytes(),
            Err(ArtifactError::InvalidManifest)
        ));
        let mut bad = ArtifactManifest {
            format: TEST_ARTIFACT_FORMAT.into(),
            attempt: "attempt".into(),
            algorithm: ARTIFACT_OBJECT_ALGORITHM.into(),
            total_bytes: 2,
            artifacts: vec![
                descriptor.clone(),
                ArtifactDescriptor {
                    name: "a".into(),
                    ..descriptor
                },
            ],
        };
        assert!(matches!(
            bad.canonical_bytes(),
            Err(ArtifactError::DuplicateName(_))
        ));
        bad.artifacts[0].name = "b".into();
        bad.artifacts[1].name = "a".into();
        assert!(matches!(
            bad.canonical_bytes(),
            Err(ArtifactError::NonCanonicalManifest)
        ));
        assert_eq!(ArtifactError::DuplicateName("a".into()).code(), "P2006");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_parent_escape_are_rejected_without_following_them() {
        let root_path = root("symlink");
        fs::create_dir_all(&root_path).unwrap();
        let outside = root("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root_path.join("objects")).unwrap();
        let result = ArtifactStore::new(&root_path, "attempt", ArtifactLimits::new(10, 1));
        assert!(matches!(result, Err(ArtifactError::Symlink(_))));
        assert!(matches!(
            ensure_safe_path(Path::new("../escape"), true),
            Err(ArtifactError::PathEscape)
        ));
        let parent_file = root("io-parent");
        fs::write(&parent_file, b"not a directory").unwrap();
        assert!(matches!(
            ArtifactStore::new(
                parent_file.join("store"),
                "attempt",
                ArtifactLimits::new(10, 1)
            ),
            Err(ArtifactError::Io(_))
        ));
        fs::remove_dir_all(root_path).unwrap();
        fs::remove_dir_all(outside).unwrap();
        fs::remove_file(parent_file).unwrap();
    }

    #[test]
    fn error_display_and_closed_hash_media_grammar_are_exercised() {
        assert!(ArtifactError::Limit.to_string().contains("byte"));
        assert!(validate_hash("sha256:bad").is_err());
        assert!(validate_hash(&format!("sha256:{}", "g".repeat(64))).is_err());
        assert!(validate_hash(&sha256(b"ok")).is_ok());
        assert!(validate_media_type("text/plain;bad").is_err());
        assert!(validate_media_type("text/plain").is_ok());
        assert_eq!(ARTIFACT_OBJECT_ALGORITHM, "sha256-v1");
    }
}
