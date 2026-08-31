#![doc = "Deterministic reliability tooling for the Tondo repository."]

pub mod gap_audit;
pub mod generator;
pub mod harness;
pub mod inventory;
pub mod layer_evidence;
pub mod matrix;
pub mod provenance;
pub mod quality;
pub mod ratchet;
pub mod regression;
pub mod spec_structure;
pub mod sync_model;
pub mod tracker;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const INVENTORY_PATH: &str = "testing/inventory.json";
pub const MATRIX_PATH: &str = "testing/coverage-matrix.json";
pub const QUALITY_BASELINE_PATH: &str = "testing/quality-baseline.json";
pub const REGRESSION_LEDGER_PATH: &str = "testing/regressions.json";

pub fn workspace_root(start: &Path) -> Result<PathBuf, String> {
    let mut candidate = start
        .canonicalize()
        .map_err(|error| format!("cannot resolve `{}`: {error}", start.display()))?;
    loop {
        let manifest = candidate.join("Cargo.toml");
        if manifest.is_file() {
            let contents = fs::read_to_string(&manifest)
                .map_err(|error| format!("cannot read `{}`: {error}", manifest.display()))?;
            if contents.lines().any(|line| line.trim() == "[workspace]") {
                return Ok(candidate);
            }
        }
        if !candidate.pop() {
            return Err(format!(
                "cannot find a Cargo workspace above `{}`",
                start.display()
            ));
        }
    }
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|error| format!("cannot encode JSON: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create `{}`: {error}", parent.display()))?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write `{}`: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        format!(
            "cannot replace `{}` with `{}`: {error}",
            path.display(),
            temporary.display()
        )
    })?;
    Ok(true)
}

pub fn check_bytes(path: &Path, expected: &[u8]) -> Result<(), String> {
    let actual =
        fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    if actual == expected {
        return Ok(());
    }
    Err(format!(
        "`{}` is stale; run `cargo run -p tondo-reliability -- generate --root .`",
        path.display()
    ))
}

pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

pub fn logical_path(root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(root)
        .map_err(|error| {
            format!(
                "`{}` is outside workspace `{}`: {error}",
                path.display(),
                root.display()
            )
        })?
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| format!("path `{}` is not valid UTF-8", path.display()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

pub fn collect_files(root: &Path, directory: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    collect_files_inner(root, directory, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_inner(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read `{}`: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot enumerate `{}`: {error}", directory.display()))?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if path.is_dir() {
            if relative
                .components()
                .any(|component| matches!(component.as_os_str().to_str(), Some(".git" | "target")))
            {
                continue;
            }
            collect_files_inner(root, &path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::Serialize;

    use super::*;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let temp_root = std::env::temp_dir();
            let mut base = temp_root.clone();
            // The SSD test profile deliberately places `TMPDIR` below the
            // checked-out repository.  A workspace-discovery test must not
            // accidentally inherit that repository's Cargo.toml, so escape
            // the nearest workspace before creating its isolated fixture.
            let mut cursor = base.clone();
            while !cursor.join("Cargo.toml").is_file() {
                let Some(parent) = cursor.parent() else {
                    break;
                };
                cursor = parent.to_path_buf();
            }
            if cursor.join("Cargo.toml").is_file() && temp_root.starts_with(&cursor) {
                base = cursor
                    .parent()
                    .expect("a workspace directory must have a parent")
                    .to_path_buf();
            }
            let path = base.join(format!(
                "tondo-reliability-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Serialize)]
    struct Document {
        value: u32,
    }

    #[test]
    fn workspace_discovery_stops_at_the_nearest_workspace_and_reports_absence() {
        let directory = TemporaryDirectory::new("workspace");
        fs::write(
            directory.0.join("Cargo.toml"),
            "[workspace]\nmembers = []\n",
        )
        .unwrap();
        let nested = directory.0.join("nested/deeper");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            workspace_root(&nested).unwrap(),
            directory.0.canonicalize().unwrap()
        );

        let absent = TemporaryDirectory::new("no-workspace");
        assert!(
            workspace_root(&absent.0)
                .unwrap_err()
                .contains("cannot find")
        );
        assert!(
            workspace_root(&absent.0.join("missing"))
                .unwrap_err()
                .contains("cannot resolve")
        );
    }

    #[test]
    fn canonical_files_are_replaced_only_on_change_and_checked_exactly() {
        let directory = TemporaryDirectory::new("canonical");
        let path = directory.0.join("nested/evidence.json");
        let bytes = canonical_json(&Document { value: 42 }).unwrap();
        assert_eq!(bytes, b"{\n  \"value\": 42\n}\n");
        assert!(write_if_changed(&path, &bytes).unwrap());
        assert!(!write_if_changed(&path, &bytes).unwrap());
        check_bytes(&path, &bytes).unwrap();

        let parent_blocker = directory.0.join("parent-blocker");
        fs::write(&parent_blocker, b"file").unwrap();
        assert!(
            write_if_changed(&parent_blocker.join("evidence.json"), &bytes)
                .unwrap_err()
                .contains("cannot create")
        );

        let temporary_blocker = directory.0.join("temporary-blocker.json");
        fs::create_dir(temporary_blocker.with_extension("tmp")).unwrap();
        assert!(
            write_if_changed(&temporary_blocker, &bytes)
                .unwrap_err()
                .contains("cannot write")
        );
        assert!(
            check_bytes(&path, b"different")
                .unwrap_err()
                .contains("stale")
        );
        assert!(
            check_bytes(&directory.0.join("missing"), &bytes)
                .unwrap_err()
                .contains("cannot read")
        );
        assert_eq!(
            sha256(b"tondo"),
            "a363d0a0361858dfd0bab4fa12573ba30d4feee1d1104bf45265225150bed6bd"
        );
    }

    #[test]
    fn logical_paths_and_recursive_collection_are_closed_to_the_workspace() {
        let directory = TemporaryDirectory::new("collection");
        fs::create_dir_all(directory.0.join("src/nested")).unwrap();
        fs::create_dir_all(directory.0.join(".git")).unwrap();
        fs::create_dir_all(directory.0.join("target")).unwrap();
        fs::create_dir_all(directory.0.join("nested/target")).unwrap();
        fs::write(directory.0.join("src/z.rs"), "").unwrap();
        fs::write(directory.0.join("src/nested/a.rs"), "").unwrap();
        fs::write(directory.0.join(".git/ignored"), "").unwrap();
        fs::write(directory.0.join("target/ignored"), "").unwrap();
        fs::write(directory.0.join("nested/target/ignored"), "").unwrap();

        assert_eq!(
            logical_path(&directory.0, &directory.0.join("src/nested/a.rs")).unwrap(),
            "src/nested/a.rs"
        );
        assert!(
            logical_path(&directory.0, Path::new("/definitely/outside"))
                .unwrap_err()
                .contains("outside workspace")
        );
        assert_eq!(
            collect_files(&directory.0, &directory.0).unwrap(),
            [
                directory.0.join("src/nested/a.rs"),
                directory.0.join("src/z.rs"),
            ]
        );
        assert!(
            collect_files(&directory.0, &directory.0.join("missing"))
                .unwrap_err()
                .contains("cannot read")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_logical_components_are_rejected() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let directory = TemporaryDirectory::new("non-utf8");
        let path = directory
            .0
            .join(OsString::from_vec(vec![b'f', 0xff, b'.', b'r', b's']));
        assert!(
            logical_path(&directory.0, &path)
                .unwrap_err()
                .contains("not valid UTF-8")
        );
    }
}
