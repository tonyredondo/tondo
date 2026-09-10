//! Coordinator-owned temporary roots for isolated test workers.
//!
//! Physical names are transport data, never test/report identity. The caller
//! must reap its worker and call `cleanup` before publishing any results.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

pub const TEMPORARY_ROOT_MODEL: &str = "worker-root-1048576-67108864-64-admitted-write/1";
pub const MAX_TEMP_ENTRIES: u64 = 1_048_576;
pub const MAX_TEMP_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_TEMP_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TempError {
    InvalidPrefix,
    Unavailable,
    PermissionDenied,
    LimitExceeded,
    IoError,
}

impl TempError {
    pub(crate) fn from_io(error: &io::Error) -> Self {
        if error.kind() == io::ErrorKind::PermissionDenied {
            Self::PermissionDenied
        } else if error
            .get_ref()
            .is_some_and(|cause| cause.is::<TreeLimitExceeded>())
        {
            Self::LimitExceeded
        } else {
            Self::IoError
        }
    }
}

/// Preserve a typed limit cause through the shared std.fs admission path.
/// Diagnostic wording must never determine a public error variant.
#[derive(Debug)]
struct TreeLimitExceeded(&'static str);

impl std::fmt::Display for TreeLimitExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for TreeLimitExceeded {}

fn limit_exceeded(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, TreeLimitExceeded(message))
}

#[derive(Debug)]
pub struct TemporaryRoot {
    path: PathBuf,
    cleanup_attempted: bool,
}

impl TemporaryRoot {
    /// Creates a fresh root under the explicitly selected project's target
    /// directory. Existing roots and symlinked parents are never adopted.
    pub fn create(project: &Path) -> io::Result<Self> {
        if !project.is_absolute() {
            return Err(invalid("temporary project root must be absolute"));
        }
        verify_directory(project)?;
        let target = project.join("target");
        create_parent(&target)?;
        let parent = target.join(".tondo-test-root");
        create_parent(&parent)?;
        for _ in 0..64 {
            let nonce = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("worker-{}-{nonce}", std::process::id()));
            match create_private_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        path,
                        cleanup_attempted: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary root nonce attempts exhausted",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Do not traverse a root whose worker or descendants may still be live.
    /// The coordinator must report the retained path and fail publication.
    pub fn retain_after_failed_isolation(&mut self) {
        self.cleanup_attempted = true;
    }

    /// Revoke only this worker's root. A failed cleanup must prevent result
    /// publication; the path is retained so the caller can report the failure.
    pub fn cleanup(&mut self) -> io::Result<()> {
        self.cleanup_attempted = true;
        match fs::symlink_metadata(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => cleanup_tree(&self.path, TreeLimits::DEFAULT),
        }
    }
}

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        // Unwinding has no success result to publish. Normal paths always
        // observe cleanup explicitly, including errors returned by spawning.
        if !self.cleanup_attempted {
            let _ = self.cleanup();
        }
    }
}

pub(crate) fn verify_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid(
            "temporary root must be a directory without symlinks",
        ));
    }
    Ok(())
}

fn create_parent(path: &Path) -> io::Result<()> {
    match create_private_directory(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => verify_directory(path),
        Err(error) => Err(error),
    }
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir(path)
    }
}

/// Clean one helper-owned immediate child, never a caller-supplied arbitrary
/// path. The same bounded traversal is used by coordinator cleanup.
pub(crate) fn cleanup_directory(root: &Path, directory: &Path) -> io::Result<()> {
    verify_directory(root)?;
    if directory.parent() != Some(root) {
        return Err(invalid("temporary directory is outside its worker root"));
    }
    cleanup_tree(directory, TreeLimits::DEFAULT)
}

#[derive(Clone, Copy)]
struct TreeLimits {
    entries: u64,
    bytes: u64,
    depth: usize,
}

impl TreeLimits {
    const DEFAULT: Self = Self {
        entries: MAX_TEMP_ENTRIES,
        bytes: MAX_TEMP_BYTES,
        depth: MAX_TEMP_DEPTH,
    };
}

#[derive(Default)]
struct TreeUsage {
    entries: u64,
    bytes: u64,
    depth: usize,
}

/// Predicted live filesystem growth. This is separate from VM heap memory.
/// Ordinary writes supply the resulting file length; atomic writes must also
/// admit the temporary file while the destination still exists.
pub(crate) enum Mutation<'a> {
    Write { length: u64 },
    FileGrowth { bytes: u64 },
    AtomicWrite { length: u64 },
    CreateDirectory,
    Remove,
    Rename { source: &'a Path },
}

pub(crate) fn check_mutation(root: &Path, target: &Path, mutation: Mutation<'_>) -> io::Result<()> {
    check_mutation_with_limits(root, target, mutation, TreeLimits::DEFAULT)
}

pub(crate) fn imported_bytes(source: &Path) -> io::Result<u64> {
    let mut usage = TreeUsage::default();
    visit_tree(source, 0, &mut usage, TreeLimits::DEFAULT, false)?;
    Ok(usage.bytes)
}

fn check_mutation_with_limits(
    root: &Path,
    target: &Path,
    mutation: Mutation<'_>,
    limits: TreeLimits,
) -> io::Result<()> {
    // The provider accounts paths in its explicit root. General filesystem
    // paths and external process effects retain the std.fs/process contract.
    if !target.starts_with(root) {
        return Ok(());
    }
    verify_directory(root)?;
    let mut usage = TreeUsage::default();
    visit_tree(root, 0, &mut usage, limits, false)?;
    let canonical_root = fs::canonicalize(root)?;
    let (target, missing) = resolve_candidate(target, limits.depth)?;
    let Ok(relative) = target.strip_prefix(&canonical_root) else {
        // A Path explicitly moved outside the root is a general std.fs path.
        return Ok(());
    };
    let depth = relative.components().count();
    if depth > limits.depth {
        return Err(limit_exceeded("temporary tree depth limit exceeded"));
    }
    let old_length = match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => 0,
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    let (bytes, entries, subtree_depth) = match mutation {
        Mutation::Write { length } => (length.saturating_sub(old_length), missing, 0),
        Mutation::FileGrowth { bytes } => (bytes, 0, 0),
        Mutation::AtomicWrite { length } => (length, missing.max(1), 0),
        Mutation::CreateDirectory => (0, missing, 0),
        Mutation::Remove => (0, 0, 0),
        Mutation::Rename { source } => {
            if fs::symlink_metadata(source)?.file_type().is_symlink() {
                return Err(invalid("temporary tree contains a symlink"));
            }
            let source = fs::canonicalize(source)?;
            let mut source_usage = TreeUsage::default();
            visit_tree(&source, 0, &mut source_usage, limits, false)?;
            if source.starts_with(&canonical_root) {
                (0, 0, source_usage.depth)
            } else {
                let mut target_usage = TreeUsage::default();
                if target.exists() {
                    visit_tree(&target, 0, &mut target_usage, limits, false)?;
                }
                (
                    source_usage.bytes.saturating_sub(target_usage.bytes),
                    source_usage.entries.saturating_sub(target_usage.entries),
                    source_usage.depth,
                )
            }
        }
    };
    if bytes > limits.bytes.saturating_sub(usage.bytes) {
        return Err(limit_exceeded("temporary tree byte limit exceeded"));
    }
    if entries > limits.entries.saturating_sub(usage.entries) {
        return Err(limit_exceeded("temporary tree entry limit exceeded"));
    }
    if subtree_depth > limits.depth.saturating_sub(depth) {
        return Err(limit_exceeded("temporary tree depth limit exceeded"));
    }
    Ok(())
}

/// Resolve existing ancestors using filesystem semantics, preserving Path's
/// lexical API. Only missing normal components can be supplied by creation.
fn resolve_candidate(path: &Path, max_missing: usize) -> io::Result<(PathBuf, u64)> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok((resolved, missing.len() as u64));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if missing.len() >= max_missing {
                    return Err(limit_exceeded("temporary tree depth limit exceeded"));
                }
                missing.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| invalid("temporary path has no existing ancestor"))?,
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| invalid("temporary path has no existing ancestor"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn cleanup_tree(path: &Path, limits: TreeLimits) -> io::Result<()> {
    verify_directory(path)?;
    // Validate the complete tree before its first deletion. Repeat the same
    // checks at each deletion step; an earlier scan does not waive limits or
    // authorize following a symlink observed during deletion. This is not an
    // OS filesystem sandbox against concurrent external path replacement.
    visit_tree(path, 0, &mut TreeUsage::default(), limits, false)?;
    visit_tree(path, 0, &mut TreeUsage::default(), limits, true)
}

fn visit_tree(
    path: &Path,
    depth: usize,
    usage: &mut TreeUsage,
    limits: TreeLimits,
    remove: bool,
) -> io::Result<()> {
    if depth > limits.depth {
        return Err(limit_exceeded("temporary tree depth limit exceeded"));
    }
    usage.depth = usage.depth.max(depth);
    usage.entries = usage.entries.saturating_add(1);
    if usage.entries > limits.entries {
        return Err(limit_exceeded("temporary tree entry limit exceeded"));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("temporary tree contains a symlink"));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            visit_tree(&entry?.path(), depth + 1, usage, limits, remove)?;
        }
        if remove {
            fs::remove_dir(path)?;
        }
    } else {
        if !metadata.is_file() {
            return Err(invalid("temporary tree contains a non-regular file"));
        }
        usage.bytes = usage.bytes.saturating_add(metadata.len());
        if usage.bytes > limits.bytes {
            return Err(limit_exceeded("temporary tree byte limit exceeded"));
        }
        if remove {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> TemporaryRoot {
        TemporaryRoot::create(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[test]
    fn public_error_causes_use_typed_limits_and_io_kinds_instead_of_messages() {
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let cause = TempError::from_io(&io::Error::new(kind, "arbitrary provider text"));
            assert_eq!(
                cause,
                if kind == io::ErrorKind::PermissionDenied {
                    TempError::PermissionDenied
                } else {
                    TempError::IoError
                }
            );
        }
        assert_eq!(
            TempError::from_io(&invalid("temporary tree byte limit exceeded")),
            TempError::IoError
        );
        assert_eq!(
            TempError::from_io(&limit_exceeded("different diagnostic text")),
            TempError::LimitExceeded
        );
    }

    #[test]
    fn private_roots_are_distinct_and_cleanup_preserves_siblings() {
        let mut first = root();
        let mut second = root();
        assert_ne!(first.path(), second.path());
        fs::create_dir(first.path().join("nested")).unwrap();
        fs::write(first.path().join("nested/payload"), b"data").unwrap();
        fs::write(second.path().join("keep"), b"sibling").unwrap();
        first.cleanup().unwrap();
        first.cleanup().unwrap();
        assert!(!first.path().exists());
        assert_eq!(fs::read(second.path().join("keep")).unwrap(), b"sibling");
        second.cleanup().unwrap();
        assert!(!second.path().exists());
    }

    #[test]
    fn deletion_checks_byte_entry_and_depth_limits_before_modifying_the_tree() {
        let mut root = root();
        fs::create_dir(root.path().join("child")).unwrap();
        fs::write(root.path().join("child/payload"), b"1234").unwrap();
        for (entries, bytes, depth, expected) in
            [(2, 4, 2, "entry"), (3, 3, 2, "byte"), (3, 4, 1, "depth")]
        {
            let error = cleanup_tree(
                root.path(),
                TreeLimits {
                    entries,
                    bytes,
                    depth,
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(TempError::from_io(&error), TempError::LimitExceeded);
            assert_eq!(
                fs::read(root.path().join("child/payload")).unwrap(),
                b"1234"
            );
        }
        cleanup_tree(
            root.path(),
            TreeLimits {
                entries: 3,
                bytes: 4,
                depth: 2,
            },
        )
        .unwrap();
        root.cleanup().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_root_escape_are_rejected_without_touching_external_contents() {
        use std::os::unix::fs::symlink;
        let mut root = root();
        let mut sibling = self::root();
        fs::write(sibling.path().join("keep"), b"sibling").unwrap();
        symlink(sibling.path(), root.path().join("link")).unwrap();
        assert!(root.cleanup().unwrap_err().to_string().contains("symlink"));
        assert!(cleanup_directory(root.path(), sibling.path()).is_err());
        assert_eq!(fs::read(sibling.path().join("keep")).unwrap(), b"sibling");
        fs::remove_file(root.path().join("link")).unwrap();
        root.cleanup().unwrap();
        sibling.cleanup().unwrap();
    }

    #[test]
    fn mutation_admission_distinguishes_overwrite_atomic_peak_and_structural_growth() {
        let mut root = root();
        let file = root.path().join("payload");
        fs::write(&file, b"1234").unwrap();
        let limits = TreeLimits {
            entries: 2,
            bytes: 4,
            depth: 1,
        };
        let check = |target: &Path, mutation| {
            check_mutation_with_limits(root.path(), target, mutation, limits)
        };
        check(&file, Mutation::Write { length: 4 }).unwrap();
        check(&file, Mutation::FileGrowth { bytes: 0 }).unwrap();
        check(&file, Mutation::Remove).unwrap();
        for mutation in [
            Mutation::Write { length: 5 },
            Mutation::AtomicWrite { length: 1 },
            Mutation::FileGrowth { bytes: 1 },
        ] {
            assert!(
                check(&file, mutation)
                    .unwrap_err()
                    .to_string()
                    .contains("byte")
            );
        }
        assert!(
            check(&root.path().join("new"), Mutation::CreateDirectory)
                .unwrap_err()
                .to_string()
                .contains("entry")
        );
        assert!(
            check(&root.path().join("a/b"), Mutation::CreateDirectory)
                .unwrap_err()
                .to_string()
                .contains("depth")
        );
        assert_eq!(fs::read(&file).unwrap(), b"1234");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        root.cleanup().unwrap();
    }

    #[test]
    fn rename_checks_imported_contents_and_the_resulting_subtree_depth() {
        let mut root = root();
        let mut source = self::root();
        let incoming = source.path().join("incoming");
        let target = root.path().join("target");
        fs::write(&incoming, b"12345").unwrap();
        fs::write(&target, b"1234").unwrap();
        let limits = TreeLimits {
            entries: 2,
            bytes: 4,
            depth: 1,
        };
        assert!(
            check_mutation_with_limits(
                root.path(),
                &target,
                Mutation::Rename { source: &incoming },
                limits
            )
            .unwrap_err()
            .to_string()
            .contains("byte")
        );
        fs::write(&incoming, b"same").unwrap();
        check_mutation_with_limits(
            root.path(),
            &target,
            Mutation::Rename { source: &incoming },
            limits,
        )
        .unwrap();
        fs::rename(&incoming, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"same");
        fs::create_dir(root.path().join("left")).unwrap();
        fs::create_dir(root.path().join("right")).unwrap();
        fs::write(root.path().join("left/item"), b"item").unwrap();
        let left = root.path().join("left");
        let moved = root.path().join("right/moved");
        let limits = TreeLimits {
            entries: 8,
            bytes: 8,
            depth: 2,
        };
        assert!(
            check_mutation_with_limits(
                root.path(),
                &moved,
                Mutation::Rename { source: &left },
                limits
            )
            .unwrap_err()
            .to_string()
            .contains("depth")
        );
        check_mutation_with_limits(
            root.path(),
            &moved,
            Mutation::Rename { source: &left },
            TreeLimits { depth: 3, ..limits },
        )
        .unwrap();
        fs::rename(&left, &moved).unwrap();
        assert_eq!(fs::read(moved.join("item")).unwrap(), b"item");
        root.cleanup().unwrap();
        source.cleanup().unwrap();
    }

    #[test]
    fn unwinding_scope_cleanup_uses_only_the_owned_root() {
        let path = {
            let root = root();
            fs::write(root.path().join("payload"), b"data").unwrap();
            root.path().to_owned()
        };
        assert!(!path.exists());
        assert!(TemporaryRoot::create(Path::new(".")).is_err());
    }
}
