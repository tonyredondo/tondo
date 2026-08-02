//! Materialized build-only `std.meta` companion from the candidate distribution.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::sha256;
use crate::meta::META_API;

pub const STD_META_FORMAT: &str = "tondo-std-meta-package-draft";
pub const STD_META_PACKAGE: &str = "toolchain:std-meta:draft";
pub const STD_META_TARGET: &str = "tondo-meta";
pub const STD_META_PROFILE: &str = "meta";

const SOURCE_BYTES: &[u8] = include_bytes!("../../../stdlib/meta/src/meta.to");
const DESCRIPTOR_BYTES: &[u8] = include_bytes!("../../../stdlib/meta/descriptor.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdMetaSourceDescriptor {
    logical_path: String,
    module: String,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StdMetaDescriptor {
    api: String,
    content_hash: String,
    format: String,
    package_id: String,
    profile: String,
    source: StdMetaSourceDescriptor,
    target: String,
}

#[derive(Debug, Serialize)]
struct StdMetaContent<'a> {
    api: &'a str,
    format: &'a str,
    package_id: &'a str,
    profile: &'a str,
    source: &'a StdMetaSourceDescriptor,
    target: &'a str,
}

/// Immutable package bytes admitted to the separate meta graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdMetaPackage {
    descriptor: StdMetaDescriptor,
}

impl StdMetaPackage {
    pub fn load_candidate() -> Result<Self, StdMetaError> {
        Self::load(candidate_descriptor_bytes(), SOURCE_BYTES)
    }

    pub fn load(descriptor_bytes: &[u8], source_bytes: &[u8]) -> Result<Self, StdMetaError> {
        let descriptor: StdMetaDescriptor = serde_json::from_slice(descriptor_bytes)
            .map_err(|error| StdMetaError::Descriptor(error.to_string()))?;
        if serde_json::to_vec(&descriptor)
            .map_err(|error| StdMetaError::Descriptor(error.to_string()))?
            != descriptor_bytes
        {
            return Err(StdMetaError::NonCanonicalDescriptor);
        }
        descriptor.validate(source_bytes)?;
        Ok(Self { descriptor })
    }

    pub fn descriptor(&self) -> &StdMetaDescriptor {
        &self.descriptor
    }

    pub fn source(&self) -> &'static [u8] {
        SOURCE_BYTES
    }

    pub fn content_hash(&self) -> &str {
        &self.descriptor.content_hash
    }
}

fn candidate_descriptor_bytes() -> &'static [u8] {
    DESCRIPTOR_BYTES
        .strip_suffix(b"\n")
        .expect("the checked-in text descriptor has one repository newline")
}

impl StdMetaDescriptor {
    fn validate(&self, source_bytes: &[u8]) -> Result<(), StdMetaError> {
        for (actual, expected, field) in [
            (self.api.as_str(), META_API, "api"),
            (self.format.as_str(), STD_META_FORMAT, "format"),
            (self.package_id.as_str(), STD_META_PACKAGE, "package_id"),
            (self.profile.as_str(), STD_META_PROFILE, "profile"),
            (self.target.as_str(), STD_META_TARGET, "target"),
            (
                self.source.logical_path.as_str(),
                "src/meta.to",
                "source.logical_path",
            ),
            (self.source.module.as_str(), "meta", "source.module"),
        ] {
            if actual != expected {
                return Err(StdMetaError::Identity {
                    field,
                    expected,
                    actual: actual.to_owned(),
                });
            }
        }
        let source_hash = sha256(source_bytes);
        if self.source.sha256 != source_hash {
            return Err(StdMetaError::SourceHash {
                expected: self.source.sha256.clone(),
                actual: source_hash,
            });
        }
        let content = StdMetaContent {
            api: &self.api,
            format: &self.format,
            package_id: &self.package_id,
            profile: &self.profile,
            source: &self.source,
            target: &self.target,
        };
        let bytes = serde_json::to_vec(&content)
            .map_err(|error| StdMetaError::Descriptor(error.to_string()))?;
        let content_hash = sha256(&bytes);
        if self.content_hash != content_hash {
            return Err(StdMetaError::ContentHash {
                expected: self.content_hash.clone(),
                actual: content_hash,
            });
        }
        Ok(())
    }

    pub fn api(&self) -> &str {
        &self.api
    }

    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    pub fn source_hash(&self) -> &str {
        &self.source.sha256
    }
}

/// Canonical source rendering utilities shared by providers and generators.
pub struct MetaRenderer;

impl MetaRenderer {
    pub fn identifier(value: &str) -> Result<&str, StdMetaError> {
        crate::package::Name::new(value)
            .map_err(|_| StdMetaError::InvalidIdentifier(value.to_owned()))?;
        Ok(value)
    }

    pub fn string(value: &str) -> String {
        use fmt::Write as _;

        let mut output = String::with_capacity(value.len() + 2);
        output.push('"');
        for character in value.chars() {
            match character {
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                '\0' => output.push_str("\\0"),
                '\\' => output.push_str("\\\\"),
                '"' => output.push_str("\\\""),
                character if character.is_control() => {
                    write!(output, "\\u{{{:X}}}", u32::from(character))
                        .expect("writing to String cannot fail");
                }
                character => output.push(character),
            }
        }
        output.push('"');
        output
    }

    pub fn indentation(level: u32) -> Result<String, StdMetaError> {
        if level > 1_000_000 {
            return Err(StdMetaError::RenderLimit);
        }
        let bytes = usize::try_from(level)
            .ok()
            .and_then(|level| level.checked_mul(4))
            .ok_or(StdMetaError::RenderLimit)?;
        Ok(" ".repeat(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdMetaError {
    Descriptor(String),
    NonCanonicalDescriptor,
    Identity {
        field: &'static str,
        expected: &'static str,
        actual: String,
    },
    SourceHash {
        expected: String,
        actual: String,
    },
    ContentHash {
        expected: String,
        actual: String,
    },
    InvalidIdentifier(String),
    RenderLimit,
}

impl fmt::Display for StdMetaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Descriptor(error) => write!(formatter, "invalid std.meta descriptor: {error}"),
            Self::NonCanonicalDescriptor => {
                formatter.write_str("std.meta descriptor is not canonical")
            }
            Self::Identity {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "std.meta descriptor `{field}` expected `{expected}`, found `{actual}`"
            ),
            Self::SourceHash { expected, actual } => write!(
                formatter,
                "std.meta source hash expected `{expected}`, found `{actual}`"
            ),
            Self::ContentHash { expected, actual } => write!(
                formatter,
                "std.meta content hash expected `{expected}`, found `{actual}`"
            ),
            Self::InvalidIdentifier(value) => {
                write!(formatter, "`{value}` is not a renderable Tondo identifier")
            }
            Self::RenderLimit => formatter.write_str("std.meta rendering size overflow"),
        }
    }
}

impl Error for StdMetaError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{
        BuildTarget, CompilationRequest, CompilationStatus, DiagnosticFormat, HostProfile,
        Operation, ResourceLimits, SourceForm, execute,
    };
    use crate::meta::{
        MetaInput, MetaLimits, MetaOutputSpec, MetaRequest, MetaResponse, MetaSnapshot,
    };
    use crate::package::{Edition, PackageGraph};
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};

    #[test]
    fn candidate_package_has_exact_descriptor_source_and_content_hashes() {
        let package = StdMetaPackage::load_candidate().unwrap();
        assert_eq!(package.descriptor().api(), META_API);
        assert_eq!(package.descriptor().package_id(), STD_META_PACKAGE);
        assert_eq!(package.descriptor().source_hash(), sha256(package.source()));
        assert_eq!(
            package.content_hash(),
            "sha256:3d10de3a2a7535da69f6c9b1e990b314ad9974d7f32e786afde3cec044d7b4b8"
        );
    }

    #[test]
    fn candidate_source_compiles_for_the_closed_meta_target() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("toolchain:std-meta:draft").unwrap(),
                ModulePath::new("meta").unwrap(),
                LogicalPath::new("src/meta.to").unwrap(),
                SOURCE_BYTES,
            ))
            .unwrap();
        let graph = PackageGraph::loose(&sources, root).unwrap();
        let request = CompilationRequest::new(
            Operation::Check,
            Edition::V0_1,
            BuildTarget::tondo_meta(),
            HostProfile::Meta,
            Default::default(),
            DiagnosticFormat::Human,
            SourceForm::Module,
            ResourceLimits::default(),
            graph,
            sources,
            root,
        )
        .unwrap();
        let output = execute(request).unwrap();
        assert_eq!(
            output.status(),
            CompilationStatus::Success,
            "{:?}",
            output.diagnostics()
        );
        assert!(output.diagnostics().diagnostics().is_empty());
    }

    #[test]
    fn descriptor_rejects_noncanonical_identity_source_and_content_drift() {
        let descriptor_bytes = candidate_descriptor_bytes();
        let mut spaced = Vec::from(descriptor_bytes);
        spaced.push(b'\n');
        assert!(matches!(
            StdMetaPackage::load(&spaced, SOURCE_BYTES),
            Err(StdMetaError::NonCanonicalDescriptor)
        ));
        assert!(matches!(
            StdMetaPackage::load(descriptor_bytes, b"changed"),
            Err(StdMetaError::SourceHash { .. })
        ));

        let mut descriptor: serde_json::Value = serde_json::from_slice(descriptor_bytes).unwrap();
        descriptor["content_hash"] = serde_json::Value::String(sha256(b"wrong"));
        let bytes = serde_json::to_vec(&descriptor).unwrap();
        assert!(matches!(
            StdMetaPackage::load(&bytes, SOURCE_BYTES),
            Err(StdMetaError::ContentHash { .. })
        ));
    }

    #[test]
    fn renderer_escapes_strings_identifiers_and_indentation_canonically() {
        assert_eq!(MetaRenderer::identifier("validName").unwrap(), "validName");
        assert!(MetaRenderer::identifier("for").is_err());
        assert_eq!(
            MetaRenderer::string("a\n\t\0\\\"\u{7}🙂"),
            "\"a\\n\\t\\0\\\\\\\"\\u{7}🙂\""
        );
        assert_eq!(MetaRenderer::indentation(3).unwrap(), "            ");
        assert!(matches!(
            MetaRenderer::indentation(1_000_001),
            Err(StdMetaError::RenderLimit)
        ));
    }

    #[test]
    fn request_and_response_round_trip_only_canonical_owned_values() {
        let request = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [MetaInput::new("schema", b"value").unwrap()],
            [MetaOutputSpec::new("generated/value.to", "generated.value").unwrap()],
            MetaLimits::new(100, 1024, 1024).unwrap(),
        )
        .unwrap();
        let request_bytes = request.canonical_bytes().unwrap();
        assert_eq!(MetaRequest::decode(&request_bytes).unwrap(), request);
        assert!(request.hash().unwrap().starts_with("sha256:"));

        let mut builder = request.into_source_builder();
        builder
            .add_source("generated/value.to", "generated.value", b"fn value() {}")
            .unwrap();
        let response = builder.finish().unwrap();
        let response_bytes = response.canonical_bytes().unwrap();
        assert_eq!(MetaResponse::decode(&response_bytes).unwrap(), response);
        assert!(response.hash().unwrap().starts_with("sha256:"));
    }

    #[test]
    fn canonical_codecs_reject_api_hash_snapshot_and_encoding_drift() {
        let request = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [MetaInput::new("input", b"bytes").unwrap()],
            [MetaOutputSpec::new("out.to", "out").unwrap()],
            MetaLimits::new(1, 1, 1).unwrap(),
        )
        .unwrap();
        let bytes = request.canonical_bytes().unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        value["api"] = serde_json::json!("wrong");
        assert!(matches!(
            MetaRequest::decode(&serde_json::to_vec(&value).unwrap()),
            Err(crate::meta::MetaContractError::UnsupportedApi(_))
        ));
        value["api"] = serde_json::json!(META_API);
        value["inputs"][0]["hash"] = serde_json::json!(sha256(b"wrong"));
        assert!(matches!(
            MetaRequest::decode(&serde_json::to_vec(&value).unwrap()),
            Err(crate::meta::MetaContractError::InputHashMismatch(_))
        ));
        value["inputs"][0]["hash"] = serde_json::json!(sha256(b"bytes"));
        value["snapshot"]["format"] = serde_json::json!("wrong");
        assert!(matches!(
            MetaRequest::decode(&serde_json::to_vec(&value).unwrap()),
            Err(crate::meta::MetaContractError::InvalidSnapshot(_))
        ));

        let mut padded = bytes;
        padded.push(b'\n');
        assert!(matches!(
            MetaRequest::decode(&padded),
            Err(crate::meta::MetaContractError::NonCanonicalEncoding)
        ));
    }

    #[test]
    fn response_codec_rejects_source_hash_and_encoding_drift() {
        let request = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [],
            [MetaOutputSpec::new("out.to", "out").unwrap()],
            MetaLimits::new(1, 1, 32).unwrap(),
        )
        .unwrap();
        let mut builder = request.into_source_builder();
        builder.add_source("out.to", "out", b"fn out() {}").unwrap();
        let response = builder.finish().unwrap();
        let bytes = response.canonical_bytes().unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["outputs"][0]["hash"] = serde_json::json!(sha256(b"wrong"));
        assert!(matches!(
            MetaResponse::decode(&serde_json::to_vec(&value).unwrap()),
            Err(crate::meta::MetaContractError::SourceHashMismatch(_))
        ));
        let mut padded = bytes;
        padded.push(b'\n');
        assert!(matches!(
            MetaResponse::decode(&padded),
            Err(crate::meta::MetaContractError::NonCanonicalEncoding)
        ));
    }
}
