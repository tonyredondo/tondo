#![doc = "Deterministic reliability tooling for the Tondo repository."]

pub mod generator;
pub mod harness;
pub mod inventory;
pub mod matrix;
pub mod quality;
pub mod regression;

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
            if matches!(
                relative
                    .components()
                    .next()
                    .and_then(|item| item.as_os_str().to_str()),
                Some(".git" | "target")
            ) {
                continue;
            }
            collect_files_inner(root, &path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}
