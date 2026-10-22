//! The snapshot store and the artifact store (testing 15.7, 15.8).

use crate::hash::{sha256, sha256_hex};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The snapshot store of a package: a file `<name>.snap` for each snapshot
/// under `snapshots/`, in a directory for its test, which holds the value.
pub(super) struct Snapshots {
    root: PathBuf,
    /// The values by test or suite and name.
    entries: BTreeMap<(String, String), String>,
    /// The hash of the store tree before the run.
    pub before: String,
    package: String,
}

impl Snapshots {
    /// Reads `snapshots/` of the package at `dir`, where every file must be
    /// a snapshot; a store that is not there is empty.
    pub(super) fn read(dir: &Path, package: &str) -> Result<Snapshots, String> {
        let root = dir.join("snapshots");
        let mut entries = BTreeMap::new();
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let Ok(list) = std::fs::read_dir(&directory) else { continue };
            for entry in list {
                let path = entry.map_err(|e| format!("cannot read {}: {e}", directory.display()))?.path();
                let shown = path.strip_prefix(dir).unwrap_or(&path).display().to_string();
                let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
                if meta.is_dir() {
                    pending.push(path);
                    continue;
                }
                let steps: Vec<String> = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect();
                let name = steps.last().and_then(|f| f.strip_suffix(".snap")).filter(|n| !n.is_empty());
                let (Some(name), true, true) = (name, steps.len() >= 4, meta.is_file()) else {
                    return Err(format!(
                        "{shown} is not a snapshot: `snapshots/` holds only files `<kind>/<module>/<suites...>/<test>/<name>.snap`; move or remove it"
                    ));
                };
                let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {shown}: {e}"))?;
                let value = String::from_utf8(bytes).map_err(|_| format!("{shown} is not UTF-8"))?;
                let test = steps[..steps.len() - 1].join("::");
                entries.insert((format!("{package}::{test}"), name.to_string()), value);
            }
        }
        let before = tree_hash(package, &entries);
        Ok(Snapshots { root, entries, before, package: package.to_string() })
    }

    /// The entries for the worker: three lengths on a line, then the
    /// identity, the name and the value.
    pub(super) fn for_worker(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for ((id, name), value) in &self.entries {
            out.extend_from_slice(format!("{} {} {}\n", id.len(), name.len(), value.len()).as_bytes());
            out.extend_from_slice(id.as_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        out
    }

    /// An entry's value, if there is one.
    pub(super) fn expected(&self, id: &str, name: &str) -> Option<String> {
        self.entries.get(&(id.to_string(), name.to_string())).cloned()
    }

    /// Writes the files of these values, keeping every other; the hash of
    /// the tree after.
    pub(super) fn publish(&self, values: Vec<((String, String), String)>) -> Result<String, String> {
        let mut entries = self.entries.clone();
        for ((id, name), value) in values {
            let test = id.strip_prefix(&format!("{}::", self.package)).unwrap_or(&id);
            let mut path = self.root.clone();
            path.extend(test.split("::"));
            std::fs::create_dir_all(&path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            write_atomically(&path.join(format!("{name}.snap")), value.as_bytes())?;
            entries.insert((id, name), value);
        }
        Ok(tree_hash(&self.package, &entries))
    }
}

/// The hash of the stores of the packages under test
/// (`tondo-snapshot-tree-v2`): for the package, its name and how many
/// snapshots it has, then each snapshot's path in the store and the hash of
/// its value, in the order of their paths.
fn tree_hash(package: &str, entries: &BTreeMap<(String, String), String>) -> String {
    let mut files: Vec<(String, &String)> = entries
        .iter()
        .map(|((id, name), value)| {
            let test = id.strip_prefix(&format!("{package}::")).unwrap_or(id);
            (format!("{}/{name}.snap", test.replace("::", "/")), value)
        })
        .collect();
    files.sort();
    let mut bytes = b"tondo-snapshot-tree-v2\0".to_vec();
    bytes.extend_from_slice(package.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&(files.len() as u64).to_be_bytes());
    for (path, value) in files {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&sha256(value.as_bytes()));
    }
    sha256_hex(&bytes)
}

/// The artifact store: content-addressed objects and their manifest.
pub(super) struct Artifacts {
    pub root: PathBuf,
    /// The first object that could not be written, which fails the run.
    pub error: Mutex<Option<String>>,
}

impl Artifacts {
    /// Stores the bytes as `objects/<sha256>`, unless an object with exactly
    /// them is there; their hash.
    pub(super) fn store(&self, bytes: &[u8]) -> String {
        let hash = sha256_hex(bytes);
        let path = self.root.join("objects").join(&hash);
        let result = match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() && std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == hash) => Ok(()),
            Ok(_) => Err(format!("the artifact object {} is not what its name says", path.display())),
            Err(_) => std::fs::create_dir_all(self.root.join("objects"))
                .map_err(|e| e.to_string())
                .and_then(|_| write_atomically(&path, bytes)),
        };
        if let Err(error) = result {
            self.error.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(error);
        }
        hash
    }
}

pub(super) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension(format!("partial-{}", std::process::id()));
    std::fs::write(&partial, bytes)
        .and_then(|_| std::fs::rename(&partial, path))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_a_file_per_snapshot() {
        let dir = std::env::temp_dir().join(format!("tondo-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let empty = Snapshots::read(&dir, "app").unwrap();
        let value = "line\n\"quoted\" \u{1}".to_string();
        let id = "app::unit::app::math::adds".to_string();
        let after = empty.publish(vec![((id.clone(), "out".into()), value.clone())]).unwrap();
        assert_ne!(after, empty.before);
        let path = dir.join("snapshots/unit/app/math/adds/out.snap");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), value);
        let read = Snapshots::read(&dir, "app").unwrap();
        assert_eq!((read.expected(&id, "out"), read.before), (Some(value), after));
        std::fs::write(dir.join("snapshots/unit/app/notes.txt"), "").unwrap();
        let error = Snapshots::read(&dir, "app").err().unwrap();
        assert!(error.starts_with("snapshots/unit/app/notes.txt is not a snapshot"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
