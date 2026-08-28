//! Content identity for quality reports.
//!
//! Coverage and mutation percentages are meaningful only when the report can
//! be tied to the exact inputs and toolchain that produced it.  This module
//! keeps that identity deliberately small and deterministic: source inputs are
//! hashed by logical path, path-only build locations are excluded, and the
//! active compiler environment is recorded separately.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{canonical_json, collect_files, logical_path, quality::canonical_report_bytes, sha256};

pub const FORMAT: &str = "tondo-quality-provenance/1";
pub const BINDING_FORMAT: &str = "tondo-quality-report-binding/1";

const ROOT_FILES: [&str; 2] = ["Cargo.lock", "Cargo.toml"];
const INPUT_DIRECTORIES: [&str; 7] = [
    ".cargo",
    ".github/workflows",
    "conformance/draft",
    "crates",
    "fuzz",
    "scripts",
    "tests",
];
// Fuzzers write crash payloads here.  They are useful diagnostics, but are
// generated state rather than a source/build input and are not present in a
// clean checkout.  Keeping them out makes quality identities reproducible
// between a developer workspace and CI.
const GENERATED_INPUT_PREFIXES: [&str; 1] = ["fuzz/artifacts/"];
const ENVIRONMENT_KEYS: [&str; 11] = [
    "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_PROFILE_DEV_DEBUG",
    "CARGO_PROFILE_RELEASE_DEBUG",
    "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_OPT_LEVEL",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "TONDO_TEST_TARGET",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    pub rustc: String,
    pub cargo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityProvenance {
    pub format: String,
    pub tree_sha256: String,
    pub input_set_sha256: String,
    pub file_count: u64,
    pub flags: Vec<String>,
    pub toolchain: Toolchain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportBinding {
    pub format: String,
    pub kind: String,
    pub report_sha256: String,
    pub before: QualityProvenance,
    pub after: QualityProvenance,
}

#[derive(Debug, Clone, Serialize)]
struct InputFile {
    path: String,
    sha256: String,
}

impl QualityProvenance {
    /// Computes the identity of the source/build inputs relevant to quality.
    pub fn current(root: &Path) -> Result<Self, String> {
        let mut files = Vec::new();
        for path in ROOT_FILES {
            let absolute = root.join(path);
            if !absolute.is_file() {
                return Err(format!("quality provenance requires `{path}`"));
            }
            files.push(absolute);
        }
        for directory in INPUT_DIRECTORIES {
            let absolute = root.join(directory);
            if absolute.is_dir() {
                for path in collect_files(root, &absolute)? {
                    let logical = logical_path(root, &path)?;
                    if GENERATED_INPUT_PREFIXES
                        .iter()
                        .any(|prefix| logical.starts_with(prefix))
                    {
                        continue;
                    }
                    files.push(path);
                }
            }
        }
        files.sort();
        files.dedup();

        let mut entries = Vec::with_capacity(files.len());
        for path in files {
            let logical = logical_path(root, &path)?;
            let bytes = fs::read(&path)
                .map_err(|error| format!("cannot read provenance input `{logical}`: {error}"))?;
            entries.push(InputFile {
                path: logical,
                sha256: sha256(&bytes),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let paths = entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        let flags = active_flags();
        let provenance = Self {
            format: FORMAT.into(),
            tree_sha256: sha256(&canonical_json(&entries)?),
            input_set_sha256: sha256(&canonical_json(&paths)?),
            file_count: entries.len() as u64,
            flags,
            toolchain: Toolchain {
                rustc: command_version("rustc", &["--version", "--verbose"])?,
                cargo: command_version("cargo", &["--version"])?,
            },
        };
        provenance.validate()?;
        Ok(provenance)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT
            || !is_sha256(&self.tree_sha256)
            || !is_sha256(&self.input_set_sha256)
            || self.file_count == 0
            || self.toolchain.rustc.is_empty()
            || self.toolchain.cargo.is_empty()
        {
            return Err("quality provenance has an invalid identity or toolchain".into());
        }
        if self.flags.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err("quality provenance flags must be sorted and unique".into());
        }
        if self.flags.iter().any(|flag| {
            let Some((key, value)) = flag.split_once('=') else {
                return true;
            };
            key.is_empty() || value.is_empty()
        }) {
            return Err("quality provenance flags must be non-empty key=value pairs".into());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        Ok(sha256(&canonical_json(self)?))
    }
}

impl ReportBinding {
    pub fn new(
        kind: &str,
        report_bytes: &[u8],
        before: QualityProvenance,
        after: QualityProvenance,
    ) -> Result<Self, String> {
        validate_kind(kind)?;
        before.validate()?;
        after.validate()?;
        if before != after {
            return Err("quality report input tree changed during the run".into());
        }
        Ok(Self {
            format: BINDING_FORMAT.into(),
            kind: kind.into(),
            report_sha256: sha256(&canonical_report_bytes(kind, report_bytes)?),
            before,
            after,
        })
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path)
            .map_err(|error| format!("cannot read report binding `{}`: {error}", path.display()))?;
        let binding: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid report binding `{}`: {error}", path.display()))?;
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != BINDING_FORMAT || !is_sha256(&self.report_sha256) {
            return Err("quality report binding has an invalid format or report hash".into());
        }
        validate_kind(&self.kind)?;
        self.before.validate()?;
        self.after.validate()?;
        if self.before != self.after {
            return Err("quality report binding records a changed input tree".into());
        }
        Ok(())
    }

    /// Verifies the canonical parsed report and the live source identity.
    pub fn verify(
        &self,
        root: &Path,
        report_path: &Path,
        expected_kind: &str,
    ) -> Result<(), String> {
        self.validate()?;
        validate_kind(expected_kind)?;
        if self.kind != expected_kind {
            return Err(format!(
                "quality report binding kind is `{}`, expected `{expected_kind}`",
                self.kind
            ));
        }
        let report = fs::read(report_path)
            .map_err(|error| format!("cannot read `{}`: {error}", report_path.display()))?;
        let actual_report = sha256(&canonical_report_bytes(expected_kind, &report)?);
        if actual_report != self.report_sha256 {
            return Err(format!(
                "{expected_kind} report changed after its binding was created"
            ));
        }
        let current = Self::provenance(root)?;
        if current != self.after {
            return Err(format!(
                "{expected_kind} report was produced from a different source tree, flags, or toolchain"
            ));
        }
        Ok(())
    }

    pub fn provenance_digest(&self) -> Result<String, String> {
        self.after.digest()
    }

    fn provenance(root: &Path) -> Result<QualityProvenance, String> {
        QualityProvenance::current(root)
    }
}

fn active_flags() -> Vec<String> {
    let mut flags = ENVIRONMENT_KEYS
        .into_iter()
        .filter_map(|key| env::var(key).ok().map(|value| format!("{key}={value}")))
        .collect::<Vec<_>>();
    flags.sort();
    flags
}

fn command_version(command: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(command)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot execute `{command}` for quality provenance: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`{command}` failed while capturing quality provenance ({})",
            output.status
        ));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|error| format!("`{command}` returned non-UTF-8 version output: {error}"))?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(format!("`{command}` returned empty version output"));
    }
    Ok(value)
}

fn validate_kind(kind: &str) -> Result<(), String> {
    if matches!(kind, "coverage" | "mutation") {
        Ok(())
    } else {
        Err(format!("unsupported quality report kind `{kind}`"))
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> std::path::PathBuf {
        let path = env::temp_dir().join(format!(
            "tondo-provenance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("crates/example/src")).unwrap();
        fs::create_dir_all(path.join("scripts")).unwrap();
        fs::write(path.join("Cargo.toml"), "[workspace]\nmembers=[]\n").unwrap();
        fs::write(path.join("Cargo.lock"), "version = 4\n").unwrap();
        fs::write(
            path.join("crates/example/src/lib.rs"),
            "pub fn value() {}\n",
        )
        .unwrap();
        fs::write(path.join("scripts/check.sh"), "#!/bin/sh\n").unwrap();
        path
    }

    #[test]
    fn current_identity_is_stable_and_tracks_inputs() {
        let root = fixture();
        let first = QualityProvenance::current(&root).unwrap();
        let second = QualityProvenance::current(&root).unwrap();
        assert_eq!(first, second);
        assert!(first.file_count >= 4);

        fs::create_dir_all(root.join("fuzz/artifacts/frontend")).unwrap();
        fs::write(root.join("fuzz/artifacts/frontend/crash"), b"generated").unwrap();
        let with_generated_artifact = QualityProvenance::current(&root).unwrap();
        assert_eq!(first, with_generated_artifact);

        fs::write(root.join("scripts/check.sh"), "#!/bin/sh\necho changed\n").unwrap();
        let changed = QualityProvenance::current(&root).unwrap();
        assert_ne!(first.tree_sha256, changed.tree_sha256);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn binding_rejects_changed_reports_and_trees() {
        let root = fixture();
        let before = QualityProvenance::current(&root).unwrap();
        let report_bytes = br#"{"outcome":"Caught","name":"frontier"}"#;
        let binding = ReportBinding::new("mutation", report_bytes, before.clone(), before).unwrap();
        let report = root.join("coverage.json");
        fs::write(&report, report_bytes).unwrap();
        binding.verify(&root, &report, "mutation").unwrap();
        fs::write(
            &report,
            br#"{"name":"frontier","outcome":"Killed","timing":17}"#,
        )
        .unwrap();
        binding.verify(&root, &report, "mutation").unwrap();
        fs::write(&report, br#"{"outcome":"Missed","name":"frontier"}"#).unwrap();
        assert!(binding.verify(&root, &report, "mutation").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validation_rejects_unknown_kinds_and_unsorted_flags() {
        assert!(validate_kind("other").is_err());
        let mut provenance = QualityProvenance {
            format: FORMAT.into(),
            tree_sha256: "a".repeat(64),
            input_set_sha256: "b".repeat(64),
            file_count: 1,
            flags: vec!["B=1".into(), "A=1".into()],
            toolchain: Toolchain {
                rustc: "rustc".into(),
                cargo: "cargo".into(),
            },
        };
        assert!(provenance.validate().is_err());
        provenance.flags.clear();
        provenance.validate().unwrap();
        assert!(ReportBinding::new("other", b"report", provenance.clone(), provenance).is_err());
    }
}
