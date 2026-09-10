//! Pure planning for public and secret test inputs.
//!
//! The input plan fixes names, descriptors, hashes and reproducibility claims.
//! It deliberately contains no input values and never consults the host.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::{sha256, validate_sha256};
use crate::driver::CapabilityName;
use crate::test_plan::TestProjectPlan;

pub const TEST_INPUT_PLAN_FORMAT: &str = "tondo-test-input-plan-draft";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestInputProfile {
    Build,
    Runtime,
    Both,
}

impl TestInputProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Runtime => "runtime",
            Self::Both => "both",
        }
    }

    fn parse(value: &str) -> Result<Self, TestInputPlanError> {
        match value {
            "build" => Ok(Self::Build),
            "runtime" => Ok(Self::Runtime),
            "both" => Ok(Self::Both),
            _ => Err(TestInputPlanError::InvalidField {
                field: "inputs.profile",
                message: format!("unsupported input profile `{value}`"),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestInputVisibility {
    Public,
    Secret,
}

impl TestInputVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Secret => "secret",
        }
    }

    fn parse(value: &str) -> Result<Self, TestInputPlanError> {
        match value {
            "public" => Ok(Self::Public),
            "secret" => Ok(Self::Secret),
            _ => Err(TestInputPlanError::InvalidField {
                field: "inputs.visibility",
                message: format!("unsupported input visibility `{value}`"),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestReproducibility {
    Closed,
    SecretDependentVersioned,
    SecretDependentUnversioned,
}

impl TestReproducibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::SecretDependentVersioned => "secret-dependent-versioned",
            Self::SecretDependentUnversioned => "secret-dependent-unversioned",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestInputDescriptor {
    name: String,
    source: String,
    profile: TestInputProfile,
    visibility: TestInputVisibility,
    sha256: Option<String>,
    provider: Option<String>,
    descriptor: Option<String>,
    version: Option<String>,
    capability: Option<String>,
}

impl TestInputDescriptor {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn profile(&self) -> TestInputProfile {
        self.profile
    }

    pub const fn visibility(&self) -> TestInputVisibility {
        self.visibility
    }

    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    pub fn descriptor(&self) -> Option<&str> {
        self.descriptor.as_deref()
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn capability(&self) -> Option<&str> {
        self.capability.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestInputPlan {
    test_plan_sha256: String,
    inputs: Vec<TestInputDescriptor>,
    public_sha256: String,
    secret_profile_sha256: Option<String>,
    secret_count: u32,
    reproducibility: TestReproducibility,
}

impl TestInputPlan {
    /// Construct a closed public input plan from already captured hashes.
    /// Each tuple is (name, logical source, profile, SHA-256). Input values
    /// never enter this record; the ordinary parser validates the result.
    pub fn from_public_hashes(
        test_plan: &TestProjectPlan,
        inputs: impl IntoIterator<Item = (String, String, TestInputProfile, String)>,
    ) -> Result<Self, TestInputPlanError> {
        let mut inputs = inputs
            .into_iter()
            .map(|(name, source, profile, hash)| TestInputDescriptor {
                name,
                source,
                profile,
                visibility: TestInputVisibility::Public,
                sha256: Some(hash),
                provider: None,
                descriptor: None,
                version: None,
                capability: None,
            })
            .collect::<Vec<_>>();
        inputs.sort_by(|left, right| left.name.cmp(&right.name));
        let plan = Self {
            test_plan_sha256: sha256(
                &test_plan
                    .canonical_bytes()
                    .map_err(|error| TestInputPlanError::Serialization(error.to_string()))?,
            ),
            public_sha256: public_digest(&inputs)?,
            inputs,
            secret_profile_sha256: None,
            secret_count: 0,
            reproducibility: TestReproducibility::Closed,
        };
        Self::parse(test_plan, &plan.canonical_bytes()?)
    }

    /// Parse a value-free input plan against the normalized test plan.
    ///
    /// Public values are represented only by their declared SHA-256. Secret
    /// values are never accepted: only provider, descriptor, and optional
    /// version metadata are retained. No host API is reachable from this
    /// method.
    pub fn parse(test_plan: &TestProjectPlan, bytes: &[u8]) -> Result<Self, TestInputPlanError> {
        let expected = sha256(
            &test_plan
                .canonical_bytes()
                .map_err(|error| TestInputPlanError::Serialization(error.to_string()))?,
        );
        let capabilities = test_plan
            .target()
            .capabilities()
            .iter()
            .map(ToString::to_string)
            .collect();
        let references = test_plan
            .sources()
            .iter()
            .map(|source| source.input())
            .collect();
        let parsed = Self::parse_bound(&expected, &capabilities, bytes, &references)?;
        for source in test_plan.sources() {
            let input = parsed
                .inputs
                .iter()
                .find(|input| input.name == source.input())
                .ok_or_else(|| TestInputPlanError::MissingInput(source.input().into()))?;
            if input.visibility != TestInputVisibility::Public
                || !matches!(
                    input.profile,
                    TestInputProfile::Build | TestInputProfile::Both
                )
            {
                return Err(TestInputPlanError::InvalidField {
                    field: "inputs.profile",
                    message: "source inputs must be public build inputs".into(),
                });
            }
        }
        Ok(parsed)
    }

    /// Revalidate an admitted input plan at the isolated worker boundary.
    /// The compiled transport supplies its expected identity and capabilities;
    /// source closure was checked before compilation.
    pub fn parse_worker(
        expected_test_plan: &str,
        capabilities: &BTreeSet<String>,
        bytes: &[u8],
    ) -> Result<Self, TestInputPlanError> {
        Self::parse_bound(expected_test_plan, capabilities, bytes, &BTreeSet::new())
    }

    fn parse_bound(
        expected_test_plan: &str,
        capabilities: &BTreeSet<String>,
        bytes: &[u8],
        referenced_names: &BTreeSet<&str>,
    ) -> Result<Self, TestInputPlanError> {
        let wire: TestInputPlanWire = serde_json::from_slice(bytes)
            .map_err(|error| TestInputPlanError::InvalidJson(error.to_string()))?;
        if wire.format != TEST_INPUT_PLAN_FORMAT {
            return Err(TestInputPlanError::InvalidField {
                field: "format",
                message: format!("expected `{TEST_INPUT_PLAN_FORMAT}`"),
            });
        }
        if wire.test_plan_sha256 != expected_test_plan {
            return Err(TestInputPlanError::PlanMismatch {
                expected: expected_test_plan.into(),
                actual: wire.test_plan_sha256,
            });
        }
        validate_sha256(&wire.test_plan_sha256).map_err(|error| {
            TestInputPlanError::InvalidField {
                field: "test_plan_sha256",
                message: error.to_string(),
            }
        })?;

        let allowed_capabilities = capabilities;
        let mut names = BTreeSet::new();
        let mut inputs = Vec::with_capacity(wire.inputs.len());
        for input in wire.inputs {
            let name = input_name("inputs.name", &input.name)?;
            if !names.insert(name.clone()) {
                return Err(TestInputPlanError::Duplicate {
                    kind: "input name",
                    value: name,
                });
            }
            let source = text("inputs.source", input.source)?;
            let profile = TestInputProfile::parse(&input.profile)?;
            let visibility = TestInputVisibility::parse(&input.visibility)?;
            let capability = input
                .capability
                .map(|value| text("inputs.capability", value))
                .transpose()?;
            if let Some(capability) = capability.as_deref() {
                CapabilityName::new(capability.to_owned()).map_err(|error| {
                    TestInputPlanError::InvalidField {
                        field: "inputs.capability",
                        message: error.to_string(),
                    }
                })?;
                if !allowed_capabilities
                    .iter()
                    .any(|allowed| *allowed == capability)
                {
                    return Err(TestInputPlanError::InvalidField {
                        field: "inputs.capability",
                        message: format!(
                            "capability `{capability}` is not enabled by the test target"
                        ),
                    });
                }
            }
            let (sha256_value, provider, descriptor, version) = match visibility {
                TestInputVisibility::Public => {
                    let hash = input
                        .sha256
                        .ok_or_else(|| TestInputPlanError::InvalidField {
                            field: "inputs.sha256",
                            message: "public inputs require a SHA-256".into(),
                        })?;
                    validate_sha256(&hash).map_err(|error| TestInputPlanError::InvalidField {
                        field: "inputs.sha256",
                        message: error.to_string(),
                    })?;
                    if input.provider.is_some()
                        || input.descriptor.is_some()
                        || input.version.is_some()
                    {
                        return Err(TestInputPlanError::InvalidField {
                            field: "inputs",
                            message: "public inputs cannot carry secret metadata".into(),
                        });
                    }
                    (Some(hash), None, None, None)
                }
                TestInputVisibility::Secret => {
                    if profile != TestInputProfile::Runtime {
                        return Err(TestInputPlanError::InvalidField {
                            field: "inputs.profile",
                            message: "secret inputs are runtime-only".into(),
                        });
                    }
                    if input.sha256.is_some() {
                        return Err(TestInputPlanError::InvalidField {
                            field: "inputs.sha256",
                            message: "secret values cannot be represented by a hash".into(),
                        });
                    }
                    let provider = text(
                        "inputs.provider",
                        input
                            .provider
                            .ok_or_else(|| TestInputPlanError::InvalidField {
                                field: "inputs.provider",
                                message: "secret inputs require a provider".into(),
                            })?,
                    )?;
                    let descriptor = text(
                        "inputs.descriptor",
                        input
                            .descriptor
                            .ok_or_else(|| TestInputPlanError::InvalidField {
                                field: "inputs.descriptor",
                                message: "secret inputs require a descriptor".into(),
                            })?,
                    )?;
                    let version = input
                        .version
                        .map(|value| text("inputs.version", value))
                        .transpose()?;
                    (None, Some(provider), Some(descriptor), version)
                }
            };
            inputs.push(TestInputDescriptor {
                name,
                source,
                profile,
                visibility,
                sha256: sha256_value,
                provider,
                descriptor,
                version,
                capability,
            });
        }
        if let Some(missing) = referenced_names.iter().find(|name| !names.contains(**name)) {
            return Err(TestInputPlanError::MissingInput((*missing).into()));
        }
        inputs.sort_by(|left, right| left.name.cmp(&right.name));

        let public_sha256 = public_digest(&inputs)?;
        if wire.public_sha256 != public_sha256 {
            return Err(TestInputPlanError::DigestMismatch {
                field: "public_sha256",
                expected: public_sha256,
                actual: wire.public_sha256,
            });
        }
        let secret_count = inputs
            .iter()
            .filter(|input| input.visibility == TestInputVisibility::Secret)
            .count() as u32;
        if wire.secret_count != secret_count {
            return Err(TestInputPlanError::CountMismatch {
                expected: secret_count,
                actual: wire.secret_count,
            });
        }
        let secret_profile_sha256 = secret_digest(&inputs)?;
        if wire.secret_profile_sha256 != secret_profile_sha256 {
            return Err(TestInputPlanError::DigestMismatch {
                field: "secret_profile_sha256",
                expected: secret_profile_sha256.clone().unwrap_or_default(),
                actual: wire.secret_profile_sha256.clone().unwrap_or_default(),
            });
        }
        let reproducibility = reproducibility(&inputs);
        if wire.reproducibility != reproducibility.as_str() {
            return Err(TestInputPlanError::InvalidField {
                field: "reproducibility",
                message: format!("expected `{}`", reproducibility.as_str()),
            });
        }
        Ok(Self {
            test_plan_sha256: expected_test_plan.into(),
            inputs,
            public_sha256,
            secret_profile_sha256,
            secret_count,
            reproducibility,
        })
    }

    /// Add value-free runtime declarations to captured public inputs. Names
    /// cannot replace a captured source, snapshot or project record. The
    /// ordinary parser validates every field, capability and digest.
    pub fn with_runtime_inputs(
        mut self,
        test_plan: &TestProjectPlan,
        bytes: &[u8],
    ) -> Result<Self, TestInputPlanError> {
        let declarations: Vec<TestInputWire> = serde_json::from_slice(bytes)
            .map_err(|error| TestInputPlanError::InvalidJson(error.to_string()))?;
        for input in declarations {
            let profile = TestInputProfile::parse(&input.profile)?;
            if profile != TestInputProfile::Runtime {
                return Err(TestInputPlanError::InvalidField {
                    field: "inputs.profile",
                    message: "declared host inputs must have the runtime profile".into(),
                });
            }
            self.inputs.push(TestInputDescriptor {
                name: input.name,
                source: input.source,
                profile,
                visibility: TestInputVisibility::parse(&input.visibility)?,
                sha256: input.sha256,
                provider: input.provider,
                descriptor: input.descriptor,
                version: input.version,
                capability: input.capability,
            });
        }
        self.inputs
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.public_sha256 = public_digest(&self.inputs)?;
        self.secret_profile_sha256 = secret_digest(&self.inputs)?;
        self.secret_count = self
            .inputs
            .iter()
            .filter(|input| input.visibility == TestInputVisibility::Secret)
            .count() as u32;
        self.reproducibility = reproducibility(&self.inputs);
        Self::parse(test_plan, &self.canonical_bytes()?)
    }

    pub fn test_plan_sha256(&self) -> &str {
        &self.test_plan_sha256
    }

    pub fn inputs(&self) -> &[TestInputDescriptor] {
        &self.inputs
    }

    pub fn public_sha256(&self) -> &str {
        &self.public_sha256
    }

    pub fn secret_profile_sha256(&self) -> Option<&str> {
        self.secret_profile_sha256.as_deref()
    }

    pub const fn secret_count(&self) -> u32 {
        self.secret_count
    }

    pub const fn reproducibility(&self) -> TestReproducibility {
        self.reproducibility
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TestInputPlanError> {
        serde_json::to_vec(&TestInputPlanWire::from_plan(self))
            .map_err(|error| TestInputPlanError::Serialization(error.to_string()))
    }
}

#[derive(Debug)]
pub enum TestInputPlanError {
    InvalidJson(String),
    InvalidField {
        field: &'static str,
        message: String,
    },
    PlanMismatch {
        expected: String,
        actual: String,
    },
    DigestMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    CountMismatch {
        expected: u32,
        actual: u32,
    },
    MissingInput(String),
    Duplicate {
        kind: &'static str,
        value: String,
    },
    Serialization(String),
}

impl fmt::Display for TestInputPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => {
                write!(formatter, "invalid test input plan JSON: {message}")
            }
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::PlanMismatch { expected, actual } => write!(
                formatter,
                "test input plan targets test plan `{actual}`, expected `{expected}`"
            ),
            Self::DigestMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "{field} `{actual}` does not match computed `{expected}`"
            ),
            Self::CountMismatch { expected, actual } => write!(
                formatter,
                "secret_count `{actual}` does not match computed `{expected}`"
            ),
            Self::MissingInput(name) => {
                write!(formatter, "test source input `{name}` has no descriptor")
            }
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind} `{value}`"),
            Self::Serialization(message) => {
                write!(formatter, "cannot encode test input plan: {message}")
            }
        }
    }
}

impl Error for TestInputPlanError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestInputPlanWire {
    format: String,
    test_plan_sha256: String,
    inputs: Vec<TestInputWire>,
    public_sha256: String,
    #[serde(default)]
    secret_profile_sha256: Option<String>,
    secret_count: u32,
    reproducibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestInputWire {
    name: String,
    source: String,
    profile: String,
    visibility: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    descriptor: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    capability: Option<String>,
}

impl TestInputPlanWire {
    fn from_plan(plan: &TestInputPlan) -> Self {
        Self {
            format: TEST_INPUT_PLAN_FORMAT.into(),
            test_plan_sha256: plan.test_plan_sha256.clone(),
            inputs: plan
                .inputs
                .iter()
                .map(|input| TestInputWire {
                    name: input.name.clone(),
                    source: input.source.clone(),
                    profile: input.profile.as_str().into(),
                    visibility: input.visibility.as_str().into(),
                    sha256: input.sha256.clone(),
                    provider: input.provider.clone(),
                    descriptor: input.descriptor.clone(),
                    version: input.version.clone(),
                    capability: input.capability.clone(),
                })
                .collect(),
            public_sha256: plan.public_sha256.clone(),
            secret_profile_sha256: plan.secret_profile_sha256.clone(),
            secret_count: plan.secret_count,
            reproducibility: plan.reproducibility.as_str().into(),
        }
    }
}

fn public_digest(inputs: &[TestInputDescriptor]) -> Result<String, TestInputPlanError> {
    #[derive(Serialize)]
    struct Fingerprint<'a> {
        name: &'a str,
        source: &'a str,
        profile: &'a str,
        sha256: &'a str,
        capability: Option<&'a str>,
    }
    let fingerprints = inputs
        .iter()
        .filter_map(|input| {
            input.sha256.as_deref().map(|sha256| Fingerprint {
                name: &input.name,
                source: &input.source,
                profile: input.profile.as_str(),
                sha256,
                capability: input.capability.as_deref(),
            })
        })
        .collect::<Vec<_>>();
    digest_hex(
        &serde_json::to_vec(&fingerprints)
            .map_err(|error| TestInputPlanError::Serialization(error.to_string()))?,
    )
}

fn secret_digest(inputs: &[TestInputDescriptor]) -> Result<Option<String>, TestInputPlanError> {
    #[derive(Serialize)]
    struct Fingerprint<'a> {
        name: &'a str,
        source: &'a str,
        profile: &'a str,
        provider: &'a str,
        descriptor: &'a str,
        version: Option<&'a str>,
        capability: Option<&'a str>,
    }
    let fingerprints = inputs
        .iter()
        .filter_map(|input| {
            Some(Fingerprint {
                name: &input.name,
                source: &input.source,
                profile: input.profile.as_str(),
                provider: input.provider.as_deref()?,
                descriptor: input.descriptor.as_deref()?,
                version: input.version.as_deref(),
                capability: input.capability.as_deref(),
            })
        })
        .collect::<Vec<_>>();
    if fingerprints.is_empty() {
        return Ok(None);
    }
    Ok(Some(digest_hex(
        &serde_json::to_vec(&fingerprints)
            .map_err(|error| TestInputPlanError::Serialization(error.to_string()))?,
    )?))
}

fn digest_hex(bytes: &[u8]) -> Result<String, TestInputPlanError> {
    sha256(bytes)
        .strip_prefix("sha256:")
        .map(str::to_owned)
        .ok_or_else(|| TestInputPlanError::Serialization("SHA-256 prefix missing".into()))
}

fn reproducibility(inputs: &[TestInputDescriptor]) -> TestReproducibility {
    let secrets = inputs
        .iter()
        .filter(|input| input.visibility == TestInputVisibility::Secret)
        .collect::<Vec<_>>();
    if secrets.is_empty() {
        TestReproducibility::Closed
    } else if secrets.iter().all(|input| input.version.is_some()) {
        TestReproducibility::SecretDependentVersioned
    } else {
        TestReproducibility::SecretDependentUnversioned
    }
}

fn input_name(field: &'static str, value: &str) -> Result<String, TestInputPlanError> {
    if value.is_empty() || value.contains('\n') || value.contains('\r') || value.contains('\\') {
        return Err(TestInputPlanError::InvalidField {
            field,
            message: "input name must be non-empty and contain no line breaks or backslashes"
                .into(),
        });
    }
    Ok(value.to_owned())
}

fn text(field: &'static str, value: String) -> Result<String, TestInputPlanError> {
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(TestInputPlanError::InvalidField {
            field,
            message: "value must be non-empty and contain no line breaks".into(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{CAPABILITY_REGISTRY, sha256};
    use crate::package::PackageId;
    use crate::project::{LockedSourceWire, ProjectPlan, package_content_hash};
    use crate::test_plan::TestProjectPlan;
    use serde_json::{Value, json};

    fn test_plan_fixture() -> TestProjectPlan {
        let package = "workspace:app@1";
        let source = b"fn main() {}\n";
        let manifest = serde_json::to_vec(&json!({
            "format": "tondo-manifest-draft",
            "target": {"name":"tondo-vm-hosted","profile":"hosted","capability_registry":CAPABILITY_REGISTRY,"capabilities":["console","process"],"features":["fast"]},
            "root": {"package":package,"source":"app/src/main.to","form":"module"},
            "standard": "toolchain:std:0.1-bootstrap",
            "packages": [{"id":package,"local_name":"app","edition":"0.1","dependencies":[],"source_sets":[{"id":"common","sources":[{"physical_path":"app/src/main.to","logical_path":"src/main.to","module":"main"}]}]}],
            "generator_inputs": [], "privileged_units": []
        }))
        .unwrap();
        let source_record = LockedSourceWire {
            source_set: "common".into(),
            physical_path: "app/src/main.to".into(),
            logical_path: "src/main.to".into(),
            module: "main".into(),
            sha256: sha256(source),
        };
        let content_hash = package_content_hash(
            &PackageId::new(package).unwrap(),
            &[],
            std::slice::from_ref(&source_record),
            None,
        )
        .unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format":"tondo-lock-draft","manifest_hash":sha256(&manifest),
            "standard":{"package_id":"toolchain:std:0.1-bootstrap","content_hash":crate::project::bootstrap_standard_hash()},
            "packages":[{"id":package,"content_hash":content_hash,"dependencies":[],"sources":[source_record],"interface":null}],
            "generator_inputs":[],"privileged_units":[]
        })).unwrap();
        let project = ProjectPlan::parse(&manifest, &lockfile).unwrap();
        let plan = json!({
            "format":"tondo-test-plan-draft","project":{"manifest_hash":sha256(&manifest),"lockfile_hash":sha256(&lockfile)},"repository_root":".",
            "roots":[{"class":"production","physical_path":"app/src","logical_path":"src"}],
            "sources":[{"class":"production","package":package,"physical_path":"app/src/main.to","logical_path":"src/main.to","module":"main","input":"source:production:app/src/main.to"}],
            "dev_dependencies":[],"codeowners":{"mode":"none"},"selector":{"kind":"none"},"shard":null,"order":{"kind":"canonical"},
            "policy":{"jobs":1,"allow_empty":false,"fail_fast":false,"retry":0,"repeat":1},"reporters":["json"],
            "artifact_store":{"path":"target/test-artifacts","content_addressed":true,"max_bytes":1024},"snapshot_stores":[],
            "target":{"name":"tondo-vm-hosted","profile":"hosted","capability_registry":CAPABILITY_REGISTRY,"capabilities":["console","process"],"features":["fast"]},
            "time_catalog":{"package":"std","module":"time","api":"monotonic-v1"},
            "limits":{"timeout_ms":1,"setup_timeout_ms":1,"teardown_timeout_ms":1,"output_bytes":1,"artifact_bytes":1,"snapshot_bytes":1,"memory_bytes":1,"instructions":1,"virtual_timers":1}
        });
        TestProjectPlan::parse(&project, &serde_json::to_vec(&plan).unwrap()).unwrap()
    }

    fn input_json(plan: &TestProjectPlan) -> Value {
        let test_plan_sha256 = sha256(&plan.canonical_bytes().unwrap());
        let public_sha256 = "b65d900505828a651399134c4d55bcef2b6a14add56884fedda93a9854ce265f";
        json!({"format":TEST_INPUT_PLAN_FORMAT,"test_plan_sha256":test_plan_sha256,"inputs":[{"name":"source:production:app/src/main.to","source":"app/src/main.to","profile":"build","visibility":"public","sha256":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}],"public_sha256":public_sha256,"secret_profile_sha256":null,"secret_count":0,"reproducibility":"closed"})
    }

    #[test]
    fn input_plan_closes_public_hashes_and_references_without_values() {
        let plan = test_plan_fixture();
        let input =
            TestInputPlan::parse(&plan, &serde_json::to_vec(&input_json(&plan)).unwrap()).unwrap();
        assert_eq!(input.inputs().len(), 1);
        assert_eq!(input.reproducibility(), TestReproducibility::Closed);
        assert_eq!(input.secret_count(), 0);
        assert!(
            !String::from_utf8(input.canonical_bytes().unwrap())
                .unwrap()
                .contains("value")
        );
    }

    #[test]
    fn input_plan_computes_versioned_and_unversioned_secret_states() {
        let plan = test_plan_fixture();
        let mut value = input_json(&plan);
        value["inputs"].as_array_mut().unwrap().push(json!({"name":"host:token","source":"environment:TOKEN","profile":"runtime","visibility":"secret","provider":"ci","descriptor":"TOKEN","version":"v1"}));
        let inputs = vec![
            TestInputDescriptor {
                name: "host:token".into(),
                source: "environment:TOKEN".into(),
                profile: TestInputProfile::Runtime,
                visibility: TestInputVisibility::Secret,
                sha256: None,
                provider: Some("ci".into()),
                descriptor: Some("TOKEN".into()),
                version: Some("v1".into()),
                capability: None,
            },
            TestInputDescriptor {
                name: "source:production:app/src/main.to".into(),
                source: "app/src/main.to".into(),
                profile: TestInputProfile::Build,
                visibility: TestInputVisibility::Public,
                sha256: Some(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .into(),
                ),
                provider: None,
                descriptor: None,
                version: None,
                capability: None,
            },
        ];
        let secret_profile_sha256 = secret_digest(&inputs).unwrap();
        value["secret_profile_sha256"] = json!(secret_profile_sha256);
        value["secret_count"] = json!(1);
        value["reproducibility"] = json!("secret-dependent-versioned");
        // The input list is already canonical by name in this fixture.
        let parsed = TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            parsed.reproducibility(),
            TestReproducibility::SecretDependentVersioned
        );
        value["inputs"][1]["version"] = Value::Null;
        value["inputs"][1]["descriptor"] = Value::Null;
        value["inputs"][1]["provider"] = Value::Null;
        value["inputs"][1]["visibility"] = json!("secret");
        value["inputs"][1]["source"] = json!("environment:OTHER");
        value["inputs"][1]["name"] = json!("host:other");
        value["inputs"][1]["descriptor"] = json!("OTHER");
        value["inputs"][1]["provider"] = json!("ci");
        value["secret_profile_sha256"] = Value::Null;
        value["reproducibility"] = json!("secret-dependent-unversioned");
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn runtime_declarations_preserve_public_identity_and_worker_admission() {
        let project = test_plan_fixture();
        let base = TestInputPlan::parse(
            &project,
            &serde_json::to_vec(&input_json(&project)).unwrap(),
        )
        .unwrap();
        let declaration = json!({"name":"host:token", "source":"environment:TOKEN", "profile":"runtime", "visibility":"secret", "provider":"fixture", "descriptor":"TOKEN", "version":"v1", "capability":"console"});
        let plan = base
            .clone()
            .with_runtime_inputs(
                &project,
                &serde_json::to_vec(&vec![declaration.clone()]).unwrap(),
            )
            .unwrap();
        assert_eq!(plan.public_sha256(), base.public_sha256());
        assert_eq!(plan.secret_count(), 1);
        let bytes = plan.canonical_bytes().unwrap();
        let capabilities = BTreeSet::from(["console".to_owned()]);
        assert_eq!(
            TestInputPlan::parse_worker(plan.test_plan_sha256(), &capabilities, &bytes).unwrap(),
            plan
        );
        assert!(matches!(
            TestInputPlan::parse_worker(&sha256(b"other plan"), &capabilities, &bytes),
            Err(TestInputPlanError::PlanMismatch { .. })
        ));
        assert!(
            TestInputPlan::parse_worker(plan.test_plan_sha256(), &BTreeSet::new(), &bytes).is_err()
        );
        for profile in ["build", "both"] {
            let mut record: Value = serde_json::from_slice(&bytes).unwrap();
            record["inputs"][0]["profile"] = profile.into();
            assert!(matches!(
                TestInputPlan::parse_worker(
                    plan.test_plan_sha256(),
                    &capabilities,
                    &serde_json::to_vec(&record).unwrap()
                ),
                Err(TestInputPlanError::InvalidField {
                    field: "inputs.profile",
                    ..
                })
            ));
            let mut input = declaration.clone();
            input["profile"] = profile.into();
            assert!(
                base.clone()
                    .with_runtime_inputs(&project, &serde_json::to_vec(&vec![input]).unwrap())
                    .is_err()
            );
        }
        let mut collision = declaration;
        collision["name"] = project.sources()[0].input().into();
        assert!(matches!(
            base.with_runtime_inputs(&project, &serde_json::to_vec(&vec![collision]).unwrap()),
            Err(TestInputPlanError::Duplicate { .. })
        ));
    }

    #[test]
    fn input_plan_rejects_missing_references_collisions_and_secret_hashes() {
        let plan = test_plan_fixture();
        let mut value = input_json(&plan);
        value["inputs"] = json!([]);
        assert!(matches!(
            TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()),
            Err(TestInputPlanError::MissingInput(_))
        ));
        let mut value = input_json(&plan);
        let item = value["inputs"][0].clone();
        value["inputs"].as_array_mut().unwrap().push(item);
        assert!(matches!(
            TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()),
            Err(TestInputPlanError::Duplicate {
                kind: "input name",
                ..
            })
        ));
        let mut value = input_json(&plan);
        value["inputs"][0]["visibility"] = json!("secret");
        value["inputs"][0]["provider"] = json!("ci");
        value["inputs"][0]["descriptor"] = json!("source");
        value["inputs"][0]["sha256"] = Value::Null;
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn input_plan_rejects_plan_hash_drift_capability_drift_and_unknown_fields() {
        let plan = test_plan_fixture();
        let mut value = input_json(&plan);
        value["test_plan_sha256"] = json!(sha256(b"different"));
        assert!(matches!(
            TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()),
            Err(TestInputPlanError::PlanMismatch { .. })
        ));
        let mut value = input_json(&plan);
        value["inputs"][0]["capability"] = json!("network");
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = input_json(&plan);
        value["unexpected"] = json!(true);
        assert!(matches!(
            TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()),
            Err(TestInputPlanError::InvalidJson(_))
        ));
    }

    #[test]
    fn input_plan_canonicalization_is_stable_and_has_no_secret_value_channel() {
        let plan = test_plan_fixture();
        let input =
            TestInputPlan::parse(&plan, &serde_json::to_vec(&input_json(&plan)).unwrap()).unwrap();
        let first = input.canonical_bytes().unwrap();
        let second = TestInputPlan::parse(&plan, &first)
            .unwrap()
            .canonical_bytes()
            .unwrap();
        assert_eq!(first, second);
        assert!(!first.windows(6).any(|window| window == b"value\""));
    }

    #[test]
    fn input_profiles_accessors_digests_and_error_messages_are_closed() {
        assert_eq!(TestInputProfile::Build.as_str(), "build");
        assert_eq!(TestInputProfile::Runtime.as_str(), "runtime");
        assert_eq!(TestInputProfile::Both.as_str(), "both");
        assert_eq!(
            TestInputProfile::parse("runtime").unwrap(),
            TestInputProfile::Runtime
        );
        assert!(TestInputProfile::parse("other").is_err());
        assert_eq!(TestInputVisibility::Public.as_str(), "public");
        assert_eq!(TestInputVisibility::Secret.as_str(), "secret");
        assert_eq!(
            TestInputVisibility::parse("secret").unwrap(),
            TestInputVisibility::Secret
        );
        assert!(TestInputVisibility::parse("other").is_err());
        assert_eq!(TestReproducibility::Closed.as_str(), "closed");
        assert_eq!(
            TestReproducibility::SecretDependentVersioned.as_str(),
            "secret-dependent-versioned"
        );
        assert_eq!(
            TestReproducibility::SecretDependentUnversioned.as_str(),
            "secret-dependent-unversioned"
        );

        let descriptor = TestInputDescriptor {
            name: "name".into(),
            source: "source".into(),
            profile: TestInputProfile::Both,
            visibility: TestInputVisibility::Secret,
            sha256: None,
            provider: Some("provider".into()),
            descriptor: Some("descriptor".into()),
            version: Some("v1".into()),
            capability: Some("console".into()),
        };
        assert_eq!(descriptor.name(), "name");
        assert_eq!(descriptor.source(), "source");
        assert_eq!(descriptor.profile(), TestInputProfile::Both);
        assert_eq!(descriptor.visibility(), TestInputVisibility::Secret);
        assert_eq!(descriptor.sha256(), None);
        assert_eq!(descriptor.provider(), Some("provider"));
        assert_eq!(descriptor.descriptor(), Some("descriptor"));
        assert_eq!(descriptor.version(), Some("v1"));
        assert_eq!(descriptor.capability(), Some("console"));
        assert_eq!(
            reproducibility(std::slice::from_ref(&descriptor)),
            TestReproducibility::SecretDependentVersioned
        );
        assert!(
            secret_digest(std::slice::from_ref(&descriptor))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            public_digest(std::slice::from_ref(&descriptor)).unwrap(),
            digest_hex(b"[]").unwrap()
        );
        assert!(digest_hex(b"").unwrap().len() == 64);
        assert!(input_name("name", "bad\\name").is_err());
        assert!(text("text", "bad\ntext".into()).is_err());

        for error in [
            TestInputPlanError::InvalidJson("bad".into()),
            TestInputPlanError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            TestInputPlanError::PlanMismatch {
                expected: "a".into(),
                actual: "b".into(),
            },
            TestInputPlanError::DigestMismatch {
                field: "x",
                expected: "a".into(),
                actual: "b".into(),
            },
            TestInputPlanError::CountMismatch {
                expected: 1,
                actual: 0,
            },
            TestInputPlanError::MissingInput("input".into()),
            TestInputPlanError::Duplicate {
                kind: "input",
                value: "input".into(),
            },
            TestInputPlanError::Serialization("bad".into()),
        ] {
            assert!(!error.to_string().is_empty());
        }

        let plan = test_plan_fixture();
        let mut value = input_json(&plan);
        value["inputs"][0]["sha256"] = Value::Null;
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = input_json(&plan);
        value["inputs"][0]["provider"] = json!("ci");
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = input_json(&plan);
        value["inputs"][0]["profile"] = json!("other");
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = input_json(&plan);
        value["inputs"][0]["name"] = json!("bad\\name");
        assert!(TestInputPlan::parse(&plan, &serde_json::to_vec(&value).unwrap()).is_err());
    }
}
