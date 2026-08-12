use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tondo_conformance::document::extract_fences;
use tondo_conformance::manifest::SuiteManifest;
use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, CompilationState, DocCategory, TargetSelection,
    WireDocumentFenceAction,
};
use unicode_normalization::UnicodeNormalization;

const SUITE_MANIFEST: &[u8] = include_bytes!("../../../conformance/0.1/manifest.json");
const FIXTURE_MANIFEST: &[u8] =
    include_bytes!("../../../conformance/0.1/fixtures/tondo-fixture-manifest.txt");
const FIXTURE_MANIFEST_SHA256: &str =
    "1b6ab9f853b7ef4b94b4b9aaff6297e20556f81e8d99c322bed03854453d76c2";

#[derive(Debug)]
pub enum DocTestError {
    Usage(String),
    Diagnostic(String),
    Internal(String),
}

pub fn execute(arguments: &[OsString]) -> Result<Vec<u8>, DocTestError> {
    let markdown = parse(arguments)?;
    let file = logical_file_name(&markdown)?;
    let bytes = fs::read(&markdown).map_err(|error| {
        DocTestError::Diagnostic(format!(
            "cannot read documentation Markdown `{}`: {error}",
            markdown.display()
        ))
    })?;
    let manifest = match serde_json::from_slice::<SuiteManifest>(SUITE_MANIFEST) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Err(DocTestError::Internal(format!(
                "embedded conformance registry is invalid: {error}"
            )));
        }
    };
    let registered_errors = manifest
        .registry
        .errors
        .into_iter()
        .collect::<BTreeSet<_>>();
    let fences = extract_fences(&bytes, &registered_errors)
        .map_err(|error| DocTestError::Diagnostic(format!("{}: {error}", file)))?;
    let fixture_hash = tondo_conformance::sha256(FIXTURE_MANIFEST);
    if fixture_hash != FIXTURE_MANIFEST_SHA256 {
        return Err(DocTestError::Internal(format!(
            "embedded fixture manifest hash `{fixture_hash}` does not match `{FIXTURE_MANIFEST_SHA256}`"
        )));
    }

    let target = TargetSelection {
        name: "tondo-vm-hosted".into(),
        profile: "hosted".into(),
        capabilities: vec!["console".into(), "process".into()],
    };
    let fixture_hex = tondo_conformance::encode_hex(FIXTURE_MANIFEST);
    let mut records = Vec::with_capacity(fences.len());
    for (sequence, fence) in fences.iter().enumerate() {
        if fence.category == DocCategory::Pseudocode {
            records.push(pseudocode_record(&file, fence));
            continue;
        }
        let action = WireDocumentFenceAction {
            file: file.clone(),
            fence_byte: fence.fence_byte,
            category: fence.category,
            fixture: fence.fixture.clone(),
            fixture_manifest_hex: fixture_hex.clone(),
            fixture_manifest_sha256: FIXTURE_MANIFEST_SHA256.into(),
            expected_codes: fence.expected_codes.clone(),
            source_hex: tondo_conformance::encode_hex(&fence.source),
        };
        let request = AdapterRequest::new(
            u64::try_from(sequence + 1).unwrap_or(u64::MAX),
            format!("doc-test/{}@{}", file, fence.fence_byte),
            target.clone(),
            AdapterAction::DocumentFence(action.clone()),
        );
        let observation = match tondo_reference_adapter::observe_document_fence(&request, &action) {
            Ok(observation) => observation,
            Err(error) => {
                return Err(DocTestError::Diagnostic(format!(
                    "{} at byte {}: {error}",
                    file, fence.fence_byte
                )));
            }
        };
        if observation.compilation != CompilationState::Success {
            return Err(DocTestError::Diagnostic(format!(
                "{} at byte {} did not satisfy its documentation contract",
                file, fence.fence_byte
            )));
        }
        let record = match observation.data.as_object() {
            Some(record) => record,
            None => {
                return Err(DocTestError::Internal(format!(
                    "documentation adapter returned a non-object record at byte {}",
                    fence.fence_byte
                )));
            }
        };
        records.push(Value::Object(record.clone()));
    }

    let mut output = match serde_json::to_vec(&Value::Array(records)) {
        Ok(output) => output,
        Err(error) => {
            return Err(DocTestError::Internal(format!(
                "cannot encode doc-test JSON: {error}"
            )));
        }
    };
    output.push(b'\n');
    Ok(output)
}

fn parse(arguments: &[OsString]) -> Result<PathBuf, DocTestError> {
    if arguments.first().and_then(|argument| argument.to_str()) != Some("doc-test") {
        return Err(DocTestError::Usage(
            "the `doc-test` command is required".into(),
        ));
    }
    let mut edition = None;
    let mut markdown = None;
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .ok_or_else(|| DocTestError::Usage("doc-test arguments must be valid UTF-8".into()))?;
        if argument == "--edition" {
            index += 1;
            let value = arguments
                .get(index)
                .and_then(|value| value.to_str())
                .ok_or_else(|| DocTestError::Usage("`--edition` requires `0.1`".into()))?;
            if edition.replace(value.to_owned()).is_some() {
                return Err(DocTestError::Usage(
                    "`--edition` may appear only once".into(),
                ));
            }
        } else if let Some(value) = argument.strip_prefix("--edition=") {
            if edition.replace(value.to_owned()).is_some() {
                return Err(DocTestError::Usage(
                    "`--edition` may appear only once".into(),
                ));
            }
        } else if argument.starts_with('-') {
            return Err(DocTestError::Usage(format!(
                "unknown doc-test option `{argument}`"
            )));
        } else if markdown.replace(PathBuf::from(argument)).is_some() {
            return Err(DocTestError::Usage(
                "doc-test accepts exactly one Markdown path".into(),
            ));
        }
        index += 1;
    }
    if edition.as_deref() != Some("0.1") {
        return Err(DocTestError::Usage(
            "`--edition 0.1` is required by the Tondo 0.1 doc-test contract".into(),
        ));
    }
    markdown.ok_or_else(|| DocTestError::Usage("a Markdown path is required".into()))
}

fn logical_file_name(path: &std::path::Path) -> Result<String, DocTestError> {
    let raw = path
        .as_os_str()
        .to_str()
        .ok_or_else(|| DocTestError::Usage("the Markdown path must be valid UTF-8".into()))?;
    #[cfg(windows)]
    let raw = raw.replace('\\', "/");
    #[cfg(not(windows))]
    let raw = raw.to_owned();
    Ok(raw.nfc().collect())
}

fn pseudocode_record(file: &str, fence: &tondo_conformance::document::DocumentFence) -> Value {
    json!({
        "file": file,
        "fence_byte": fence.fence_byte,
        "category": "pseudocode",
        "edition": "0.1",
        "fixture": null,
        "fixture_sha256": null,
        "production": null,
        "source_sha256": fence.source_sha256,
        "formatted_sha256": null,
        "parse_ok": null,
        "typecheck_ok": null,
        "expected_codes": [],
        "actual_codes": []
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[cfg(unix)]
    #[test]
    fn logical_file_name_rejects_non_utf8_paths() {
        let path = PathBuf::from(OsString::from_vec(vec![b'd', b'o', 0xff]));
        let error = logical_file_name(&path).unwrap_err();
        assert!(matches!(error, DocTestError::Usage(message) if message.contains("valid UTF-8")));
    }

    #[test]
    fn execute_publishes_syntax_and_pseudocode_records() {
        let path = std::env::temp_dir().join(format!(
            "tondo-doc-test-unit-{}-{}.md",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        fs::write(
            &path,
            b"~~~tondo\nlet value = 1\n~~~\n~~~tondo pseudocode\nnot Tondo\n~~~\n",
        )
        .unwrap();
        let output = execute(&[
            OsString::from("doc-test"),
            OsString::from("--edition"),
            OsString::from("0.1"),
            path.clone().into_os_string(),
        ])
        .unwrap();
        fs::remove_file(path).unwrap();

        let records: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(records.as_array().unwrap().len(), 2);
        assert_eq!(records[1]["category"], "pseudocode");

        let missing = std::env::temp_dir().join(format!(
            "tondo-doc-test-unit-missing-{}.md",
            std::process::id()
        ));
        let error = execute(&[
            OsString::from("doc-test"),
            OsString::from("--edition"),
            OsString::from("0.1"),
            missing.into_os_string(),
        ])
        .unwrap_err();
        assert!(
            matches!(error, DocTestError::Diagnostic(message) if message.contains("cannot read"))
        );

        assert!(matches!(
            parse(&[OsString::from("doc-test"), OsString::from("--edition")]),
            Err(DocTestError::Usage(message)) if message.contains("requires `0.1`")
        ));
        assert!(matches!(
            parse(&[
                OsString::from("doc-test"),
                OsString::from("--edition"),
                OsString::from("0.1"),
            ]),
            Err(DocTestError::Usage(message)) if message.contains("Markdown path")
        ));
        #[cfg(unix)]
        assert!(matches!(
            parse(&[
                OsString::from("doc-test"),
                OsString::from("--edition"),
                OsString::from("0.1"),
                OsString::from_vec(vec![b'd', b'o', 0xff]),
            ]),
            Err(DocTestError::Usage(message)) if message.contains("valid UTF-8")
        ));
    }

    #[test]
    fn execute_rejects_a_compile_fail_contract_mismatch() {
        let path = std::env::temp_dir().join(format!(
            "tondo-doc-test-unit-fail-{}-{}.md",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        fs::write(
            &path,
            b"~~~tondo compile-fail E0005\nlet value: Int = \"text\"\n~~~\n",
        )
        .unwrap();
        let error = execute(&[
            OsString::from("doc-test"),
            OsString::from("--edition=0.1"),
            path.clone().into_os_string(),
        ])
        .unwrap_err();
        fs::remove_file(path).unwrap();

        assert!(
            matches!(error, DocTestError::Diagnostic(message) if message.contains("did not satisfy"))
        );
    }
}
