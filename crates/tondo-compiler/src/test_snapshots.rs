//! Canonical textual snapshots and explicit atomic update stages.
//!
//! Checks are pure reads against an immutable package store. An update stage
//! belongs to one invocation/attempt and can publish only after the caller
//! marks the complete invocation successful.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::artifact::sha256;

pub const TEST_SNAPSHOT_FORMAT: &str = "tondo-snapshot-store-0.1/1";
pub const P2007_MISMATCH: &str = "P2007";
pub const P2008_CONFLICT: &str = "P2008";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotKey {
    pub node_id: String,
    pub name: String,
}

impl SnapshotKey {
    pub fn new(node_id: impl Into<String>, name: impl Into<String>) -> Result<Self, SnapshotError> {
        let key = Self {
            node_id: node_id.into(),
            name: name.into(),
        };
        validate_key(&key)?;
        Ok(key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEntry {
    pub node_id: String,
    pub name: String,
    pub value: String,
}

impl SnapshotEntry {
    fn key(&self) -> SnapshotKey {
        SnapshotKey {
            node_id: self.node_id.clone(),
            name: self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotStore {
    pub format: String,
    pub package: String,
    pub entries: Vec<SnapshotEntry>,
}

impl SnapshotStore {
    pub fn empty(package: impl Into<String>) -> Result<Self, SnapshotError> {
        let store = Self {
            format: TEST_SNAPSHOT_FORMAT.into(),
            package: package.into(),
            entries: Vec::new(),
        };
        store.validate()?;
        Ok(store)
    }

    pub fn from_entries(
        package: impl Into<String>,
        entries: impl IntoIterator<Item = SnapshotEntry>,
    ) -> Result<Self, SnapshotError> {
        let mut store = Self {
            format: TEST_SNAPSHOT_FORMAT.into(),
            package: package.into(),
            entries: entries.into_iter().collect(),
        };
        store.normalize()?;
        store.validate()?;
        Ok(store)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SnapshotError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| SnapshotError::Serialization(error.to_string()))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, SnapshotError> {
        let store: Self = serde_json::from_slice(bytes)
            .map_err(|error| SnapshotError::Serialization(error.to_string()))?;
        store.validate()?;
        if store.canonical_bytes()? != bytes {
            return Err(SnapshotError::NonCanonical);
        }
        Ok(store)
    }

    pub fn content_hash(&self) -> Result<String, SnapshotError> {
        Ok(sha256(&self.canonical_bytes()?))
    }

    pub fn entry(&self, key: &SnapshotKey) -> Option<&SnapshotEntry> {
        self.entries.iter().find(|entry| entry.key() == *key)
    }

    pub fn entries(&self) -> &[SnapshotEntry] {
        &self.entries
    }

    pub fn check(
        &self,
        node_id: impl Into<String>,
        name: impl Into<String>,
        actual: &str,
        diff_limit: usize,
    ) -> Result<SnapshotOutcome, SnapshotError> {
        let key = SnapshotKey::new(node_id, name)?;
        let actual_sha256 = sha256(actual.as_bytes());
        let Some(expected) = self.entry(&key) else {
            return Ok(SnapshotOutcome::Missing { actual_sha256 });
        };
        let expected_sha256 = sha256(expected.value.as_bytes());
        if expected.value == actual {
            Ok(SnapshotOutcome::Matched {
                expected_sha256,
                actual_sha256,
            })
        } else {
            Ok(SnapshotOutcome::Mismatched {
                expected_sha256,
                actual_sha256,
                diff: bounded_diff(&expected.value, actual, diff_limit),
            })
        }
    }

    pub fn load(root: &Path, relative: &Path) -> Result<Self, SnapshotError> {
        let path = safe_join(root, relative)?;
        reject_symlink(&path)?;
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(io_error)?
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Self::parse(&bytes)
    }

    pub fn publish_atomic(&self, root: &Path, relative: &Path) -> Result<(), SnapshotError> {
        let path = safe_join(root, relative)?;
        ensure_parent(&path)?;
        if path.exists() {
            reject_symlink(&path)?;
        }
        let bytes = self.canonical_bytes()?;
        let temp = temp_path(path.parent().unwrap_or(root));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        fs::rename(temp, path).map_err(io_error)?;
        Ok(())
    }

    fn normalize(&mut self) -> Result<(), SnapshotError> {
        self.entries
            .sort_by(|left, right| key_cmp(&left.key(), &right.key()));
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            validate_entry(entry)?;
            if !seen.insert(entry.key()) {
                return Err(SnapshotError::Duplicate(
                    entry.node_id.clone(),
                    entry.name.clone(),
                ));
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), SnapshotError> {
        if self.format != TEST_SNAPSHOT_FORMAT || self.package.trim().is_empty() {
            return Err(SnapshotError::InvalidStore);
        }
        let mut previous = None;
        let mut seen = BTreeSet::new();
        for entry in &self.entries {
            validate_entry(entry)?;
            if !seen.insert(entry.key()) {
                return Err(SnapshotError::Duplicate(
                    entry.node_id.clone(),
                    entry.name.clone(),
                ));
            }
            if previous.as_ref().is_some_and(|key: &SnapshotKey| {
                key_cmp(key, &entry.key()) != std::cmp::Ordering::Less
            }) {
                return Err(SnapshotError::NonCanonical);
            }
            previous = Some(entry.key());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotOutcome {
    Matched {
        expected_sha256: String,
        actual_sha256: String,
    },
    Missing {
        actual_sha256: String,
    },
    Mismatched {
        expected_sha256: String,
        actual_sha256: String,
        diff: String,
    },
}

impl SnapshotOutcome {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Matched { .. } => "OK",
            Self::Missing { .. } | Self::Mismatched { .. } => P2007_MISMATCH,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotUpdateKind {
    Created,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotUpdate {
    pub key: SnapshotKey,
    pub kind: SnapshotUpdateKind,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPolicy {
    jobs: u32,
    canonical_order: bool,
    shard: bool,
    retry: bool,
    repeat: bool,
    allow_flaky: bool,
}

impl SnapshotPolicy {
    pub const fn new(
        jobs: u32,
        canonical_order: bool,
        shard: bool,
        retry: bool,
        repeat: bool,
        allow_flaky: bool,
    ) -> Self {
        Self {
            jobs,
            canonical_order,
            shard,
            retry,
            repeat,
            allow_flaky,
        }
    }

    pub const fn validate(self) -> Result<(), SnapshotError> {
        if self.jobs != 1
            || !self.canonical_order
            || self.shard
            || self.retry
            || self.repeat
            || self.allow_flaky
        {
            Err(SnapshotError::Policy)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotUpdateStage {
    original: SnapshotStore,
    updates: BTreeMap<SnapshotKey, SnapshotUpdate>,
    complete_success: bool,
    published: bool,
}

impl SnapshotUpdateStage {
    pub fn new(original: SnapshotStore, policy: SnapshotPolicy) -> Result<Self, SnapshotError> {
        policy.validate()?;
        Ok(Self {
            original,
            updates: BTreeMap::new(),
            complete_success: false,
            published: false,
        })
    }

    pub fn stage(
        &mut self,
        node_id: impl Into<String>,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<SnapshotUpdate, SnapshotError> {
        if self.published {
            return Err(SnapshotError::Published);
        }
        let key = SnapshotKey::new(node_id, name)?;
        if self.updates.contains_key(&key) {
            return Err(SnapshotError::Conflict(key.node_id, key.name));
        }
        let kind = if self.original.entry(&key).is_some() {
            SnapshotUpdateKind::Updated
        } else {
            SnapshotUpdateKind::Created
        };
        let update = SnapshotUpdate {
            key: key.clone(),
            kind,
            value: value.into(),
        };
        self.updates.insert(key, update.clone());
        Ok(update)
    }

    pub fn updates(&self) -> impl Iterator<Item = &SnapshotUpdate> {
        self.updates.values()
    }

    pub fn mark_success(&mut self) {
        self.complete_success = true;
    }

    pub fn staged_store(&self) -> Result<SnapshotStore, SnapshotError> {
        if !self.complete_success {
            return Err(SnapshotError::Incomplete);
        }
        let mut entries = self.original.entries.clone();
        for update in self.updates.values() {
            if let Some(entry) = entries.iter_mut().find(|entry| entry.key() == update.key) {
                entry.value = update.value.clone();
            } else {
                entries.push(SnapshotEntry {
                    node_id: update.key.node_id.clone(),
                    name: update.key.name.clone(),
                    value: update.value.clone(),
                });
            }
        }
        SnapshotStore::from_entries(self.original.package.clone(), entries)
    }

    pub fn publish(
        &mut self,
        root: &Path,
        relative: &Path,
    ) -> Result<SnapshotStore, SnapshotError> {
        if self.published {
            return Err(SnapshotError::Published);
        }
        let store = self.staged_store()?;
        store.publish_atomic(root, relative)?;
        self.published = true;
        Ok(store)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidKey,
    InvalidStore,
    Duplicate(String, String),
    Conflict(String, String),
    NonCanonical,
    Policy,
    Incomplete,
    Published,
    PathEscape,
    Symlink(PathBuf),
    Io(String),
    Serialization(String),
}

impl SnapshotError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Duplicate(_, _)
            | Self::Conflict(_, _)
            | Self::Policy
            | Self::PathEscape
            | Self::Symlink(_) => P2008_CONFLICT,
            Self::InvalidKey | Self::InvalidStore | Self::NonCanonical | Self::Incomplete => {
                "E2200"
            }
            Self::Published | Self::Io(_) | Self::Serialization(_) => "E2201",
        }
    }
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => formatter.write_str("snapshot key is invalid"),
            Self::InvalidStore => formatter.write_str("snapshot store is invalid"),
            Self::Duplicate(node, name) => write!(formatter, "snapshot duplicate: {node}/{name}"),
            Self::Conflict(node, name) => write!(formatter, "snapshot conflict: {node}/{name}"),
            Self::NonCanonical => formatter.write_str("snapshot store is not canonical"),
            Self::Policy => formatter.write_str("snapshot update requires one canonical job"),
            Self::Incomplete => formatter.write_str("snapshot update invocation did not succeed"),
            Self::Published => formatter.write_str("snapshot update was already published"),
            Self::PathEscape => formatter.write_str("snapshot path escapes the package root"),
            Self::Symlink(path) => write!(
                formatter,
                "snapshot path contains a symlink: {}",
                path.display()
            ),
            Self::Io(message) => write!(formatter, "snapshot store I/O failed: {message}"),
            Self::Serialization(message) => {
                write!(formatter, "snapshot serialization failed: {message}")
            }
        }
    }
}

impl Error for SnapshotError {}

fn validate_key(key: &SnapshotKey) -> Result<(), SnapshotError> {
    if key.node_id.trim().is_empty()
        || key.name.trim().is_empty()
        || key.node_id.contains(['\n', '\r', '/', '\\'])
        || key.name.contains(['\n', '\r', '/', '\\'])
        || key.node_id.chars().any(char::is_control)
        || key.name.chars().any(char::is_control)
    {
        Err(SnapshotError::InvalidKey)
    } else {
        Ok(())
    }
}

fn validate_entry(entry: &SnapshotEntry) -> Result<(), SnapshotError> {
    validate_key(&entry.key())
}

fn key_cmp(left: &SnapshotKey, right: &SnapshotKey) -> std::cmp::Ordering {
    left.node_id
        .as_bytes()
        .cmp(right.node_id.as_bytes())
        .then_with(|| left.name.as_bytes().cmp(right.name.as_bytes()))
}

fn bounded_diff(expected: &str, actual: &str, limit: usize) -> String {
    let limit = limit.max(16);
    let expected_bytes = expected.as_bytes();
    let actual_bytes = actual.as_bytes();
    let prefix = expected_bytes
        .iter()
        .zip(actual_bytes)
        .take_while(|(left, right)| left == right)
        .count();
    let mut diff = format!(
        "at byte {prefix}: expected {:?}, actual {:?}",
        expected.get(prefix..).unwrap_or_default(),
        actual.get(prefix..).unwrap_or_default()
    );
    if diff.len() > limit {
        let keep = limit.saturating_sub(3);
        let mut boundary = keep.min(diff.len());
        while boundary > 0 && !diff.is_char_boundary(boundary) {
            boundary -= 1;
        }
        diff.truncate(boundary);
        diff.push_str("...");
    }
    diff
}

fn safe_join(root: &Path, relative: &Path) -> Result<PathBuf, SnapshotError> {
    if relative.is_absolute() {
        return Err(SnapshotError::PathEscape);
    }
    let mut path = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(value) => path.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SnapshotError::PathEscape);
            }
        }
    }
    ensure_safe_path(&path, true)?;
    Ok(path)
}

fn ensure_parent(path: &Path) -> Result<(), SnapshotError> {
    let parent = path.parent().ok_or(SnapshotError::PathEscape)?;
    ensure_safe_path(parent, true)?;
    fs::create_dir_all(parent).map_err(io_error)?;
    ensure_safe_path(parent, false)
}

fn ensure_safe_path(path: &Path, allow_missing: bool) -> Result<(), SnapshotError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        // Windows drive prefixes are not paths on their own (`C:` is a
        // drive-relative spelling), and asking the filesystem for metadata
        // on one returns ERROR_INVALID_FUNCTION.  Keep the prefix while
        // deferring the check until the root or first normal component has
        // been appended (`C:\\` or `C:\\...`).
        let check_component = !matches!(component, Component::Prefix(_));
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => return Err(SnapshotError::PathEscape),
            Component::Normal(value) => current.push(value),
        }
        if !check_component {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SnapshotError::Symlink(current));
            }
            Ok(_) => {}
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), SnapshotError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(SnapshotError::Symlink(path.into()))
        }
        Ok(_) => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn temp_path(parent: &Path) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".snapshot-{}-{id}.tmp", std::process::id()))
}

fn io_error(error: io::Error) -> SnapshotError {
    SnapshotError::Io(error.to_string())
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
            "tondo-snapshots-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn entry(node: &str, name: &str, value: &str) -> SnapshotEntry {
        SnapshotEntry {
            node_id: node.into(),
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn stores_are_canonical_sorted_and_hashed_without_losing_stale_entries() {
        let store = SnapshotStore::from_entries(
            "pkg",
            [
                entry("z", "b", "2"),
                entry("a", "a", "1"),
                entry("z", "a", "3"),
            ],
        )
        .unwrap();
        assert_eq!(
            store
                .entries()
                .iter()
                .map(|entry| format!("{}/{}", entry.node_id, entry.name))
                .collect::<Vec<_>>(),
            ["a/a", "z/a", "z/b"]
        );
        let bytes = store.canonical_bytes().unwrap();
        assert_eq!(SnapshotStore::parse(&bytes).unwrap(), store);
        assert_eq!(store.content_hash().unwrap(), sha256(&bytes));
        assert_eq!(
            store
                .entry(&SnapshotKey::new("z", "a").unwrap())
                .unwrap()
                .value,
            "3"
        );
        assert!(SnapshotStore::empty("pkg").unwrap().entries().is_empty());
        assert_eq!(TEST_SNAPSHOT_FORMAT, "tondo-snapshot-store-0.1/1");
    }

    #[test]
    fn checks_exact_strings_and_returns_bounded_match_missing_or_diff() {
        let store = SnapshotStore::from_entries(
            "pkg",
            [
                entry("node", "same", "exact"),
                entry("node", "bad", "expected"),
            ],
        )
        .unwrap();
        assert!(matches!(
            store.check("node", "same", "exact", 32).unwrap(),
            SnapshotOutcome::Matched { .. }
        ));
        assert!(matches!(
            store.check("node", "missing", "new", 32).unwrap(),
            SnapshotOutcome::Missing { .. }
        ));
        let mismatch = store.check("node", "bad", "actual value", 18).unwrap();
        assert_eq!(mismatch.code(), P2007_MISMATCH);
        if let SnapshotOutcome::Mismatched { diff, .. } = mismatch {
            assert!(diff.len() <= 18);
            assert!(diff.contains("byte"));
        } else {
            panic!("expected mismatch");
        }
    }

    #[test]
    fn update_stage_is_explicit_preserves_stale_values_and_publishes_once() {
        let original = SnapshotStore::from_entries(
            "pkg",
            [entry("old", "stale", "keep"), entry("node", "name", "old")],
        )
        .unwrap();
        let policy = SnapshotPolicy::new(1, true, false, false, false, false);
        let mut stage = SnapshotUpdateStage::new(original, policy).unwrap();
        let created = stage.stage("node", "new", "created").unwrap();
        assert_eq!(created.kind, SnapshotUpdateKind::Created);
        let updated = stage.stage("node", "name", "new").unwrap();
        assert_eq!(updated.kind, SnapshotUpdateKind::Updated);
        assert_eq!(
            stage.stage("node", "name", "duplicate"),
            Err(SnapshotError::Conflict("node".into(), "name".into()))
        );
        assert_eq!(stage.staged_store(), Err(SnapshotError::Incomplete));
        stage.mark_success();
        let staged = stage.staged_store().unwrap();
        assert_eq!(
            staged
                .entries()
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["name", "new", "stale"]
        );
        let root_path = root("update");
        let relative = Path::new("pkg/snapshots.json");
        let published = stage.publish(&root_path, relative).unwrap();
        assert_eq!(
            SnapshotStore::load(&root_path, relative).unwrap(),
            published
        );
        assert_eq!(
            stage.publish(&root_path, relative),
            Err(SnapshotError::Published)
        );
        fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn policy_rejects_parallel_or_nondeterministic_update_inputs() {
        let policies = [
            SnapshotPolicy::new(2, true, false, false, false, false),
            SnapshotPolicy::new(1, false, false, false, false, false),
            SnapshotPolicy::new(1, true, true, false, false, false),
            SnapshotPolicy::new(1, true, false, true, false, false),
            SnapshotPolicy::new(1, true, false, false, true, false),
            SnapshotPolicy::new(1, true, false, false, false, true),
        ];
        for policy in policies {
            assert_eq!(policy.validate(), Err(SnapshotError::Policy));
        }
        assert_eq!(
            SnapshotPolicy::new(1, true, false, false, false, false).validate(),
            Ok(())
        );
    }

    #[test]
    fn malformed_store_keys_duplicates_and_noncanonical_bytes_are_rejected() {
        assert!(matches!(
            SnapshotKey::new("", "name"),
            Err(SnapshotError::InvalidKey)
        ));
        assert!(matches!(
            SnapshotKey::new("node", "bad/name"),
            Err(SnapshotError::InvalidKey)
        ));
        assert!(matches!(
            SnapshotStore::from_entries(
                "pkg",
                [entry("node", "same", "a"), entry("node", "same", "b")]
            ),
            Err(SnapshotError::Duplicate(_, _))
        ));
        let bad_json = br#"{"package":"pkg","format":"tondo-snapshot-store-0.1/1","entries":[]}"#;
        assert!(matches!(
            SnapshotStore::parse(bad_json),
            Err(SnapshotError::NonCanonical)
        ));
        assert!(matches!(
            SnapshotStore::empty(""),
            Err(SnapshotError::InvalidStore)
        ));
        assert_eq!(
            SnapshotError::Conflict("n".into(), "x".into()).code(),
            P2008_CONFLICT
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_paths_reject_symlinks_and_escape_without_following_them() {
        let root_path = root("symlink");
        fs::create_dir_all(&root_path).unwrap();
        let outside = root("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root_path.join("pkg")).unwrap();
        let store = SnapshotStore::empty("pkg").unwrap();
        assert!(matches!(
            store.publish_atomic(&root_path, Path::new("pkg/store.json")),
            Err(SnapshotError::Symlink(_))
        ));
        assert!(matches!(
            safe_join(&root_path, Path::new("../escape")),
            Err(SnapshotError::PathEscape)
        ));
        fs::remove_dir_all(root_path).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn bounded_diff_and_error_display_cover_edges() {
        assert!(bounded_diff("same", "same", 0).len() <= 16);
        let diff = bounded_diff("a".repeat(100).as_str(), "b".repeat(100).as_str(), 16);
        assert!(diff.len() <= 16);
        assert!(SnapshotError::Incomplete.to_string().contains("succeed"));
        assert!(SnapshotError::PathEscape.to_string().contains("escapes"));
        assert_eq!(
            SnapshotOutcome::Matched {
                expected_sha256: "a".into(),
                actual_sha256: "a".into()
            }
            .code(),
            "OK"
        );
        assert_eq!(
            SnapshotOutcome::Missing {
                actual_sha256: "a".into()
            }
            .code(),
            P2007_MISMATCH
        );
    }
}
