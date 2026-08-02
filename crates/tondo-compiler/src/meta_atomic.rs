//! Complete-identity cache and atomic publication for meta products.
//!
//! Provider execution returns untrusted source responses. This module is the
//! single boundary that binds those responses to their complete invocation,
//! validates cache hits, and constructs interface/artifact bytes only after
//! every producer has succeeded.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::Serialize;

use crate::artifact::{sha256, validate_sha256};
use crate::meta::{META_MODEL, MetaLimits, MetaRequest, MetaResponse, MetaSource};
use crate::toolchain::{
    Artifact, FormatError, GenerationOutput, GenerationRecord, Interface, ModelRoot, SourceHash,
};

const META_VM_ID: &str = "tondo-meta/0.1";

/// The only two kinds of source producer admitted by Tondo 0.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MetaProducerKind {
    Derive,
    Generator,
}

impl MetaProducerKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Derive => "derive",
            Self::Generator => "generator",
        }
    }
}

/// Complete, canonical identity of one meta invocation before it executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetaInvocation {
    compiler: String,
    meta_vm: String,
    edition: String,
    target: String,
    profile: String,
    capabilities: Vec<String>,
    features: Vec<String>,
    kind: MetaProducerKind,
    id: String,
    provider_package: String,
    provider_hash: String,
    entry: String,
    model_roots: Vec<ModelRoot>,
    model_hash: String,
    payload_hash: String,
    outputs: Vec<InvocationOutput>,
    limits: MetaLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct InvocationOutput {
    path: String,
    module: String,
}

/// Stable toolchain and target inputs shared by a group of invocations.
#[derive(Debug, Clone, Copy)]
pub struct MetaBuildContext<'a> {
    pub compiler: &'a str,
    pub edition: &'a str,
    pub target: &'a str,
    pub profile: &'a str,
    pub capabilities: &'a [String],
    pub features: &'a [String],
}

/// Locked provider identity for one invocation.
#[derive(Debug, Clone, Copy)]
pub struct MetaProviderIdentity<'a> {
    pub kind: MetaProducerKind,
    pub id: &'a str,
    pub package: &'a str,
    pub hash: &'a str,
    pub entry: &'a str,
}

impl MetaInvocation {
    pub fn new(
        context: MetaBuildContext<'_>,
        provider: MetaProviderIdentity<'_>,
        model_roots: impl IntoIterator<Item = ModelRoot>,
        request: &MetaRequest,
    ) -> Result<Self, MetaAtomicError> {
        let mut capabilities = context.capabilities.to_vec();
        let mut features = context.features.to_vec();
        let mut model_roots = model_roots.into_iter().collect::<Vec<_>>();
        capabilities.sort();
        features.sort();
        model_roots.sort_by(|left, right| {
            (&left.package, &left.module).cmp(&(&right.package, &right.module))
        });
        require_unique(&capabilities, "capability")?;
        require_unique(&features, "feature")?;
        require_unique_by(
            &model_roots,
            |root| (&root.package, &root.module),
            "model root",
        )?;
        for (field, value) in [
            ("compiler", context.compiler),
            ("edition", context.edition),
            ("target", context.target),
            ("profile", context.profile),
            ("producer id", provider.id),
            ("provider package", provider.package),
            ("provider entry", provider.entry),
        ] {
            require_text(field, value)?;
        }
        validate_sha256(provider.hash)
            .map_err(|_| MetaAtomicError::InvalidIdentity("provider hash".into()))?;
        let outputs = request
            .outputs()
            .iter()
            .map(|output| InvocationOutput {
                path: output.path().to_owned(),
                module: output.module().to_owned(),
            })
            .collect::<Vec<_>>();
        if outputs.is_empty() {
            return Err(MetaAtomicError::InvalidIdentity(
                "invocation has no outputs".into(),
            ));
        }
        let invocation = Self {
            compiler: context.compiler.into(),
            meta_vm: META_VM_ID.into(),
            edition: context.edition.into(),
            target: context.target.into(),
            profile: context.profile.into(),
            capabilities,
            features,
            kind: provider.kind,
            id: provider.id.into(),
            provider_package: provider.package.into(),
            provider_hash: provider.hash.into(),
            entry: provider.entry.into(),
            model_roots,
            model_hash: request.snapshot().hash().map_err(|error| {
                MetaAtomicError::InvalidIdentity(format!("model hash: {error}"))
            })?,
            payload_hash: request.hash().map_err(|error| {
                MetaAtomicError::InvalidIdentity(format!("request hash: {error}"))
            })?,
            outputs,
            limits: request.limits(),
        };
        invocation.validate_id()?;
        Ok(invocation)
    }

    pub fn identity_hash(&self) -> Result<String, MetaAtomicError> {
        canonical_hash(self)
    }

    pub fn payload_hash(&self) -> &str {
        &self.payload_hash
    }

    fn validate_id(&self) -> Result<(), MetaAtomicError> {
        let valid = match self.kind {
            MetaProducerKind::Generator => is_kebab(&self.id),
            MetaProducerKind::Derive => self.id.strip_prefix("derive:").is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            }),
        };
        if valid {
            Ok(())
        } else {
            Err(MetaAtomicError::InvalidIdentity(format!(
                "invalid {} id `{}`",
                self.kind.as_str(),
                self.id
            )))
        }
    }

    /// Bind a provider response to this identity and calculate final hashes.
    pub fn accept(&self, response: MetaResponse) -> Result<AcceptedMetaResult, MetaAtomicError> {
        let response = MetaResponse::decode(&response.canonical_bytes()?)?;
        let expected = self
            .outputs
            .iter()
            .map(|output| (output.path.as_str(), output.module.as_str()))
            .collect::<Vec<_>>();
        let actual = response
            .outputs()
            .iter()
            .map(|output| (output.path(), output.module()))
            .collect::<Vec<_>>();
        if expected != actual {
            return Err(MetaAtomicError::OutputManifestMismatch(self.id.clone()));
        }
        let identity_hash = self.identity_hash()?;
        let outputs = response
            .outputs()
            .iter()
            .map(|source| GenerationOutput {
                source_id: generated_source_id(&identity_hash, source.path()),
                module: source.module().into(),
                path: source.path().into(),
                sha256: source.hash().into(),
            })
            .collect();
        Ok(AcceptedMetaResult {
            identity_hash: identity_hash.clone(),
            response_hash: response.hash()?,
            record: GenerationRecord {
                kind: self.kind.as_str().into(),
                id: self.id.clone(),
                provider_package: self.provider_package.clone(),
                provider_hash: self.provider_hash.clone(),
                entry: self.entry.clone(),
                model_roots: self.model_roots.clone(),
                model_hash: self.model_hash.clone(),
                request_hash: identity_hash,
                outputs,
            },
            response,
        })
    }
}

/// A response whose source bytes and generation record are inseparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedMetaResult {
    identity_hash: String,
    response_hash: String,
    record: GenerationRecord,
    response: MetaResponse,
}

impl AcceptedMetaResult {
    pub fn identity_hash(&self) -> &str {
        &self.identity_hash
    }

    pub fn response_hash(&self) -> &str {
        &self.response_hash
    }

    pub fn record(&self) -> &GenerationRecord {
        &self.record
    }

    pub fn response(&self) -> &MetaResponse {
        &self.response
    }
}

/// In-memory cache model with fail-closed hit validation.
#[derive(Debug, Default)]
pub struct MetaResultCache {
    entries: BTreeMap<String, AcceptedMetaResult>,
}

impl MetaResultCache {
    pub fn lookup(
        &self,
        invocation: &MetaInvocation,
    ) -> Result<Option<AcceptedMetaResult>, MetaAtomicError> {
        let key = invocation.identity_hash()?;
        let Some(entry) = self.entries.get(&key) else {
            return Ok(None);
        };
        let accepted =
            invocation.accept(MetaResponse::decode(&entry.response.canonical_bytes()?)?)?;
        if &accepted != entry {
            return Err(MetaAtomicError::CorruptCacheEntry(key));
        }
        Ok(Some(accepted))
    }

    pub fn insert(&mut self, result: AcceptedMetaResult) -> Result<(), MetaAtomicError> {
        match self.entries.get(&result.identity_hash) {
            Some(existing) if existing != &result => Err(MetaAtomicError::CacheCollision(
                result.identity_hash.clone(),
            )),
            Some(_) => Ok(()),
            None => {
                self.entries.insert(result.identity_hash.clone(), result);
                Ok(())
            }
        }
    }
}

/// Transaction that owns all generated sources until final products validate.
#[derive(Debug, Default)]
pub struct MetaProductTransaction {
    results: BTreeMap<(String, String), AcceptedMetaResult>,
    output_paths: BTreeSet<String>,
}

impl MetaProductTransaction {
    pub fn stage(&mut self, result: AcceptedMetaResult) -> Result<(), MetaAtomicError> {
        let key = (result.record.kind.clone(), result.record.id.clone());
        if self.results.contains_key(&key) {
            return Err(MetaAtomicError::DuplicateProducer {
                kind: key.0,
                id: key.1,
            });
        }
        let new_paths = result
            .record
            .outputs
            .iter()
            .map(|output| output.path.clone())
            .collect::<BTreeSet<_>>();
        for output in &result.record.outputs {
            if self.output_paths.contains(&output.path) {
                return Err(MetaAtomicError::OutputCollision(output.path.clone()));
            }
        }
        self.output_paths.extend(new_paths);
        self.results.insert(key, result);
        Ok(())
    }

    /// Validate and return all products at once. No caller-visible bytes exist
    /// on any error path.
    pub fn finish(
        self,
        mut interface: Interface,
        mut artifact: Artifact,
    ) -> Result<MetaProducts, MetaAtomicError> {
        if self.results.is_empty() {
            return Err(MetaAtomicError::EmptyTransaction);
        }
        if interface.package_id != artifact.package_id
            || interface.edition != artifact.edition
            || interface.target != artifact.target
            || interface.profile != artifact.profile
            || interface.capabilities != artifact.capabilities
            || interface.features != artifact.features
            || interface.source_sets != artifact.source_sets
        {
            return Err(MetaAtomicError::ProductIdentityMismatch);
        }

        let mut generation = Vec::with_capacity(self.results.len());
        let mut generated_sources = Vec::new();
        for result in self.results.into_values() {
            generation.push(result.record);
            generated_sources.extend(result.response.outputs().iter().cloned());
        }
        let generation_hash = canonical_hash(&generation)?;
        interface.meta_model = Some(META_MODEL.into());
        interface.generation_hash = generation_hash;
        let interface_bytes = interface.encode()?;

        artifact.meta_model = Some(META_MODEL.into());
        artifact.generation = generation;
        let mut source_keys = artifact
            .source_hashes
            .iter()
            .map(|source| {
                (
                    source.source_id.clone(),
                    source.module.clone(),
                    source.path.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        for ((record, output), source) in artifact
            .generation
            .iter()
            .flat_map(|record| record.outputs.iter().map(move |output| (record, output)))
            .zip(generated_sources.iter())
        {
            let Some(matched) = record
                .outputs
                .iter()
                .find(|output| output.path == source.path())
            else {
                return Err(MetaAtomicError::ProductSourceMismatch(source.path().into()));
            };
            if matched != output {
                return Err(MetaAtomicError::ProductSourceMismatch(source.path().into()));
            }
            let key = (
                output.source_id.clone(),
                source.module().to_owned(),
                source.path().to_owned(),
            );
            if !source_keys.insert(key.clone()) {
                return Err(MetaAtomicError::ProductSourceCollision(key.2));
            }
            artifact.source_hashes.push(SourceHash {
                source_id: key.0,
                module: key.1,
                path: key.2,
                sha256: source.hash().into(),
            });
        }
        artifact.source_hashes.sort_by(|left, right| {
            (&left.source_id, &left.module, &left.path).cmp(&(
                &right.source_id,
                &right.module,
                &right.path,
            ))
        });
        artifact.interface_hash = interface.content_hash()?;
        artifact.build_hash = artifact.calculated_build_hash()?;
        let artifact_bytes = artifact.encode()?;
        Ok(MetaProducts {
            generated_sources,
            interface_bytes,
            artifact_bytes,
        })
    }
}

/// Complete publication unit. Filesystem publication, if any, receives this
/// value only after all three members have been validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaProducts {
    generated_sources: Vec<MetaSource>,
    interface_bytes: Vec<u8>,
    artifact_bytes: Vec<u8>,
}

impl MetaProducts {
    pub fn generated_sources(&self) -> &[MetaSource] {
        &self.generated_sources
    }

    pub fn interface_bytes(&self) -> &[u8] {
        &self.interface_bytes
    }

    pub fn artifact_bytes(&self) -> &[u8] {
        &self.artifact_bytes
    }
}

#[derive(Debug)]
pub enum MetaAtomicError {
    InvalidIdentity(String),
    OutputManifestMismatch(String),
    CorruptCacheEntry(String),
    CacheCollision(String),
    DuplicateProducer { kind: String, id: String },
    OutputCollision(String),
    EmptyTransaction,
    ProductIdentityMismatch,
    ProductSourceMismatch(String),
    ProductSourceCollision(String),
    Meta(crate::meta::MetaContractError),
    Toolchain(FormatError),
    Serialization(String),
}

impl fmt::Display for MetaAtomicError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity(reason) => write!(formatter, "invalid meta identity: {reason}"),
            Self::OutputManifestMismatch(id) => {
                write!(formatter, "meta output manifest mismatch for `{id}`")
            }
            Self::CorruptCacheEntry(hash) => write!(formatter, "corrupt meta cache entry `{hash}`"),
            Self::CacheCollision(hash) => write!(formatter, "meta cache collision `{hash}`"),
            Self::DuplicateProducer { kind, id } => {
                write!(formatter, "duplicate {kind} producer `{id}`")
            }
            Self::OutputCollision(path) => write!(formatter, "meta output collision `{path}`"),
            Self::EmptyTransaction => formatter.write_str("meta transaction has no producers"),
            Self::ProductIdentityMismatch => {
                formatter.write_str("meta interface and artifact identities differ")
            }
            Self::ProductSourceMismatch(path) => {
                write!(formatter, "meta product source mismatch `{path}`")
            }
            Self::ProductSourceCollision(path) => {
                write!(formatter, "meta product source collision `{path}`")
            }
            Self::Meta(error) => error.fmt(formatter),
            Self::Toolchain(error) => error.fmt(formatter),
            Self::Serialization(error) => {
                write!(formatter, "meta identity encoding failed: {error}")
            }
        }
    }
}

impl Error for MetaAtomicError {}

impl From<crate::meta::MetaContractError> for MetaAtomicError {
    fn from(error: crate::meta::MetaContractError) -> Self {
        Self::Meta(error)
    }
}

impl From<FormatError> for MetaAtomicError {
    fn from(error: FormatError) -> Self {
        Self::Toolchain(error)
    }
}

fn canonical_hash(value: &impl Serialize) -> Result<String, MetaAtomicError> {
    serde_json::to_vec(value)
        .map(|bytes| sha256(&bytes))
        .map_err(|error| MetaAtomicError::Serialization(error.to_string()))
}

fn generated_source_id(identity_hash: &str, path: &str) -> String {
    sha256(format!("{identity_hash}\0{path}").as_bytes()).replacen("sha256:", "gen:", 1)
}

fn require_text(field: &str, value: &str) -> Result<(), MetaAtomicError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        Err(MetaAtomicError::InvalidIdentity(field.into()))
    } else {
        Ok(())
    }
}

fn require_unique(values: &[String], kind: &str) -> Result<(), MetaAtomicError> {
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(MetaAtomicError::InvalidIdentity(format!(
            "duplicate {kind}"
        )))
    } else {
        Ok(())
    }
}

fn require_unique_by<'a, T, K: Ord + 'a>(
    values: &'a [T],
    key: impl Fn(&'a T) -> K,
    kind: &str,
) -> Result<(), MetaAtomicError> {
    if values.windows(2).any(|pair| key(&pair[0]) == key(&pair[1])) {
        Err(MetaAtomicError::InvalidIdentity(format!(
            "duplicate {kind}"
        )))
    } else {
        Ok(())
    }
}

fn is_kebab(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{CAPABILITY_REGISTRY, COMPILER_ID};
    use crate::meta::{MetaOutputSpec, MetaSnapshot};
    use crate::toolchain::{ARTIFACT_FORMAT, INTERFACE_FORMAT};

    fn hash(label: &str) -> String {
        sha256(label.as_bytes())
    }

    fn request(path: &str, module: &str) -> MetaRequest {
        MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [],
            [MetaOutputSpec::new(path, module).unwrap()],
            MetaLimits::new(10_000, 4096, 4096).unwrap(),
        )
        .unwrap()
    }

    fn response(request: &MetaRequest, source: &[u8]) -> MetaResponse {
        let path = request.outputs()[0].path().to_owned();
        let module = request.outputs()[0].module().to_owned();
        let mut builder = request.clone().into_source_builder();
        builder.add_source(path, module, source).unwrap();
        builder.finish().unwrap()
    }

    fn invocation_for(id: &str, path: &str, module: &str) -> (MetaInvocation, MetaRequest) {
        let request = request(path, module);
        let capabilities = vec!["process".into(), "console".into()];
        let features = vec!["zeta".into(), "alpha".into()];
        let invocation = MetaInvocation::new(
            MetaBuildContext {
                compiler: COMPILER_ID,
                edition: "0.1",
                target: "tondo-vm-hosted",
                profile: "hosted",
                capabilities: &capabilities,
                features: &features,
            },
            MetaProviderIdentity {
                kind: MetaProducerKind::Generator,
                id,
                package: "workspace:meta@1",
                hash: &hash("provider"),
                entry: "schema.generate",
            },
            [ModelRoot {
                package: "workspace:app@1".into(),
                module: "app".into(),
            }],
            &request,
        )
        .unwrap();
        (invocation, request)
    }

    fn accepted(id: &str, path: &str) -> AcceptedMetaResult {
        let (invocation, request) = invocation_for(id, path, "generated");
        invocation
            .accept(response(&request, b"fn generated(): Unit {}\n"))
            .unwrap()
    }

    fn interface() -> Interface {
        Interface {
            format: INTERFACE_FORMAT.into(),
            compiler: COMPILER_ID.into(),
            edition: "0.1".into(),
            package_id: "workspace:app@1".into(),
            target: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capability_registry: CAPABILITY_REGISTRY.into(),
            capabilities: vec!["console".into()],
            features: vec![],
            meta_model: None,
            source_sets: vec!["@15:workspace:app@1#common".into()],
            modules: vec!["app".into()],
            generation_hash: sha256(&[]),
            api_hash: hash("api"),
            dependencies: vec![],
        }
    }

    fn artifact() -> Artifact {
        let mut artifact = Artifact {
            format: ARTIFACT_FORMAT.into(),
            compiler: COMPILER_ID.into(),
            edition: "0.1".into(),
            source_form: "module".into(),
            package_id: "workspace:app@1".into(),
            target: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capability_registry: CAPABILITY_REGISTRY.into(),
            capabilities: vec!["console".into()],
            features: vec![],
            meta_model: None,
            source_sets: vec!["@15:workspace:app@1#common".into()],
            manifest_hash: hash("manifest"),
            lockfile_hash: hash("lockfile"),
            generator_inputs: BTreeMap::new(),
            generation: vec![],
            source_hashes: vec![SourceHash {
                source_id: "pkg:main".into(),
                module: "app".into(),
                path: "src/main.to".into(),
                sha256: hash("source"),
            }],
            interface_hash: hash("interface"),
            build_hash: hash("build"),
            reproducible: true,
        };
        artifact.build_hash = artifact.calculated_build_hash().unwrap();
        artifact
    }

    #[test]
    fn complete_identity_is_canonical_and_binds_final_outputs() {
        let (invocation, request) =
            invocation_for("generate-schema", "generated/schema.to", "schema");
        assert_eq!(invocation.payload_hash(), request.hash().unwrap());
        assert!(invocation.identity_hash().unwrap().starts_with("sha256:"));
        let accepted = invocation
            .accept(response(&request, b"fn generated(): Unit {}\n"))
            .unwrap();
        assert_eq!(accepted.identity_hash(), accepted.record().request_hash);
        assert_eq!(
            accepted.response_hash(),
            accepted.response().hash().unwrap()
        );
        assert_eq!(accepted.record().kind, "generator");
        assert_eq!(accepted.record().outputs.len(), 1);
        assert_eq!(
            accepted.record().outputs[0].sha256,
            accepted.response().outputs()[0].hash()
        );

        let mut reordered = invocation.clone();
        reordered.capabilities.reverse();
        assert_ne!(
            invocation.identity_hash().unwrap(),
            reordered.identity_hash().unwrap(),
            "mutating an already canonical identity must be observable"
        );
    }

    #[test]
    fn cache_hits_are_verified_and_conflicts_fail_closed() {
        let (invocation, request) = invocation_for("generate-schema", "generated/a.to", "a");
        let accepted = invocation
            .accept(response(&request, b"fn a(): Unit {}\n"))
            .unwrap();
        let mut cache = MetaResultCache::default();
        assert!(cache.lookup(&invocation).unwrap().is_none());
        cache.insert(accepted.clone()).unwrap();
        cache.insert(accepted.clone()).unwrap();
        assert_eq!(cache.lookup(&invocation).unwrap(), Some(accepted.clone()));

        let mut conflicting = accepted.clone();
        conflicting.response_hash = hash("other");
        assert!(matches!(
            cache.insert(conflicting),
            Err(MetaAtomicError::CacheCollision(_))
        ));
        cache
            .entries
            .get_mut(accepted.identity_hash())
            .unwrap()
            .response_hash = hash("corrupt");
        assert!(matches!(
            cache.lookup(&invocation),
            Err(MetaAtomicError::CorruptCacheEntry(_))
        ));
    }

    #[test]
    fn transaction_publishes_sources_interface_and_artifact_together() {
        let mut transaction = MetaProductTransaction::default();
        transaction
            .stage(accepted("generate-a", "generated/a.to"))
            .unwrap();
        transaction
            .stage(accepted("generate-b", "generated/b.to"))
            .unwrap();
        let products = transaction.finish(interface(), artifact()).unwrap();
        assert_eq!(products.generated_sources().len(), 2);
        let interface = Interface::decode(products.interface_bytes()).unwrap();
        let artifact = Artifact::decode(products.artifact_bytes()).unwrap();
        assert_eq!(interface.meta_model.as_deref(), Some(META_MODEL));
        assert_eq!(artifact.meta_model.as_deref(), Some(META_MODEL));
        assert_eq!(artifact.generation.len(), 2);
        assert_eq!(artifact.source_hashes.len(), 3);
        assert_eq!(artifact.interface_hash, interface.content_hash().unwrap());
        assert_eq!(
            artifact.build_hash,
            artifact.calculated_build_hash().unwrap()
        );
    }

    #[test]
    fn invalid_identities_and_manifests_are_rejected_before_staging() {
        let meta_request = request("generated/a.to", "a");
        let capabilities = vec!["console".into(), "console".into()];
        let base = MetaBuildContext {
            compiler: COMPILER_ID,
            edition: "0.1",
            target: "tondo-vm-hosted",
            profile: "hosted",
            capabilities: &capabilities,
            features: &[],
        };
        let provider = MetaProviderIdentity {
            kind: MetaProducerKind::Generator,
            id: "Bad_Id",
            package: "workspace:meta@1",
            hash: "bad",
            entry: "schema.generate",
        };
        assert!(MetaInvocation::new(base, provider, [], &meta_request).is_err());

        let capabilities = Vec::new();
        let mut provider = provider;
        let provider_hash = hash("provider");
        provider.hash = &provider_hash;
        let base = MetaBuildContext {
            capabilities: &capabilities,
            ..base
        };
        assert!(MetaInvocation::new(base, provider, [], &meta_request).is_err());
        provider.kind = MetaProducerKind::Derive;
        provider.id = "derive:not-a-hash";
        assert!(MetaInvocation::new(base, provider, [], &meta_request).is_err());
        provider.id = "derive:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert!(MetaInvocation::new(base, provider, [], &meta_request).is_ok());

        let empty = MetaRequest::new(
            MetaSnapshot::new([], [], []).unwrap(),
            [],
            [],
            MetaLimits::new(1, 1, 1).unwrap(),
        )
        .unwrap();
        assert!(MetaInvocation::new(base, provider, [], &empty).is_err());

        let (invocation, _) = invocation_for("generate-schema", "generated/a.to", "a");
        let other = request("generated/b.to", "b");
        assert!(matches!(
            invocation.accept(response(&other, b"fn b(): Unit {}\n")),
            Err(MetaAtomicError::OutputManifestMismatch(_))
        ));
    }

    #[test]
    fn transaction_rejects_duplicates_collisions_and_incompatible_products() {
        let first = accepted("generate-a", "generated/a.to");
        let mut transaction = MetaProductTransaction::default();
        transaction.stage(first.clone()).unwrap();
        assert!(matches!(
            transaction.stage(first),
            Err(MetaAtomicError::DuplicateProducer { .. })
        ));

        let mut collision = accepted("generate-b", "generated/b.to");
        collision.record.outputs[0].path = "generated/a.to".into();
        let mut leading = collision.record.outputs[0].clone();
        leading.path = "generated/leading.to".into();
        collision.record.outputs.insert(0, leading);
        assert!(matches!(
            transaction.stage(collision),
            Err(MetaAtomicError::OutputCollision(_))
        ));
        assert!(!transaction.output_paths.contains("generated/leading.to"));

        assert!(matches!(
            MetaProductTransaction::default().finish(interface(), artifact()),
            Err(MetaAtomicError::EmptyTransaction)
        ));
        let mut bad_interface = interface();
        bad_interface.profile = "other".into();
        let mut transaction = MetaProductTransaction::default();
        transaction
            .stage(accepted("generate-c", "generated/c.to"))
            .unwrap();
        assert!(matches!(
            transaction.finish(bad_interface, artifact()),
            Err(MetaAtomicError::ProductIdentityMismatch)
        ));
    }

    #[test]
    fn product_source_integrity_and_error_vocabulary_are_closed() {
        let mut mismatched = accepted("generate-a", "generated/a.to");
        mismatched.record.outputs[0].path = "generated/other.to".into();
        let mut transaction = MetaProductTransaction::default();
        transaction.stage(mismatched).unwrap();
        assert!(matches!(
            transaction.finish(interface(), artifact()),
            Err(MetaAtomicError::ProductSourceMismatch(_))
        ));

        let accepted = accepted("generate-a", "generated/a.to");
        let output = &accepted.record.outputs[0];
        let mut artifact = artifact();
        artifact.source_hashes.push(SourceHash {
            source_id: output.source_id.clone(),
            module: output.module.clone(),
            path: output.path.clone(),
            sha256: output.sha256.clone(),
        });
        artifact.source_hashes.sort_by(|left, right| {
            (&left.source_id, &left.module, &left.path).cmp(&(
                &right.source_id,
                &right.module,
                &right.path,
            ))
        });
        let mut transaction = MetaProductTransaction::default();
        transaction.stage(accepted).unwrap();
        assert!(matches!(
            transaction.finish(interface(), artifact),
            Err(MetaAtomicError::ProductSourceCollision(_))
        ));

        let errors = [
            MetaAtomicError::InvalidIdentity("x".into()),
            MetaAtomicError::OutputManifestMismatch("x".into()),
            MetaAtomicError::CorruptCacheEntry("x".into()),
            MetaAtomicError::CacheCollision("x".into()),
            MetaAtomicError::DuplicateProducer {
                kind: "derive".into(),
                id: "x".into(),
            },
            MetaAtomicError::OutputCollision("x".into()),
            MetaAtomicError::EmptyTransaction,
            MetaAtomicError::ProductIdentityMismatch,
            MetaAtomicError::ProductSourceMismatch("x".into()),
            MetaAtomicError::ProductSourceCollision("x".into()),
            MetaAtomicError::Serialization("x".into()),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
