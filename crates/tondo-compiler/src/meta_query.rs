//! Canonical, privacy-bounded tooling view of accepted meta expansions.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::{sha256, validate_sha256};
use crate::meta_atomic::AcceptedMetaResult;
use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use crate::syntax::{
    LexLimits, LexMode, ParseLimits, ParseMode, format_parsed, lex_with_limits, parse,
};

pub const META_QUERY_FORMAT: &str = "tondo-meta-query-0.1/1";

/// Tooling-only facts supplied by the semantic owner of one accepted result.
/// No arbitrary symbol list exists: the exact derive target is the only private
/// identity this view can retain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaQueryDescriptor {
    identity_hash: String,
    target: Option<String>,
    introduced_bounds: Vec<String>,
}

impl MetaQueryDescriptor {
    pub fn new(
        identity_hash: impl Into<String>,
        target: Option<impl Into<String>>,
        introduced_bounds: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, MetaQueryError> {
        let identity_hash = identity_hash.into();
        require_hash("descriptor identity", &identity_hash)?;
        let target = target.map(Into::into);
        if let Some(target) = &target {
            require_text("derive target", target)?;
        }
        let mut introduced_bounds = introduced_bounds
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>();
        for bound in &introduced_bounds {
            require_text("introduced bound", bound)?;
        }
        introduced_bounds.sort();
        require_unique(&introduced_bounds, "introduced bound")?;
        Ok(Self {
            identity_hash,
            target,
            introduced_bounds,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaQueryDocument {
    format: String,
    expansions: Vec<MetaExpansion>,
}

impl MetaQueryDocument {
    pub fn build<'a>(
        results: impl IntoIterator<Item = &'a AcceptedMetaResult>,
        descriptors: impl IntoIterator<Item = MetaQueryDescriptor>,
    ) -> Result<Self, MetaQueryError> {
        let mut descriptor_map = BTreeMap::new();
        for descriptor in descriptors {
            let identity = descriptor.identity_hash.clone();
            if descriptor_map
                .insert(identity.clone(), descriptor)
                .is_some()
            {
                return Err(MetaQueryError::DuplicateDescriptor(identity));
            }
        }
        let descriptors = descriptor_map;
        let mut seen_descriptors = BTreeSet::new();
        let mut expansions = Vec::new();
        for result in results {
            let descriptor = descriptors
                .get(result.identity_hash())
                .ok_or_else(|| MetaQueryError::MissingDescriptor(result.identity_hash().into()))?;
            if !seen_descriptors.insert(result.identity_hash().to_owned()) {
                return Err(MetaQueryError::DuplicateResult(
                    result.identity_hash().into(),
                ));
            }
            let record = result.record();
            if record.kind == "derive" && descriptor.target.is_none() {
                return Err(MetaQueryError::MissingTarget(record.id.clone()));
            }
            if record.kind == "generator"
                && (descriptor.target.is_some() || !descriptor.introduced_bounds.is_empty())
            {
                return Err(MetaQueryError::GeneratorHasDeriveFacts(record.id.clone()));
            }
            for output in &record.outputs {
                let source = result
                    .response()
                    .output(&output.path)
                    .ok_or_else(|| MetaQueryError::MissingSource(output.path.clone()))?;
                if source.hash() != output.sha256 {
                    return Err(MetaQueryError::OutputHashMismatch(output.path.clone()));
                }
                require_formatted_source(source.bytes())?;
                let source_text = std::str::from_utf8(source.bytes())
                    .map_err(|_| MetaQueryError::InvalidSource(output.path.clone()))?;
                let mappings = source
                    .mappings()
                    .iter()
                    .map(|mapping| MetaQueryMapping {
                        generated_start: mapping.generated_start(),
                        generated_end: mapping.generated_end(),
                        origin_file: mapping.origin().file(),
                        origin_start: mapping.origin().start(),
                        origin_end: mapping.origin().end(),
                    })
                    .collect();
                expansions.push(MetaExpansion {
                    kind: record.kind.clone(),
                    id: record.id.clone(),
                    target: descriptor.target.clone(),
                    source_id: output.source_id.clone(),
                    module: output.module.clone(),
                    path: output.path.clone(),
                    source: source_text.into(),
                    provider_package: record.provider_package.clone(),
                    provider_hash: record.provider_hash.clone(),
                    entry: record.entry.clone(),
                    model_hash: record.model_hash.clone(),
                    request_hash: record.request_hash.clone(),
                    output_hash: output.sha256.clone(),
                    introduced_bounds: descriptor.introduced_bounds.clone(),
                    mappings,
                });
            }
        }
        if seen_descriptors.len() != descriptors.len() {
            let additional = descriptors
                .keys()
                .find(|identity| !seen_descriptors.contains(*identity))
                .expect("descriptor cardinality differs")
                .clone();
            return Err(MetaQueryError::AdditionalDescriptor(additional));
        }
        expansions.sort_by(|left, right| {
            (&left.kind, &left.id, &left.source_id, &left.path).cmp(&(
                &right.kind,
                &right.id,
                &right.source_id,
                &right.path,
            ))
        });
        let document = Self {
            format: META_QUERY_FORMAT.into(),
            expansions,
        };
        document.validate()?;
        Ok(document)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaQueryError> {
        let document: Self = serde_json::from_slice(bytes)
            .map_err(|error| MetaQueryError::Serialization(error.to_string()))?;
        document.validate()?;
        if document.canonical_bytes()? != bytes {
            return Err(MetaQueryError::NonCanonical);
        }
        Ok(document)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MetaQueryError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| MetaQueryError::Serialization(error.to_string()))
    }

    pub fn hash(&self) -> Result<String, MetaQueryError> {
        Ok(sha256(&self.canonical_bytes()?))
    }

    pub fn expansions(&self) -> &[MetaExpansion] {
        &self.expansions
    }

    pub fn by_producer(&self, kind: &str, id: &str) -> Vec<&MetaExpansion> {
        self.expansions
            .iter()
            .filter(|expansion| expansion.kind == kind && expansion.id == id)
            .collect()
    }

    pub fn by_output(&self, source_id: &str) -> Option<&MetaExpansion> {
        self.expansions
            .iter()
            .find(|expansion| expansion.source_id == source_id)
    }

    fn validate(&self) -> Result<(), MetaQueryError> {
        if self.format != META_QUERY_FORMAT {
            return Err(MetaQueryError::UnsupportedFormat(self.format.clone()));
        }
        let mut previous = None;
        let mut source_ids = BTreeSet::new();
        for expansion in &self.expansions {
            expansion.validate()?;
            let key = (
                expansion.kind.as_str(),
                expansion.id.as_str(),
                expansion.source_id.as_str(),
                expansion.path.as_str(),
            );
            if previous.is_some_and(|previous| previous >= key) {
                return Err(MetaQueryError::NonCanonical);
            }
            previous = Some(key);
            if !source_ids.insert(&expansion.source_id) {
                return Err(MetaQueryError::DuplicateSourceId(
                    expansion.source_id.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaExpansion {
    kind: String,
    id: String,
    target: Option<String>,
    source_id: String,
    module: String,
    path: String,
    source: String,
    provider_package: String,
    provider_hash: String,
    entry: String,
    model_hash: String,
    request_hash: String,
    output_hash: String,
    introduced_bounds: Vec<String>,
    mappings: Vec<MetaQueryMapping>,
}

impl MetaExpansion {
    pub fn kind(&self) -> &str {
        &self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn module(&self) -> &str {
        &self.module
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn provider_package(&self) -> &str {
        &self.provider_package
    }
    pub fn provider_hash(&self) -> &str {
        &self.provider_hash
    }
    pub fn entry(&self) -> &str {
        &self.entry
    }
    pub fn model_hash(&self) -> &str {
        &self.model_hash
    }
    pub fn request_hash(&self) -> &str {
        &self.request_hash
    }
    pub fn output_hash(&self) -> &str {
        &self.output_hash
    }
    pub fn introduced_bounds(&self) -> &[String] {
        &self.introduced_bounds
    }
    pub fn mappings(&self) -> &[MetaQueryMapping] {
        &self.mappings
    }

    fn validate(&self) -> Result<(), MetaQueryError> {
        if !matches!(self.kind.as_str(), "derive" | "generator") {
            return Err(MetaQueryError::InvalidExpansion("kind".into()));
        }
        for (field, value) in [
            ("id", self.id.as_str()),
            ("source_id", self.source_id.as_str()),
            ("module", self.module.as_str()),
            ("path", self.path.as_str()),
            ("provider_package", self.provider_package.as_str()),
            ("entry", self.entry.as_str()),
        ] {
            require_text(field, value)?;
        }
        if self.source.is_empty() {
            return Err(MetaQueryError::InvalidExpansion("source".into()));
        }
        for (field, value) in [
            ("provider_hash", self.provider_hash.as_str()),
            ("model_hash", self.model_hash.as_str()),
            ("request_hash", self.request_hash.as_str()),
            ("output_hash", self.output_hash.as_str()),
        ] {
            require_hash(field, value)?;
        }
        if sha256(self.source.as_bytes()) != self.output_hash {
            return Err(MetaQueryError::OutputHashMismatch(self.path.clone()));
        }
        require_formatted_source(self.source.as_bytes())?;
        if self.kind == "derive" && self.target.is_none() {
            return Err(MetaQueryError::MissingTarget(self.id.clone()));
        }
        if self.kind == "generator" && (self.target.is_some() || !self.introduced_bounds.is_empty())
        {
            return Err(MetaQueryError::GeneratorHasDeriveFacts(self.id.clone()));
        }
        if let Some(target) = &self.target {
            require_text("target", target)?;
        }
        require_unique(&self.introduced_bounds, "introduced bound")?;
        for bound in &self.introduced_bounds {
            require_text("introduced bound", bound)?;
        }
        if self
            .introduced_bounds
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(MetaQueryError::NonCanonical);
        }
        let mut previous_end = 0;
        for mapping in &self.mappings {
            mapping.validate(&self.source, previous_end)?;
            previous_end = mapping.generated_end;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaQueryMapping {
    generated_start: u32,
    generated_end: u32,
    origin_file: u32,
    origin_start: u32,
    origin_end: u32,
}

impl MetaQueryMapping {
    pub fn generated_start(&self) -> u32 {
        self.generated_start
    }
    pub fn generated_end(&self) -> u32 {
        self.generated_end
    }
    pub fn origin_file(&self) -> u32 {
        self.origin_file
    }
    pub fn origin_start(&self) -> u32 {
        self.origin_start
    }
    pub fn origin_end(&self) -> u32 {
        self.origin_end
    }

    fn validate(&self, source: &str, previous_end: u32) -> Result<(), MetaQueryError> {
        let start = usize::try_from(self.generated_start).ok();
        let end = usize::try_from(self.generated_end).ok();
        if self.generated_start > self.generated_end
            || self.generated_start < previous_end
            || start.is_none_or(|start| !source.is_char_boundary(start))
            || end.is_none_or(|end| !source.is_char_boundary(end))
            || self.origin_start > self.origin_end
        {
            return Err(MetaQueryError::InvalidSourceMap);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum MetaQueryError {
    MissingDescriptor(String),
    AdditionalDescriptor(String),
    DuplicateDescriptor(String),
    DuplicateResult(String),
    MissingTarget(String),
    GeneratorHasDeriveFacts(String),
    MissingSource(String),
    OutputHashMismatch(String),
    InvalidSource(String),
    DuplicateSourceId(String),
    InvalidExpansion(String),
    InvalidSourceMap,
    UnsupportedFormat(String),
    NonCanonical,
    Serialization(String),
}

impl fmt::Display for MetaQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingDescriptor(hash) => {
                write!(formatter, "missing meta query descriptor `{hash}`")
            }
            Self::AdditionalDescriptor(hash) => {
                write!(formatter, "unused meta query descriptor `{hash}`")
            }
            Self::DuplicateDescriptor(hash) => {
                write!(formatter, "duplicate meta query descriptor `{hash}`")
            }
            Self::DuplicateResult(hash) => {
                write!(formatter, "duplicate meta query result `{hash}`")
            }
            Self::MissingTarget(id) => write!(formatter, "derive query `{id}` has no target"),
            Self::GeneratorHasDeriveFacts(id) => write!(
                formatter,
                "generator query `{id}` contains derive-only facts"
            ),
            Self::MissingSource(path) => write!(formatter, "meta query source is missing `{path}`"),
            Self::OutputHashMismatch(path) => {
                write!(formatter, "meta query output hash mismatch `{path}`")
            }
            Self::InvalidSource(path) => write!(
                formatter,
                "meta query source is not canonical Tondo `{path}`"
            ),
            Self::DuplicateSourceId(id) => {
                write!(formatter, "duplicate meta query source ID `{id}`")
            }
            Self::InvalidExpansion(field) => write!(formatter, "invalid meta expansion `{field}`"),
            Self::InvalidSourceMap => formatter.write_str("invalid meta query source map"),
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported meta query format `{format}`")
            }
            Self::NonCanonical => formatter.write_str("meta query document is not canonical"),
            Self::Serialization(error) => write!(formatter, "meta query encoding failed: {error}"),
        }
    }
}

impl Error for MetaQueryError {}

fn require_formatted_source(bytes: &[u8]) -> Result<(), MetaQueryError> {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("generated:query").expect("static source ID is valid"),
            ModulePath::new("generated").expect("static module is valid"),
            LogicalPath::new("generated/query.to").expect("static path is valid"),
            bytes.to_vec(),
        ))
        .map_err(|_| MetaQueryError::InvalidSource("generated query".into()))?;
    let lexed = lex_with_limits(&sources, file, LexMode::Module, LexLimits::DEFAULT)
        .map_err(|_| MetaQueryError::InvalidSource("generated query".into()))?;
    if !lexed.diagnostics().is_empty() {
        return Err(MetaQueryError::InvalidSource("generated query".into()));
    }
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .map_err(|_| MetaQueryError::InvalidSource("generated query".into()))?;
    if !parsed.diagnostics().is_empty()
        || format_parsed(&sources, file, &parsed)
            .map(|formatted| formatted.bytes() != bytes)
            .unwrap_or(true)
    {
        return Err(MetaQueryError::InvalidSource("generated query".into()));
    }
    Ok(())
}

fn require_hash(field: &str, value: &str) -> Result<(), MetaQueryError> {
    validate_sha256(value).map_err(|_| MetaQueryError::InvalidExpansion(field.into()))
}

fn require_text(field: &str, value: &str) -> Result<(), MetaQueryError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        Err(MetaQueryError::InvalidExpansion(field.into()))
    } else {
        Ok(())
    }
}

fn require_unique(values: &[String], kind: &str) -> Result<(), MetaQueryError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(MetaQueryError::InvalidExpansion(format!(
                "duplicate {kind}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::COMPILER_ID;
    use crate::meta::{
        MetaLimits, MetaOutputSpec, MetaRequest, MetaSnapshot, MetaSourceMapEntry, MetaSpan,
    };
    use crate::meta_atomic::{
        MetaBuildContext, MetaInvocation, MetaProducerKind, MetaProviderIdentity,
    };

    fn accepted(kind: MetaProducerKind, id: &str, path: &str, mapped: bool) -> AcceptedMetaResult {
        let request = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [],
            [MetaOutputSpec::new(path, "generated").unwrap()],
            MetaLimits::new(10_000, 4096, 4096).unwrap(),
        )
        .unwrap();
        let provider_hash = sha256(b"provider");
        let invocation = MetaInvocation::new(
            MetaBuildContext {
                compiler: COMPILER_ID,
                edition: "0.1",
                target: "tondo-vm-hosted",
                profile: "hosted",
                capabilities: &[],
                features: &[],
            },
            MetaProviderIdentity {
                kind,
                id,
                package: "workspace:meta@1",
                hash: &provider_hash,
                entry: "meta.generate",
            },
            [],
            &request,
        )
        .unwrap();
        let source = b"fn generated(): String {\n    \"ok\"\n}\n";
        let mut builder = request.into_source_builder();
        if mapped {
            builder
                .add_mapped_source(
                    path,
                    "generated",
                    source,
                    [MetaSourceMapEntry::new(0, 2, MetaSpan::new(7, 10, 12).unwrap()).unwrap()],
                )
                .unwrap();
        } else {
            builder.add_source(path, "generated", source).unwrap();
        }
        invocation.accept(builder.finish().unwrap()).unwrap()
    }

    fn generator() -> AcceptedMetaResult {
        accepted(
            MetaProducerKind::Generator,
            "generate-schema",
            "generated/schema.to",
            true,
        )
    }

    fn derive() -> AcceptedMetaResult {
        accepted(
            MetaProducerKind::Derive,
            "derive:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "generated/derive.to",
            false,
        )
    }

    fn descriptor(
        result: &AcceptedMetaResult,
        target: Option<&str>,
        bounds: &[&str],
    ) -> MetaQueryDescriptor {
        MetaQueryDescriptor::new(result.identity_hash(), target, bounds.iter().copied()).unwrap()
    }

    fn document() -> MetaQueryDocument {
        let generator = generator();
        let derive = derive();
        MetaQueryDocument::build(
            [&generator, &derive],
            [
                descriptor(&derive, Some("app.User"), &["Z", "A"]),
                descriptor(&generator, None, &[]),
            ],
        )
        .unwrap()
    }

    #[test]
    fn query_is_canonical_complete_and_does_not_carry_unrelated_private_symbols() {
        let document = document();
        assert_eq!(document.expansions().len(), 2);
        assert!(document.hash().unwrap().starts_with("sha256:"));
        let bytes = document.canonical_bytes().unwrap();
        assert_eq!(MetaQueryDocument::decode(&bytes).unwrap(), document);
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("app.User"));
        assert!(!text.contains("app.SecretOutsideTarget"));

        let derive = document.by_producer(
            "derive",
            "derive:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        assert_eq!(derive.len(), 1);
        let expansion = derive[0];
        assert_eq!(expansion.kind(), "derive");
        assert!(expansion.id().starts_with("derive:"));
        assert_eq!(expansion.target(), Some("app.User"));
        assert_eq!(expansion.module(), "generated");
        assert_eq!(expansion.path(), "generated/derive.to");
        assert_eq!(
            expansion.source(),
            "fn generated(): String {\n    \"ok\"\n}\n"
        );
        assert_eq!(expansion.provider_package(), "workspace:meta@1");
        assert!(expansion.provider_hash().starts_with("sha256:"));
        assert_eq!(expansion.entry(), "meta.generate");
        assert!(expansion.model_hash().starts_with("sha256:"));
        assert!(expansion.request_hash().starts_with("sha256:"));
        assert!(expansion.output_hash().starts_with("sha256:"));
        assert_eq!(expansion.introduced_bounds(), ["A", "Z"]);
        assert_eq!(
            document.by_output(expansion.source_id()).unwrap(),
            expansion
        );
        assert!(document.by_output("missing").is_none());
    }

    #[test]
    fn source_map_projection_is_numeric_bounded_and_total() {
        let document = document();
        let generator = document.by_producer("generator", "generate-schema")[0];
        let mapping = generator.mappings()[0];
        assert_eq!(mapping.generated_start(), 0);
        assert_eq!(mapping.generated_end(), 2);
        assert_eq!(mapping.origin_file(), 7);
        assert_eq!(mapping.origin_start(), 10);
        assert_eq!(mapping.origin_end(), 12);
        assert!(
            MetaQueryMapping {
                generated_start: 4,
                generated_end: 5,
                origin_file: 0,
                origin_start: 0,
                origin_end: 0,
            }
            .validate("// é\n", 0)
            .is_err()
        );
    }

    #[test]
    fn descriptor_set_and_derive_only_facts_are_exact() {
        let generator = generator();
        let derive = derive();
        assert!(matches!(
            MetaQueryDocument::build([&generator], []),
            Err(MetaQueryError::MissingDescriptor(_))
        ));
        assert!(matches!(
            MetaQueryDocument::build(
                [&generator],
                [
                    descriptor(&generator, None, &[]),
                    descriptor(&derive, None, &[])
                ]
            ),
            Err(MetaQueryError::AdditionalDescriptor(_))
        ));
        assert!(matches!(
            MetaQueryDocument::build(
                [&generator, &generator],
                [descriptor(&generator, None, &[])]
            ),
            Err(MetaQueryError::DuplicateResult(_))
        ));
        let duplicate = descriptor(&generator, None, &[]);
        assert!(matches!(
            MetaQueryDocument::build([&generator], [duplicate.clone(), duplicate]),
            Err(MetaQueryError::DuplicateDescriptor(_))
        ));
        assert!(matches!(
            MetaQueryDocument::build([&derive], [descriptor(&derive, None, &[])]),
            Err(MetaQueryError::MissingTarget(_))
        ));
        assert!(matches!(
            MetaQueryDocument::build(
                [&generator],
                [descriptor(&generator, Some("app.User"), &["T"])]
            ),
            Err(MetaQueryError::GeneratorHasDeriveFacts(_))
        ));
    }

    #[test]
    fn descriptors_reject_invalid_hash_text_and_duplicate_bounds() {
        assert!(MetaQueryDescriptor::new("bad", None::<String>, ["T"]).is_err());
        assert!(
            MetaQueryDescriptor::new(
                sha256(b"identity"),
                Some("bad\nname"),
                std::iter::empty::<String>()
            )
            .is_err()
        );
        assert!(MetaQueryDescriptor::new(sha256(b"identity"), None::<String>, ["T", "T"]).is_err());
    }

    #[test]
    fn query_rejects_unformatted_or_invalid_generated_source() {
        let request = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [],
            [MetaOutputSpec::new("generated/bad.to", "generated").unwrap()],
            MetaLimits::new(100, 1000, 1000).unwrap(),
        )
        .unwrap();
        let provider_hash = sha256(b"provider");
        let invocation = MetaInvocation::new(
            MetaBuildContext {
                compiler: COMPILER_ID,
                edition: "0.1",
                target: "tondo-vm-hosted",
                profile: "hosted",
                capabilities: &[],
                features: &[],
            },
            MetaProviderIdentity {
                kind: MetaProducerKind::Generator,
                id: "generate-bad",
                package: "workspace:meta@1",
                hash: &provider_hash,
                entry: "meta.generate",
            },
            [],
            &request,
        )
        .unwrap();
        let mut builder = request.into_source_builder();
        builder
            .add_source("generated/bad.to", "generated", b"fn bad( }".to_vec())
            .unwrap();
        let result = invocation.accept(builder.finish().unwrap()).unwrap();
        assert!(matches!(
            MetaQueryDocument::build([&result], [descriptor(&result, None, &[])]),
            Err(MetaQueryError::InvalidSource(_))
        ));
    }

    #[test]
    fn decoded_documents_reject_schema_hash_order_and_mapping_drift() {
        let document = document();
        let mut value = serde_json::to_value(&document).unwrap();
        value["format"] = "future".into();
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            MetaQueryDocument::decode(&bytes),
            Err(MetaQueryError::UnsupportedFormat(_))
        ));

        let mut value = serde_json::to_value(&document).unwrap();
        value["unknown"] = true.into();
        assert!(matches!(
            MetaQueryDocument::decode(&serde_json::to_vec(&value).unwrap()),
            Err(MetaQueryError::Serialization(_))
        ));

        let mut value = serde_json::to_value(&document).unwrap();
        value["expansions"].as_array_mut().unwrap().reverse();
        assert!(matches!(
            MetaQueryDocument::decode(&serde_json::to_vec(&value).unwrap()),
            Err(MetaQueryError::NonCanonical)
        ));

        let mut value = serde_json::to_value(&document).unwrap();
        let expansions = value["expansions"].as_array_mut().unwrap();
        let generator = expansions
            .iter_mut()
            .find(|entry| entry["kind"] == "generator")
            .unwrap();
        generator["mappings"][0]["generated_end"] = 1000.into();
        assert!(matches!(
            MetaQueryDocument::decode(&serde_json::to_vec(&value).unwrap()),
            Err(MetaQueryError::InvalidSourceMap)
        ));
    }

    #[test]
    fn expansion_validation_closes_kind_hash_bounds_and_source_identity() {
        let document = document();
        let mut invalid = document.clone();
        invalid.expansions[0].kind = "macro".into();
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[0].provider_hash = "bad".into();
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[0].source.push(' ');
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[0].source = "fn broken( }".into();
        invalid.expansions[0].output_hash = sha256(invalid.expansions[0].source.as_bytes());
        assert!(matches!(
            invalid.canonical_bytes(),
            Err(MetaQueryError::InvalidSource(_))
        ));

        let derive_index = document
            .expansions
            .iter()
            .position(|expansion| expansion.kind == "derive")
            .unwrap();
        let mut invalid = document.clone();
        invalid.expansions[derive_index].target = None;
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[derive_index].introduced_bounds = vec!["Z".into(), "A".into()];
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[derive_index].introduced_bounds = vec!["T".into(), "T".into()];
        assert!(invalid.canonical_bytes().is_err());

        let mut invalid = document.clone();
        invalid.expansions[1].source_id = invalid.expansions[0].source_id.clone();
        invalid.expansions.sort_by(|left, right| {
            (&left.kind, &left.id, &left.source_id, &left.path).cmp(&(
                &right.kind,
                &right.id,
                &right.source_id,
                &right.path,
            ))
        });
        assert!(matches!(
            invalid.canonical_bytes(),
            Err(MetaQueryError::DuplicateSourceId(_))
        ));
    }

    #[test]
    fn error_vocabulary_is_actionable_and_closed() {
        let errors = [
            MetaQueryError::MissingDescriptor("x".into()),
            MetaQueryError::AdditionalDescriptor("x".into()),
            MetaQueryError::DuplicateDescriptor("x".into()),
            MetaQueryError::DuplicateResult("x".into()),
            MetaQueryError::MissingTarget("x".into()),
            MetaQueryError::GeneratorHasDeriveFacts("x".into()),
            MetaQueryError::MissingSource("x".into()),
            MetaQueryError::OutputHashMismatch("x".into()),
            MetaQueryError::InvalidSource("x".into()),
            MetaQueryError::DuplicateSourceId("x".into()),
            MetaQueryError::InvalidExpansion("x".into()),
            MetaQueryError::InvalidSourceMap,
            MetaQueryError::UnsupportedFormat("x".into()),
            MetaQueryError::NonCanonical,
            MetaQueryError::Serialization("x".into()),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
