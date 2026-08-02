//! Atomic one-round execution for manifest-declared generators.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use tondo_vm::runtime::{RuntimeValue, VmOutcome};

use crate::artifact::sha256;
use crate::meta::{
    META_MODEL, MetaContractError, MetaInput, MetaLimits, MetaModelError, MetaOutputSpec,
    MetaRequest, MetaResponse, MetaRoot, MetaSnapshot,
};
use crate::meta_vm::{MetaVmArtifact, MetaVmError, MetaVmLimits};
use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
use crate::syntax::{
    LexLimits, LexMode, ParseLimits, ParseMode, format_parsed, lex_with_limits, parse,
};
use crate::toolchain::{LockedGenerator, LockedNamedInput};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProviderKey {
    package: String,
    entry: String,
    hash: String,
}

impl ProviderKey {
    fn for_generator(generator: &LockedGenerator) -> Result<Self, GeneratorExecutionError> {
        required("provider package", &generator.provider_package)?;
        required("provider entry", &generator.entry)?;
        require_hash("provider hash", &generator.provider_hash)?;
        Ok(Self {
            package: generator.provider_package.clone(),
            entry: generator.entry.clone(),
            hash: generator.provider_hash.clone(),
        })
    }
}

/// Exact immutable values visible while compiling one generator provider.
#[derive(Debug, Clone, Copy)]
pub struct GeneratorProviderRequest<'a> {
    generator_id: &'a str,
    owner_package: &'a str,
    provider_package: &'a str,
    entry: &'a str,
    provider_hash: &'a str,
    request: &'a MetaRequest,
}

impl<'a> GeneratorProviderRequest<'a> {
    pub fn generator_id(&self) -> &'a str {
        self.generator_id
    }

    pub fn owner_package(&self) -> &'a str {
        self.owner_package
    }

    pub fn provider_package(&self) -> &'a str {
        self.provider_package
    }

    pub fn entry(&self) -> &'a str {
        self.entry
    }

    pub fn provider_hash(&self) -> &'a str {
        self.provider_hash
    }

    pub fn meta_request(&self) -> &'a MetaRequest {
        self.request
    }
}

/// Trusted compiler boundary. User generator code runs only as the returned
/// artifact after the orchestrator reloads it under the locked budgets.
pub trait GeneratorProviderCompiler: Send + Sync {
    fn compile(&self, request: GeneratorProviderRequest<'_>) -> Result<MetaVmArtifact, String>;
}

#[derive(Default)]
pub struct GeneratorProviderRegistry {
    providers: BTreeMap<ProviderKey, Arc<dyn GeneratorProviderCompiler>>,
}

impl GeneratorProviderRegistry {
    pub fn insert_for(
        &mut self,
        generator: &LockedGenerator,
        provider: Arc<dyn GeneratorProviderCompiler>,
    ) -> Result<(), GeneratorExecutionError> {
        let key = ProviderKey::for_generator(generator)?;
        if self.providers.insert(key.clone(), provider).is_some() {
            return Err(GeneratorExecutionError::DuplicateProvider(format!(
                "{}::{}@{}",
                key.package, key.entry, key.hash
            )));
        }
        Ok(())
    }

    fn get(
        &self,
        generator: &LockedGenerator,
    ) -> Result<&Arc<dyn GeneratorProviderCompiler>, GeneratorExecutionError> {
        let key = ProviderKey::for_generator(generator)?;
        self.providers
            .get(&key)
            .ok_or_else(|| GeneratorExecutionError::MissingProvider(generator.id.clone()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorResult {
    generator_id: String,
    provider_hash: String,
    response: MetaResponse,
}

impl GeneratorResult {
    pub fn generator_id(&self) -> &str {
        &self.generator_id
    }

    pub fn provider_hash(&self) -> &str {
        &self.provider_hash
    }

    pub fn response(&self) -> &MetaResponse {
        &self.response
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorExecution {
    results: Vec<GeneratorResult>,
}

impl GeneratorExecution {
    pub fn results(&self) -> &[GeneratorResult] {
        &self.results
    }

    pub fn into_results(self) -> Vec<GeneratorResult> {
        self.results
    }
}

/// Execute every generator exactly once and return nothing unless the complete
/// one-round plan succeeds.
pub fn execute_generator_plan(
    generators: &[LockedGenerator],
    locked_inputs: &[LockedNamedInput],
    input_values: &BTreeMap<String, Vec<u8>>,
    snapshots: &BTreeMap<String, MetaSnapshot>,
    registry: &GeneratorProviderRegistry,
) -> Result<GeneratorExecution, GeneratorExecutionError> {
    validate_input_catalog(locked_inputs, input_values)?;
    let mut ordered = generators.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));
    ensure_unique(
        ordered.iter().map(|generator| generator.id.as_str()),
        "generator",
    )?;

    let expected_snapshot_ids = ordered
        .iter()
        .map(|generator| generator.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual_snapshot_ids = snapshots
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if expected_snapshot_ids != actual_snapshot_ids {
        return Err(GeneratorExecutionError::SnapshotSet);
    }

    let mut paths = BTreeSet::new();
    for generator in &ordered {
        validate_generator(generator, locked_inputs)?;
        for output in &generator.outputs {
            if !paths.insert(output.logical_path.as_str()) {
                return Err(GeneratorExecutionError::OutputCollision(
                    output.logical_path.clone(),
                ));
            }
        }
    }

    let mut results = Vec::with_capacity(ordered.len());
    for generator in ordered {
        let snapshot = snapshots
            .get(&generator.id)
            .expect("the exact snapshot set was validated")
            .clone();
        let expected_roots = generator
            .model_roots
            .iter()
            .map(|root| MetaRoot::new(root.package.clone(), root.module.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let expected_roots = MetaSnapshot::new(expected_roots, [], [])?;
        if snapshot.roots() != expected_roots.roots() {
            return Err(GeneratorExecutionError::SnapshotRoots(generator.id.clone()));
        }

        let inputs = generator
            .inputs
            .iter()
            .map(|name| {
                MetaInput::new(
                    name.clone(),
                    input_values
                        .get(name)
                        .expect("generator inputs were validated")
                        .clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let outputs = generator
            .outputs
            .iter()
            .map(|output| MetaOutputSpec::new(output.logical_path.clone(), output.module.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let limits = MetaLimits::new(
            generator.limits.steps,
            generator.limits.memory_bytes,
            generator.limits.output_bytes,
        )?;
        let request = MetaRequest::new(snapshot, inputs, outputs, limits)?;
        let provider = registry.get(generator)?;
        let artifact = provider
            .compile(GeneratorProviderRequest {
                generator_id: &generator.id,
                owner_package: &generator.owner_package,
                provider_package: &generator.provider_package,
                entry: &generator.entry,
                provider_hash: &generator.provider_hash,
                request: &request,
            })
            .map_err(|message| GeneratorExecutionError::ProviderFailed {
                generator: generator.id.clone(),
                message,
            })?;
        let execution = artifact
            .load(MetaVmLimits::for_request(limits))
            .and_then(|program| program.run_with_output_meter(measure_generator_output))
            .map_err(|source| GeneratorExecutionError::ProviderVm {
                generator: generator.id.clone(),
                source,
            })?;
        let VmOutcome::Returned(RuntimeValue::String(encoded)) = execution.outcome else {
            return Err(GeneratorExecutionError::InvalidProviderResult(
                generator.id.clone(),
            ));
        };
        let response = MetaResponse::decode(encoded.as_bytes()).map_err(|error| {
            GeneratorExecutionError::ProviderVm {
                generator: generator.id.clone(),
                source: MetaVmError::StructuredOutput(error.to_string()),
            }
        })?;
        let response = validate_and_format_response(request, response)?;
        results.push(GeneratorResult {
            generator_id: generator.id.clone(),
            provider_hash: generator.provider_hash.clone(),
            response,
        });
    }

    Ok(GeneratorExecution { results })
}

fn validate_input_catalog(
    locked_inputs: &[LockedNamedInput],
    values: &BTreeMap<String, Vec<u8>>,
) -> Result<(), GeneratorExecutionError> {
    ensure_unique(
        locked_inputs.iter().map(|input| input.name.as_str()),
        "input",
    )?;
    let expected = locked_inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect::<BTreeSet<_>>();
    let actual = values.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(GeneratorExecutionError::InputSet);
    }
    for input in locked_inputs {
        input
            .validate("locked generator input")
            .map_err(|error| GeneratorExecutionError::InvalidPlan(error.to_string()))?;
        required("input name", &input.name)?;
        require_hash("input hash", &input.sha256)?;
        if sha256(
            values
                .get(&input.name)
                .expect("the exact input set was validated"),
        ) != input.sha256
        {
            return Err(GeneratorExecutionError::InputHash(input.name.clone()));
        }
    }
    Ok(())
}

fn validate_generator(
    generator: &LockedGenerator,
    locked_inputs: &[LockedNamedInput],
) -> Result<(), GeneratorExecutionError> {
    generator
        .validate()
        .map_err(|error| GeneratorExecutionError::InvalidPlan(error.to_string()))?;
    required("generator id", &generator.id)?;
    required("owner package", &generator.owner_package)?;
    ProviderKey::for_generator(generator)?;
    if generator.meta_model != META_MODEL {
        return Err(GeneratorExecutionError::InvalidPlan(format!(
            "generator `{}` uses unsupported meta model `{}`",
            generator.id, generator.meta_model
        )));
    }
    if generator.outputs.is_empty() {
        return Err(GeneratorExecutionError::InvalidPlan(format!(
            "generator `{}` has no outputs",
            generator.id
        )));
    }
    ensure_unique(
        generator.inputs.iter().map(String::as_str),
        "generator input",
    )?;
    let available = locked_inputs
        .iter()
        .map(|input| input.name.as_str())
        .collect::<BTreeSet<_>>();
    for input in &generator.inputs {
        if !available.contains(input.as_str()) {
            return Err(GeneratorExecutionError::UnknownInput {
                generator: generator.id.clone(),
                input: input.clone(),
            });
        }
    }
    ensure_unique(
        generator
            .model_roots
            .iter()
            .map(|root| (root.package.as_str(), root.module.as_str())),
        "model root",
    )?;
    ensure_unique(
        generator
            .outputs
            .iter()
            .map(|output| output.logical_path.as_str()),
        "generator output",
    )?;
    MetaLimits::new(
        generator.limits.steps,
        generator.limits.memory_bytes,
        generator.limits.output_bytes,
    )?;
    Ok(())
}

fn validate_and_format_response(
    request: MetaRequest,
    response: MetaResponse,
) -> Result<MetaResponse, GeneratorExecutionError> {
    let mut builder = request.into_source_builder();
    for source in response.outputs() {
        let bytes = format_generated_module(source.bytes())?;
        if !source.mappings().is_empty() && bytes != source.bytes() {
            return Err(GeneratorExecutionError::InvalidGeneratedSource);
        }
        builder.add_mapped_source(
            source.path(),
            source.module(),
            bytes,
            source.mappings().iter().copied(),
        )?;
    }
    Ok(builder.finish()?)
}

fn format_generated_module(bytes: &[u8]) -> Result<Vec<u8>, GeneratorExecutionError> {
    let mut sources = SourceDatabase::new();
    let file = sources
        .add(SourceInput::virtual_file(
            SourceId::new("generated:manifest").expect("the generated source identity is valid"),
            ModulePath::new("generated").expect("the generated module is valid"),
            LogicalPath::new("generated/manifest.to").expect("the generated path is valid"),
            bytes.to_vec(),
        ))
        .map_err(|_| GeneratorExecutionError::InvalidGeneratedSource)?;
    let lexed = lex_with_limits(&sources, file, LexMode::Module, LexLimits::DEFAULT)
        .map_err(|_| GeneratorExecutionError::InvalidGeneratedSource)?;
    if !lexed.diagnostics().is_empty() {
        return Err(GeneratorExecutionError::InvalidGeneratedSource);
    }
    let parsed = parse(
        &sources,
        file,
        lexed,
        ParseMode::Module,
        ParseLimits::default(),
    )
    .map_err(|_| GeneratorExecutionError::InvalidGeneratedSource)?;
    if !parsed.diagnostics().is_empty() {
        return Err(GeneratorExecutionError::InvalidGeneratedSource);
    }
    format_parsed(&sources, file, &parsed)
        .map(|source| source.into_bytes())
        .map_err(|_| GeneratorExecutionError::InvalidGeneratedSource)
}

fn measure_generator_output(outcome: &VmOutcome) -> Result<u64, MetaVmError> {
    let VmOutcome::Returned(RuntimeValue::String(encoded)) = outcome else {
        return Ok(0);
    };
    let response = MetaResponse::decode(encoded.as_bytes())
        .map_err(|error| MetaVmError::StructuredOutput(error.to_string()))?;
    response.outputs().iter().try_fold(0_u64, |total, source| {
        total
            .checked_add(source.bytes().len() as u64)
            .ok_or(MetaVmError::OutputSizeOverflow)
    })
}

fn required(field: &str, value: &str) -> Result<(), GeneratorExecutionError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(GeneratorExecutionError::InvalidPlan(format!(
            "invalid {field}"
        )));
    }
    Ok(())
}

fn require_hash(field: &str, value: &str) -> Result<(), GeneratorExecutionError> {
    let valid = value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    if !valid {
        return Err(GeneratorExecutionError::InvalidPlan(format!(
            "invalid {field}"
        )));
    }
    Ok(())
}

fn ensure_unique<T: Ord>(
    values: impl IntoIterator<Item = T>,
    kind: &str,
) -> Result<(), GeneratorExecutionError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(GeneratorExecutionError::InvalidPlan(format!(
                "duplicate {kind}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum GeneratorExecutionError {
    InvalidPlan(String),
    InputSet,
    InputHash(String),
    UnknownInput {
        generator: String,
        input: String,
    },
    SnapshotSet,
    SnapshotRoots(String),
    OutputCollision(String),
    DuplicateProvider(String),
    MissingProvider(String),
    ProviderFailed {
        generator: String,
        message: String,
    },
    ProviderVm {
        generator: String,
        source: MetaVmError,
    },
    InvalidProviderResult(String),
    InvalidGeneratedSource,
    Model(MetaModelError),
    Contract(MetaContractError),
}

impl fmt::Display for GeneratorExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPlan(message) => write!(formatter, "invalid generator plan: {message}"),
            Self::InputSet => {
                formatter.write_str("generator input values do not match the lockfile")
            }
            Self::InputHash(input) => {
                write!(formatter, "generator input `{input}` has the wrong hash")
            }
            Self::UnknownInput { generator, input } => write!(
                formatter,
                "generator `{generator}` references unknown input `{input}`"
            ),
            Self::SnapshotSet => formatter.write_str("generator snapshots do not match the plan"),
            Self::SnapshotRoots(generator) => write!(
                formatter,
                "generator `{generator}` received the wrong root closure"
            ),
            Self::OutputCollision(path) => write!(
                formatter,
                "generator output `{path}` has more than one owner"
            ),
            Self::DuplicateProvider(provider) => {
                write!(formatter, "duplicate generator provider `{provider}`")
            }
            Self::MissingProvider(generator) => {
                write!(formatter, "generator `{generator}` has no locked provider")
            }
            Self::ProviderFailed { generator, message } => write!(
                formatter,
                "generator `{generator}` provider failed: {message}"
            ),
            Self::ProviderVm { generator, source } => {
                write!(formatter, "generator `{generator}` VM failed: {source}")
            }
            Self::InvalidProviderResult(generator) => write!(
                formatter,
                "generator `{generator}` did not return a structured response"
            ),
            Self::InvalidGeneratedSource => {
                formatter.write_str("generator returned invalid Tondo source")
            }
            Self::Model(error) => error.fmt(formatter),
            Self::Contract(error) => error.fmt(formatter),
        }
    }
}

impl Error for GeneratorExecutionError {}

impl From<MetaModelError> for GeneratorExecutionError {
    fn from(error: MetaModelError) -> Self {
        Self::Model(error)
    }
}

impl From<MetaContractError> for GeneratorExecutionError {
    fn from(error: MetaContractError) -> Self {
        Self::Contract(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{MetaSourceMapEntry, MetaSpan};
    use crate::meta_test_support::string_artifact;
    use crate::toolchain::{Limits, ModelRoot, Output};

    struct Good;

    impl GeneratorProviderCompiler for Good {
        fn compile(&self, request: GeneratorProviderRequest<'_>) -> Result<MetaVmArtifact, String> {
            assert_eq!(request.generator_id(), "generate-schema");
            assert_eq!(request.owner_package(), "workspace:app@1");
            assert_eq!(request.provider_package(), "workspace:meta@1");
            assert_eq!(request.entry(), "schema.generate");
            assert_eq!(request.provider_hash(), provider_hash());
            assert_eq!(request.meta_request().inputs()[0].name(), "schema");
            assert_eq!(request.meta_request().inputs()[0].bytes(), b"schema-v1");
            let mut builder = request.meta_request().clone().into_source_builder();
            builder
                .add_source(
                    "generated/schema.to",
                    "schema",
                    b"fn generated():String{\"ok\"}\n".to_vec(),
                )
                .unwrap();
            let response = builder.finish().unwrap().canonical_bytes().unwrap();
            Ok(string_artifact(std::str::from_utf8(&response).unwrap()))
        }
    }

    struct InvalidResponse;

    impl GeneratorProviderCompiler for InvalidResponse {
        fn compile(
            &self,
            _request: GeneratorProviderRequest<'_>,
        ) -> Result<MetaVmArtifact, String> {
            Ok(string_artifact("not a response"))
        }
    }

    struct Failure;

    impl GeneratorProviderCompiler for Failure {
        fn compile(
            &self,
            _request: GeneratorProviderRequest<'_>,
        ) -> Result<MetaVmArtifact, String> {
            Err("rejected".into())
        }
    }

    struct ArbitraryResponse {
        path: &'static str,
        source: Vec<u8>,
        mapped: bool,
    }

    impl GeneratorProviderCompiler for ArbitraryResponse {
        fn compile(
            &self,
            _request: GeneratorProviderRequest<'_>,
        ) -> Result<MetaVmArtifact, String> {
            let limits = MetaLimits::new(u64::MAX, u64::MAX, u64::MAX).unwrap();
            let snapshot = MetaSnapshot::new([], [], []).unwrap();
            let output = MetaOutputSpec::new(self.path, "schema").unwrap();
            let mut builder = MetaRequest::new(snapshot, [], [output], limits)
                .unwrap()
                .into_source_builder();
            let mappings = self
                .mapped
                .then(|| MetaSourceMapEntry::new(0, 1, MetaSpan::new(0, 0, 1).unwrap()).unwrap());
            builder
                .add_mapped_source(self.path, "schema", self.source.clone(), mappings)
                .unwrap();
            let bytes = builder.finish().unwrap().canonical_bytes().unwrap();
            Ok(string_artifact(std::str::from_utf8(&bytes).unwrap()))
        }
    }

    fn provider_hash() -> &'static str {
        "sha256:1111111111111111111111111111111111111111111111111111111111111111"
    }

    fn generator() -> LockedGenerator {
        LockedGenerator {
            id: "generate-schema".into(),
            owner_package: "workspace:app@1".into(),
            provider_package: "workspace:meta@1".into(),
            entry: "schema.generate".into(),
            meta_model: META_MODEL.into(),
            provider_hash: provider_hash().into(),
            inputs: vec!["schema".into()],
            model_roots: vec![ModelRoot {
                package: "workspace:app@1".into(),
                module: "schema".into(),
            }],
            outputs: vec![Output {
                logical_path: "generated/schema.to".into(),
                module: "schema".into(),
            }],
            limits: Limits {
                steps: 10_000,
                memory_bytes: 64 * 1024,
                output_bytes: 1024,
            },
        }
    }

    fn inputs() -> (Vec<LockedNamedInput>, BTreeMap<String, Vec<u8>>) {
        let bytes = b"schema-v1".to_vec();
        (
            vec![LockedNamedInput {
                name: "schema".into(),
                sha256: sha256(&bytes),
            }],
            BTreeMap::from([("schema".into(), bytes)]),
        )
    }

    fn snapshots() -> BTreeMap<String, MetaSnapshot> {
        BTreeMap::from([(
            "generate-schema".into(),
            MetaSnapshot::new(
                [MetaRoot::new("workspace:app@1", "schema").unwrap()],
                [],
                [],
            )
            .unwrap(),
        )])
    }

    #[test]
    fn generator_receives_only_locked_values_and_publishes_formatted_outputs() {
        assert!(
            execute_generator_plan(
                &[],
                &[],
                &BTreeMap::new(),
                &BTreeMap::new(),
                &GeneratorProviderRegistry::default(),
            )
            .unwrap()
            .results()
            .is_empty()
        );
        let generator = generator();
        let (locked, values) = inputs();
        let mut registry = GeneratorProviderRegistry::default();
        registry.insert_for(&generator, Arc::new(Good)).unwrap();
        let execution = execute_generator_plan(
            std::slice::from_ref(&generator),
            &locked,
            &values,
            &snapshots(),
            &registry,
        )
        .unwrap();
        let result = &execution.results()[0];
        assert_eq!(result.generator_id(), "generate-schema");
        assert_eq!(result.provider_hash(), provider_hash());
        assert_eq!(
            std::str::from_utf8(result.response().outputs()[0].bytes()).unwrap(),
            "fn generated(): String {\n    \"ok\"\n}\n"
        );
        assert_eq!(execution.into_results().len(), 1);
    }

    #[test]
    fn ambient_input_and_snapshot_drift_are_rejected_before_execution() {
        let generator = generator();
        let (locked, mut values) = inputs();
        values.insert("ambient".into(), b"hidden".to_vec());
        assert!(matches!(
            execute_generator_plan(
                std::slice::from_ref(&generator),
                &locked,
                &values,
                &snapshots(),
                &GeneratorProviderRegistry::default()
            ),
            Err(GeneratorExecutionError::InputSet)
        ));

        let (_, values) = inputs();
        let wrong =
            BTreeMap::from([(generator.id.clone(), MetaSnapshot::new([], [], []).unwrap())]);
        assert!(matches!(
            execute_generator_plan(
                &[generator],
                &locked,
                &values,
                &wrong,
                &GeneratorProviderRegistry::default()
            ),
            Err(GeneratorExecutionError::SnapshotRoots(_))
        ));
    }

    #[test]
    fn output_collisions_are_rejected_before_any_provider_runs() {
        let first = generator();
        let mut second = first.clone();
        second.id = "generate-other".into();
        let (locked, values) = inputs();
        let mut snapshots = snapshots();
        snapshots.insert(second.id.clone(), snapshots[&first.id].clone());
        let mut registry = GeneratorProviderRegistry::default();
        registry.insert_for(&first, Arc::new(Failure)).unwrap();
        assert!(matches!(
            execute_generator_plan(&[second, first], &locked, &values, &snapshots, &registry),
            Err(GeneratorExecutionError::OutputCollision(_))
        ));
    }

    #[test]
    fn provider_failures_and_invalid_responses_publish_nothing() {
        let generator = generator();
        let (locked, values) = inputs();
        let mut failed = GeneratorProviderRegistry::default();
        failed.insert_for(&generator, Arc::new(Failure)).unwrap();
        assert!(matches!(
            execute_generator_plan(
                std::slice::from_ref(&generator),
                &locked,
                &values,
                &snapshots(),
                &failed
            ),
            Err(GeneratorExecutionError::ProviderFailed { .. })
        ));

        let mut invalid = GeneratorProviderRegistry::default();
        invalid
            .insert_for(&generator, Arc::new(InvalidResponse))
            .unwrap();
        assert!(matches!(
            execute_generator_plan(
                std::slice::from_ref(&generator),
                &locked,
                &values,
                &snapshots(),
                &invalid
            ),
            Err(GeneratorExecutionError::ProviderVm { .. })
        ));
        assert!(invalid.insert_for(&generator, Arc::new(Good)).is_err());
    }

    #[test]
    fn locked_input_plan_and_provider_identity_fail_closed() {
        let generator = generator();
        let (locked, mut values) = inputs();
        values.insert("schema".into(), b"drift".to_vec());
        let error = execute_generator_plan(
            std::slice::from_ref(&generator),
            &locked,
            &values,
            &snapshots(),
            &GeneratorProviderRegistry::default(),
        )
        .unwrap_err();
        assert!(matches!(error, GeneratorExecutionError::InputHash(_)));
        assert!(!error.to_string().is_empty());

        let (_, values) = inputs();
        assert!(matches!(
            execute_generator_plan(
                std::slice::from_ref(&generator),
                &locked,
                &values,
                &BTreeMap::new(),
                &GeneratorProviderRegistry::default(),
            ),
            Err(GeneratorExecutionError::SnapshotSet)
        ));
        assert!(matches!(
            execute_generator_plan(
                &[generator.clone(), generator.clone()],
                &locked,
                &values,
                &snapshots(),
                &GeneratorProviderRegistry::default(),
            ),
            Err(GeneratorExecutionError::InvalidPlan(_))
        ));

        let mut unknown = generator.clone();
        unknown.inputs = vec!["missing".into()];
        assert!(matches!(
            execute_generator_plan(
                &[unknown],
                &locked,
                &values,
                &snapshots(),
                &GeneratorProviderRegistry::default(),
            ),
            Err(GeneratorExecutionError::UnknownInput { .. })
        ));
        assert!(matches!(
            execute_generator_plan(
                &[generator],
                &locked,
                &values,
                &snapshots(),
                &GeneratorProviderRegistry::default(),
            ),
            Err(GeneratorExecutionError::MissingProvider(_))
        ));
    }

    #[test]
    fn output_contract_rejects_paths_syntax_maps_and_budget_drift() {
        let generator = generator();
        let (locked, values) = inputs();
        let run = |provider: Arc<dyn GeneratorProviderCompiler>| {
            let mut registry = GeneratorProviderRegistry::default();
            registry.insert_for(&generator, provider).unwrap();
            execute_generator_plan(
                std::slice::from_ref(&generator),
                &locked,
                &values,
                &snapshots(),
                &registry,
            )
        };

        assert!(matches!(
            run(Arc::new(ArbitraryResponse {
                path: "generated/other.to",
                source: b"fn other() {}\n".to_vec(),
                mapped: false,
            })),
            Err(GeneratorExecutionError::Contract(
                MetaContractError::UnknownOutput(_)
            ))
        ));
        assert!(matches!(
            run(Arc::new(ArbitraryResponse {
                path: "generated/schema.to",
                source: b"fn broken(\n".to_vec(),
                mapped: false,
            })),
            Err(GeneratorExecutionError::InvalidGeneratedSource)
        ));
        assert!(matches!(
            run(Arc::new(ArbitraryResponse {
                path: "generated/schema.to",
                source: b"fn mapped():String{\"ok\"}\n".to_vec(),
                mapped: true,
            })),
            Err(GeneratorExecutionError::InvalidGeneratedSource)
        ));
        assert!(matches!(
            run(Arc::new(ArbitraryResponse {
                path: "generated/schema.to",
                source: vec![b' '; 1025],
                mapped: false,
            })),
            Err(GeneratorExecutionError::ProviderVm {
                source: MetaVmError::OutputLimit { .. },
                ..
            })
        ));
    }

    #[test]
    fn public_error_vocabulary_has_actionable_stable_text() {
        let errors = vec![
            GeneratorExecutionError::InvalidPlan("bad".into()),
            GeneratorExecutionError::InputSet,
            GeneratorExecutionError::InputHash("schema".into()),
            GeneratorExecutionError::UnknownInput {
                generator: "gen".into(),
                input: "missing".into(),
            },
            GeneratorExecutionError::SnapshotSet,
            GeneratorExecutionError::SnapshotRoots("gen".into()),
            GeneratorExecutionError::OutputCollision("generated/a.to".into()),
            GeneratorExecutionError::DuplicateProvider("provider".into()),
            GeneratorExecutionError::MissingProvider("gen".into()),
            GeneratorExecutionError::ProviderFailed {
                generator: "gen".into(),
                message: "failure".into(),
            },
            GeneratorExecutionError::ProviderVm {
                generator: "gen".into(),
                source: MetaVmError::StructuredOutput("failure".into()),
            },
            GeneratorExecutionError::InvalidProviderResult("gen".into()),
            GeneratorExecutionError::InvalidGeneratedSource,
            GeneratorExecutionError::Model(MetaModelError::NonCanonicalEncoding),
            GeneratorExecutionError::Contract(MetaContractError::InvalidLimit),
        ];
        for error in errors {
            assert!(!error.to_string().is_empty());
        }
    }
}
