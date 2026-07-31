//! Worker-side materialization and revocation of test inputs.
//!
//! Planning remains value-free in [`crate::test_inputs`]. This module is the
//! next boundary: it resolves only descriptors admitted by that plan, checks
//! public bytes against their declared digest, keeps secret bytes in the
//! worker-owned set, and publishes metadata that cannot reconstruct secrets.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::artifact::sha256;
use crate::test_inputs::{
    TestInputDescriptor, TestInputPlan, TestInputProfile, TestInputVisibility, TestReproducibility,
};

pub const TEST_INPUT_RUNTIME_FORMAT: &str = "tondo-test-input-runtime-draft/1";

/// Which worker surface is being initialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaterializationTarget {
    Build,
    Runtime,
}

impl MaterializationTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Runtime => "runtime",
        }
    }

    fn accepts(self, profile: TestInputProfile) -> bool {
        matches!(
            (self, profile),
            (
                Self::Build,
                TestInputProfile::Build | TestInputProfile::Both
            ) | (
                Self::Runtime,
                TestInputProfile::Runtime | TestInputProfile::Both
            )
        )
    }
}

/// Host provider used only during worker initialization. Implementations must
/// not include secret bytes in an [`InputError`].
pub trait InputProvider {
    fn materialize(&self, descriptor: TestInputDescriptor) -> Result<Vec<u8>, InputError>;
}

impl<F> InputProvider for F
where
    F: Fn(TestInputDescriptor) -> Result<Vec<u8>, InputError>,
{
    fn materialize(&self, descriptor: TestInputDescriptor) -> Result<Vec<u8>, InputError> {
        self(descriptor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputError {
    ProviderUnavailable {
        name: String,
    },
    ProviderFailed {
        name: String,
    },
    PublicHashMismatch {
        name: String,
        expected: String,
        actual: String,
    },
    InputNotMaterialized(String),
    Revoked,
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderUnavailable { name } => {
                write!(formatter, "provider unavailable for input `{name}`")
            }
            Self::ProviderFailed { name } => {
                write!(formatter, "provider failed for input `{name}`")
            }
            Self::PublicHashMismatch {
                name,
                expected,
                actual,
            } => write!(
                formatter,
                "public input `{name}` has digest `{actual}`, expected `{expected}`"
            ),
            Self::InputNotMaterialized(name) => {
                write!(
                    formatter,
                    "input `{name}` was not materialized for this worker"
                )
            }
            Self::Revoked => formatter.write_str("test inputs have been revoked"),
        }
    }
}

impl Error for InputError {}

/// Metadata for one materialized input. Secret records intentionally have no
/// value digest in the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRecord {
    name: String,
    profile: TestInputProfile,
    visibility: TestInputVisibility,
    bytes: u64,
    sha256: Option<String>,
}

impl InputRecord {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub const fn profile(&self) -> TestInputProfile {
        self.profile
    }
    pub const fn visibility(&self) -> TestInputVisibility {
        self.visibility
    }
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }
}

/// Machine-readable materialization metadata. It contains secret identity
/// only through the already closed plan digest/count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputMaterializationReport {
    target: MaterializationTarget,
    public_count: u32,
    secret_count: u32,
    public_sha256: String,
    secret_profile_sha256: Option<String>,
    reproducibility: TestReproducibility,
    records: Vec<InputRecord>,
}

impl InputMaterializationReport {
    pub const fn target(&self) -> MaterializationTarget {
        self.target
    }
    pub const fn public_count(&self) -> u32 {
        self.public_count
    }
    pub const fn secret_count(&self) -> u32 {
        self.secret_count
    }
    pub fn public_sha256(&self) -> &str {
        &self.public_sha256
    }
    pub fn secret_profile_sha256(&self) -> Option<&str> {
        self.secret_profile_sha256.as_deref()
    }
    pub const fn reproducibility(&self) -> TestReproducibility {
        self.reproducibility
    }
    pub fn records(&self) -> &[InputRecord] {
        &self.records
    }

    /// Stable metadata bytes for diagnostics/cache manifests. Secret names and
    /// values never appear here; only the plan-level secret profile digest does.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = format!(
            "target={};public_count={};secret_count={};public_sha256={};secret_profile_sha256={};reproducibility={};",
            self.target.as_str(),
            self.public_count,
            self.secret_count,
            self.public_sha256,
            self.secret_profile_sha256.as_deref().unwrap_or("none"),
            self.reproducibility.as_str(),
        );
        for record in &self.records {
            bytes.push_str(&format!(
                "record={}:{:?}:{}:{};",
                record.name,
                record.visibility,
                record.bytes,
                record.sha256.as_deref().unwrap_or("secret")
            ));
        }
        bytes.into_bytes()
    }
}

/// A worker-owned value. Debug output is metadata-only so accidental logging
/// of this Rust object cannot print secret bytes.
pub struct MaterializedInput {
    descriptor: TestInputDescriptor,
    bytes: Vec<u8>,
    revoked: bool,
}

impl fmt::Debug for MaterializedInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MaterializedInput")
            .field("name", &self.descriptor.name())
            .field("visibility", &self.descriptor.visibility())
            .field("bytes", &self.bytes.len())
            .field("revoked", &self.revoked)
            .finish()
    }
}

impl MaterializedInput {
    pub fn name(&self) -> &str {
        self.descriptor.name()
    }
    pub const fn visibility(&self) -> TestInputVisibility {
        self.descriptor.visibility()
    }
    pub fn bytes(&self) -> Result<&[u8], InputError> {
        if self.revoked {
            return Err(InputError::Revoked);
        }
        Ok(&self.bytes)
    }

    fn revoke(&mut self) {
        if self.revoked {
            return;
        }
        for byte in &mut self.bytes {
            *byte = 0;
        }
        self.revoked = true;
    }
}

impl Drop for MaterializedInput {
    fn drop(&mut self) {
        for byte in &mut self.bytes {
            *byte = 0;
        }
    }
}

/// Materialized values and detached report metadata. The map remains private
/// to prevent replacement without rechecking its descriptor.
pub struct WorkerInputs {
    values: BTreeMap<String, MaterializedInput>,
    report: InputMaterializationReport,
    revoked: bool,
}

impl fmt::Debug for WorkerInputs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkerInputs")
            .field("names", &self.values.keys().collect::<Vec<_>>())
            .field("report", &self.report)
            .field("revoked", &self.revoked)
            .finish()
    }
}

impl WorkerInputs {
    pub fn report(&self) -> &InputMaterializationReport {
        &self.report
    }
    pub fn len(&self) -> usize {
        self.values.len()
    }
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    pub const fn revoked(&self) -> bool {
        self.revoked
    }

    pub fn get(&self, name: &str) -> Result<&[u8], InputError> {
        if self.revoked {
            return Err(InputError::Revoked);
        }
        self.values
            .get(name)
            .ok_or_else(|| InputError::InputNotMaterialized(name.into()))?
            .bytes()
    }

    pub fn contains(&self, name: &str) -> bool {
        !self.revoked && self.values.contains_key(name)
    }

    /// Zero every worker-owned buffer. Revocation is idempotent and happens
    /// before the worker result is detached.
    pub fn revoke(&mut self) -> Result<(), InputError> {
        if self.revoked {
            return Ok(());
        }
        for value in self.values.values_mut() {
            value.revoke();
        }
        self.revoked = true;
        Ok(())
    }
}

impl Drop for WorkerInputs {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            value.revoke();
        }
    }
}

/// Resolve active descriptors for one worker. The provider is called only for
/// profiles accepted by `target`; secret values never enter the report/cache
/// identity.
pub fn materialize<P: InputProvider>(
    plan: &TestInputPlan,
    target: MaterializationTarget,
    provider: &P,
) -> Result<WorkerInputs, InputError> {
    let mut values = BTreeMap::new();
    let mut records = Vec::new();
    let mut secret_count = 0_u32;
    for descriptor in plan.inputs() {
        if !target.accepts(descriptor.profile()) {
            continue;
        }
        let name = descriptor.name().to_owned();
        let bytes = match catch_unwind(AssertUnwindSafe(|| {
            provider.materialize(descriptor.clone())
        })) {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(InputError::ProviderUnavailable { .. })) => {
                return Err(InputError::ProviderUnavailable { name });
            }
            Ok(Err(_)) | Err(_) => return Err(InputError::ProviderFailed { name }),
        };
        let digest = sha256(&bytes);
        if descriptor.visibility() == TestInputVisibility::Public {
            let expected = descriptor.sha256().unwrap_or_default();
            if digest != expected {
                return Err(InputError::PublicHashMismatch {
                    name,
                    expected: expected.into(),
                    actual: digest,
                });
            }
        }
        let public = descriptor.visibility() == TestInputVisibility::Public;
        if public {
            records.push(InputRecord {
                name: descriptor.name().into(),
                profile: descriptor.profile(),
                visibility: descriptor.visibility(),
                bytes: bytes.len() as u64,
                sha256: Some(digest.strip_prefix("sha256:").unwrap_or_default().into()),
            });
        } else {
            secret_count = secret_count.saturating_add(1);
        }
        if values
            .insert(
                name.clone(),
                MaterializedInput {
                    descriptor: descriptor.clone(),
                    bytes,
                    revoked: false,
                },
            )
            .is_some()
        {
            return Err(InputError::ProviderFailed { name });
        }
    }
    let public_count = records.len() as u32;
    Ok(WorkerInputs {
        values,
        report: InputMaterializationReport {
            target,
            public_count,
            secret_count,
            public_sha256: plan.public_sha256().into(),
            secret_profile_sha256: plan.secret_profile_sha256().map(str::to_owned),
            reproducibility: plan.reproducibility(),
            records,
        },
        revoked: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{CAPABILITY_REGISTRY, sha256 as artifact_sha256};
    use crate::package::PackageId;
    use crate::project::{LockedSourceWire, ProjectPlan, package_content_hash};
    use crate::test_plan::TestProjectPlan;
    use serde_json::json;

    fn plan_fixture() -> TestProjectPlan {
        let package = "workspace:input-runtime@1";
        let source = b"fn main() {}\n";
        let manifest = serde_json::to_vec(&json!({
            "format": "tondo-manifest-draft",
            "target": {"name":"tondo-vm-hosted","profile":"hosted","capability_registry":CAPABILITY_REGISTRY,"capabilities":["console"],"features":[]},
            "root": {"package":package,"source":"app/src/main.to","form":"module"},
            "standard": "toolchain:std:0.1-bootstrap",
            "packages": [{"id":package,"local_name":"app","edition":"0.1","dependencies":[],"source_sets":[{"id":"common","sources":[{"physical_path":"app/src/main.to","logical_path":"src/main.to","module":"main"}]}]}],
            "generator_inputs": [], "privileged_units": []
        })).unwrap();
        let source_record = LockedSourceWire {
            source_set: "common".into(),
            physical_path: "app/src/main.to".into(),
            logical_path: "src/main.to".into(),
            module: "main".into(),
            sha256: artifact_sha256(source),
        };
        let content_hash = package_content_hash(
            &PackageId::new(package).unwrap(),
            &[],
            std::slice::from_ref(&source_record),
            None,
        )
        .unwrap();
        let lockfile = serde_json::to_vec(&json!({
            "format":"tondo-lock-draft","manifest_hash":artifact_sha256(&manifest),
            "standard":{"package_id":"toolchain:std:0.1-bootstrap","content_hash":crate::project::bootstrap_standard_hash()},
            "packages":[{"id":package,"content_hash":content_hash,"dependencies":[],"sources":[source_record],"interface":null}],
            "generator_inputs":[],"privileged_units":[]
        })).unwrap();
        let project = ProjectPlan::parse(&manifest, &lockfile).unwrap();
        let test_plan = json!({
            "format":"tondo-test-plan-draft","project":{"manifest_hash":artifact_sha256(&manifest),"lockfile_hash":artifact_sha256(&lockfile)},"repository_root":".",
            "roots":[{"class":"production","physical_path":"app/src","logical_path":"src"}],
            "sources":[{"class":"production","package":package,"physical_path":"app/src/main.to","logical_path":"src/main.to","module":"main","input":"source:production:app/src/main.to"}],
            "dev_dependencies":[],"codeowners":{"mode":"none"},"selector":{"kind":"none"},"shard":null,"order":{"kind":"canonical"},
            "policy":{"jobs":1,"allow_empty":false,"fail_fast":false,"retry":0,"repeat":1},"reporters":["json"],
            "artifact_store":{"path":"target/test-artifacts","content_addressed":true,"max_bytes":1024},"snapshot_stores":[],
            "target":{"name":"tondo-vm-hosted","profile":"hosted","capability_registry":CAPABILITY_REGISTRY,"capabilities":["console"],"features":[]},
            "time_catalog":{"package":"std","module":"time","api":"monotonic-v1"},
            "limits":{"timeout_ms":1,"setup_timeout_ms":1,"teardown_timeout_ms":1,"output_bytes":1,"artifact_bytes":1,"snapshot_bytes":1,"memory_bytes":1,"instructions":1,"virtual_timers":1}
        });
        TestProjectPlan::parse(&project, &serde_json::to_vec(&test_plan).unwrap()).unwrap()
    }

    fn input_plan(plan: &TestProjectPlan, include_secret: bool) -> TestInputPlan {
        let public_hash = artifact_sha256(b"public-config");
        let mut inputs = vec![json!({
            "name":"source:production:app/src/main.to","source":"app/src/main.to","profile":"build","visibility":"public","sha256":public_hash
        })];
        if include_secret {
            inputs.push(json!({"name":"host:token","source":"environment:TOKEN","profile":"runtime","visibility":"secret","provider":"ci","descriptor":"TOKEN","version":"v1"}));
        }
        #[derive(serde::Serialize)]
        struct PublicFingerprint<'a> {
            name: &'a str,
            source: &'a str,
            profile: &'a str,
            sha256: &'a str,
            capability: Option<&'a str>,
        }
        let public_fingerprints = vec![PublicFingerprint {
            name: "source:production:app/src/main.to",
            source: "app/src/main.to",
            profile: "build",
            sha256: &public_hash,
            capability: None,
        }];
        let public_sha = artifact_sha256(&serde_json::to_vec(&public_fingerprints).unwrap())
            .strip_prefix("sha256:")
            .unwrap()
            .to_owned();
        let secret_profile = if include_secret {
            #[derive(serde::Serialize)]
            struct SecretFingerprint<'a> {
                name: &'a str,
                source: &'a str,
                profile: &'a str,
                provider: &'a str,
                descriptor: &'a str,
                version: Option<&'a str>,
                capability: Option<&'a str>,
            }
            let secret_fp = vec![SecretFingerprint {
                name: "host:token",
                source: "environment:TOKEN",
                profile: "runtime",
                provider: "ci",
                descriptor: "TOKEN",
                version: Some("v1"),
                capability: None,
            }];
            Some(
                artifact_sha256(&serde_json::to_vec(&secret_fp).unwrap())
                    .strip_prefix("sha256:")
                    .unwrap()
                    .to_owned(),
            )
        } else {
            None
        };
        let value = json!({
            "format":"tondo-test-input-plan-draft","test_plan_sha256":artifact_sha256(&plan.canonical_bytes().unwrap()),"inputs":inputs,
            "public_sha256":public_sha,"secret_profile_sha256":secret_profile,"secret_count":if include_secret {1} else {0},"reproducibility":if include_secret {"secret-dependent-versioned"} else {"closed"}
        });
        TestInputPlan::parse(plan, &serde_json::to_vec(&value).unwrap()).unwrap()
    }

    #[test]
    fn runtime_materializes_only_runtime_inputs_and_checks_public_hashes() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, true);
        let calls = std::sync::Mutex::new(Vec::<String>::new());
        let worker = materialize(
            &inputs,
            MaterializationTarget::Runtime,
            &|descriptor: TestInputDescriptor| {
                calls.lock().unwrap().push(descriptor.name().to_owned());
                if descriptor.visibility() == TestInputVisibility::Secret {
                    Ok(b"secret-value".to_vec())
                } else {
                    Ok(b"public-config".to_vec())
                }
            },
        )
        .unwrap();
        assert_eq!(worker.len(), 1);
        assert!(worker.contains("host:token"));
        assert_eq!(worker.get("host:token").unwrap(), b"secret-value");
        assert_eq!(*calls.lock().unwrap(), ["host:token"]);
    }

    #[test]
    fn build_target_excludes_runtime_secret_and_report_has_no_secret_value() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, true);
        let worker = materialize(
            &inputs,
            MaterializationTarget::Build,
            &|descriptor: TestInputDescriptor| {
                assert_eq!(descriptor.visibility(), TestInputVisibility::Public);
                Ok(b"public-config".to_vec())
            },
        )
        .unwrap();
        assert_eq!(worker.len(), 1);
        assert_eq!(worker.report().secret_count(), 0);
        let bytes = String::from_utf8(worker.report().canonical_bytes()).unwrap();
        assert!(!bytes.contains("TOKEN"));
        assert!(!bytes.contains("secret-value"));
    }

    #[test]
    fn public_hash_mismatch_is_rejected_without_returning_a_worker() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, false);
        let error = materialize(&inputs, MaterializationTarget::Build, &|_| {
            Ok(b"wrong".to_vec())
        })
        .unwrap_err();
        assert!(matches!(error, InputError::PublicHashMismatch { .. }));
    }

    #[test]
    fn provider_errors_and_panics_are_sanitized_to_input_identity() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, true);
        let error = materialize(&inputs, MaterializationTarget::Runtime, &|_| {
            Err(InputError::ProviderUnavailable {
                name: "secret-value".into(),
            })
        })
        .unwrap_err();
        assert_eq!(
            error,
            InputError::ProviderUnavailable {
                name: "host:token".into()
            }
        );
        let panic_error = materialize(&inputs, MaterializationTarget::Runtime, &|_| -> Result<
            Vec<u8>,
            InputError,
        > {
            panic!("secret-value")
        })
        .unwrap_err();
        assert_eq!(
            panic_error,
            InputError::ProviderFailed {
                name: "host:token".into()
            }
        );
    }

    #[test]
    fn revoke_zeroizes_values_and_is_idempotent() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, true);
        let mut worker = materialize(
            &inputs,
            MaterializationTarget::Runtime,
            &|descriptor: TestInputDescriptor| {
                if descriptor.visibility() == TestInputVisibility::Secret {
                    Ok(b"secret-value".to_vec())
                } else {
                    Ok(b"public-config".to_vec())
                }
            },
        )
        .unwrap();
        worker.revoke().unwrap();
        assert!(worker.revoked());
        assert_eq!(worker.get("host:token"), Err(InputError::Revoked));
        worker.revoke().unwrap();
    }

    #[test]
    fn report_preserves_plan_digests_and_secret_reproducibility_without_names() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, true);
        let worker = materialize(
            &inputs,
            MaterializationTarget::Runtime,
            &|descriptor: TestInputDescriptor| {
                if descriptor.visibility() == TestInputVisibility::Secret {
                    Ok(vec![1, 2, 3])
                } else {
                    Ok(b"public-config".to_vec())
                }
            },
        )
        .unwrap();
        let report = worker.report();
        assert_eq!(report.public_sha256(), inputs.public_sha256());
        assert_eq!(
            report.secret_profile_sha256(),
            inputs.secret_profile_sha256()
        );
        assert_eq!(
            report.reproducibility(),
            TestReproducibility::SecretDependentVersioned
        );
        assert_eq!(report.secret_count(), 1);
        assert!(
            !String::from_utf8(report.canonical_bytes())
                .unwrap()
                .contains("host:token")
        );
    }

    #[test]
    fn missing_input_and_non_materialized_access_are_distinct() {
        let plan = plan_fixture();
        let inputs = input_plan(&plan, false);
        let worker = materialize(&inputs, MaterializationTarget::Build, &|_| {
            Ok(b"public-config".to_vec())
        })
        .unwrap();
        assert_eq!(
            worker.get("missing"),
            Err(InputError::InputNotMaterialized("missing".into()))
        );
        assert!(!worker.contains("host:token"));
    }
}
