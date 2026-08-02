//! Pure, closed planning for a Tondo test build.
//!
//! This module is deliberately limited to the plan boundary. It validates the
//! source classes and execution metadata that `tondo test` will later hand to
//! discovery and workers, but it never reads the repository, resolves a
//! CODEOWNERS file, materializes an input, or executes a test body.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::{CAPABILITY_REGISTRY, validate_sha256};
use crate::project::ProjectPlan;
use crate::source::ModulePath;

pub const TEST_PLAN_FORMAT: &str = "tondo-test-plan-draft";
const TIME_CATALOG_PACKAGE: &str = "std";
const TIME_CATALOG_MODULE: &str = "time";
const TIME_CATALOG_API: &str = "monotonic-v1";

/// The only source classes admitted by the 0.1 test plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TestSourceClass {
    Production,
    UnitTest,
    IntegrationTest,
}

impl TestSourceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::UnitTest => "unit-test",
            Self::IntegrationTest => "integration-test",
        }
    }

    fn parse(value: &str) -> Result<Self, TestPlanError> {
        match value {
            "production" => Ok(Self::Production),
            "unit-test" => Ok(Self::UnitTest),
            "integration-test" => Ok(Self::IntegrationTest),
            _ => Err(TestPlanError::InvalidField {
                field: "class",
                message: format!("unsupported source class `{value}`"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSourceRoot {
    class: TestSourceClass,
    physical_path: String,
    logical_path: String,
}

impl TestSourceRoot {
    pub fn class(&self) -> TestSourceClass {
        self.class
    }

    pub fn physical_path(&self) -> &str {
        &self.physical_path
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSource {
    class: TestSourceClass,
    package: String,
    physical_path: String,
    logical_path: String,
    module: String,
    input: String,
}

impl TestSource {
    pub fn class(&self) -> TestSourceClass {
        self.class
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn physical_path(&self) -> &str {
        &self.physical_path
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn input(&self) -> &str {
        &self.input
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDevDependency {
    alias: String,
    package: String,
    interface_path: String,
    sha256: String,
}

impl TestDevDependency {
    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn interface_path(&self) -> &str {
        &self.interface_path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeownersMode {
    Auto,
    None,
    Path(String),
}

impl CodeownersMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::Path(_) => "path",
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Path(path) => Some(path),
            Self::Auto | Self::None => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSelector {
    None,
    Filter(String),
    Glob(String),
    Exact(String),
}

impl TestSelector {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Filter(_) => "filter",
            Self::Glob(_) => "glob",
            Self::Exact(_) => "exact",
        }
    }

    pub fn value(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Filter(value) | Self::Glob(value) | Self::Exact(value) => Some(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestShard {
    index: u32,
    count: u32,
}

impl TestShard {
    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn count(self) -> u32 {
        self.count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestOrder {
    Canonical,
    Random { seed: Option<String> },
}

impl TestOrder {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Random { .. } => "random",
        }
    }

    pub fn seed(&self) -> Option<&str> {
        match self {
            Self::Canonical => None,
            Self::Random { seed } => seed.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestPolicy {
    jobs: u32,
    allow_empty: bool,
    fail_fast: bool,
    retry: u32,
    repeat: u32,
}

impl TestPolicy {
    pub const fn jobs(self) -> u32 {
        self.jobs
    }

    pub const fn allow_empty(self) -> bool {
        self.allow_empty
    }

    pub const fn fail_fast(self) -> bool {
        self.fail_fast
    }

    pub const fn retry(self) -> u32 {
        self.retry
    }

    pub const fn repeat(self) -> u32 {
        self.repeat
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestLimits {
    timeout_ms: u64,
    setup_timeout_ms: u64,
    teardown_timeout_ms: u64,
    output_bytes: u64,
    artifact_bytes: u64,
    snapshot_bytes: u64,
    memory_bytes: u64,
    instructions: u64,
    virtual_timers: u64,
}

impl TestLimits {
    pub const fn timeout_ms(self) -> u64 {
        self.timeout_ms
    }

    pub const fn setup_timeout_ms(self) -> u64 {
        self.setup_timeout_ms
    }

    pub const fn teardown_timeout_ms(self) -> u64 {
        self.teardown_timeout_ms
    }

    pub const fn output_bytes(self) -> u64 {
        self.output_bytes
    }

    pub const fn artifact_bytes(self) -> u64 {
        self.artifact_bytes
    }

    pub const fn snapshot_bytes(self) -> u64 {
        self.snapshot_bytes
    }

    pub const fn memory_bytes(self) -> u64 {
        self.memory_bytes
    }

    pub const fn instructions(self) -> u64 {
        self.instructions
    }

    pub const fn virtual_timers(self) -> u64 {
        self.virtual_timers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestArtifactStore {
    path: String,
    max_bytes: u64,
}

impl TestArtifactStore {
    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSnapshotStore {
    name: String,
    path: String,
    update: bool,
    max_bytes: u64,
}

impl TestSnapshotStore {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn update(&self) -> bool {
        self.update
    }

    pub const fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestTarget {
    name: String,
    profile: String,
    capabilities: Vec<String>,
    features: Vec<String>,
}

impl TestTarget {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn profile(&self) -> &str {
        &self.profile
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeCatalog {
    package: String,
    module: String,
    api: String,
}

impl TimeCatalog {
    pub fn package(&self) -> &str {
        &self.package
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn api(&self) -> &str {
        &self.api
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestProjectPlan {
    manifest_hash: String,
    lockfile_hash: String,
    repository_root: String,
    roots: Vec<TestSourceRoot>,
    sources: Vec<TestSource>,
    dev_dependencies: Vec<TestDevDependency>,
    codeowners: CodeownersMode,
    selector: TestSelector,
    shard: Option<TestShard>,
    order: TestOrder,
    policy: TestPolicy,
    reporters: Vec<String>,
    artifact_store: TestArtifactStore,
    snapshot_stores: Vec<TestSnapshotStore>,
    snapshot_stores_implicit: bool,
    target: TestTarget,
    time_catalog: TimeCatalog,
    limits: TestLimits,
}

impl TestProjectPlan {
    /// Materialize the opinionated test plan used when no sidecar is
    /// supplied. The production project is already closed, so its selected
    /// source graph is the only source input that needs to be repeated here.
    /// Callers may overlay invocation-local policy such as retry or selection
    /// without writing this plan to disk.
    pub fn defaults(project: &ProjectPlan, jobs: u32) -> Self {
        let mut roots = BTreeSet::new();
        let sources = project
            .selected_source_records()
            .map(|(package, physical_path, logical_path, module)| {
                roots.insert((source_parent(physical_path), source_parent(logical_path)));
                TestSource {
                    class: TestSourceClass::Production,
                    package: package.to_owned(),
                    physical_path: physical_path.to_owned(),
                    logical_path: logical_path.to_owned(),
                    module: module.to_owned(),
                    input: format!("source:production:{physical_path}"),
                }
            })
            .collect::<Vec<_>>();
        let roots = roots
            .into_iter()
            .map(|(physical_path, logical_path)| TestSourceRoot {
                class: TestSourceClass::Production,
                physical_path,
                logical_path,
            })
            .collect();
        let capabilities = project
            .capabilities()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let features = project
            .features()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        Self {
            manifest_hash: project.manifest_hash().to_owned(),
            lockfile_hash: project.lockfile_hash().to_owned(),
            repository_root: String::new(),
            roots,
            sources,
            dev_dependencies: Vec::new(),
            codeowners: CodeownersMode::Auto,
            selector: TestSelector::None,
            shard: None,
            order: TestOrder::Canonical,
            policy: TestPolicy {
                jobs: jobs.max(1),
                allow_empty: false,
                fail_fast: false,
                retry: 0,
                repeat: 1,
            },
            reporters: vec!["human".into(), "json".into()],
            artifact_store: TestArtifactStore {
                path: "target/test-artifacts".into(),
                max_bytes: 16 * 1024 * 1024,
            },
            snapshot_stores: vec![TestSnapshotStore {
                name: "default".into(),
                path: "tests/snapshots.json".into(),
                update: false,
                max_bytes: 16 * 1024 * 1024,
            }],
            snapshot_stores_implicit: true,
            target: TestTarget {
                name: project.target_name().into(),
                profile: project.profile().as_str().into(),
                capabilities,
                features,
            },
            time_catalog: TimeCatalog {
                package: TIME_CATALOG_PACKAGE.into(),
                module: TIME_CATALOG_MODULE.into(),
                api: TIME_CATALOG_API.into(),
            },
            limits: TestLimits {
                timeout_ms: 30_000,
                setup_timeout_ms: 30_000,
                teardown_timeout_ms: 30_000,
                output_bytes: 1024 * 1024,
                artifact_bytes: 16 * 1024 * 1024,
                snapshot_bytes: 16 * 1024 * 1024,
                memory_bytes: 64 * 1024 * 1024,
                instructions: 10_000_000,
                virtual_timers: 1_024,
            },
        }
    }

    /// Parse and validate a closed test plan against an already validated
    /// production project. This method does not read any path named by the
    /// plan; all path and input checks are purely structural.
    pub fn parse(project: &ProjectPlan, bytes: &[u8]) -> Result<Self, TestPlanError> {
        let wire: TestPlanWire = serde_json::from_slice(bytes)
            .map_err(|error| TestPlanError::InvalidJson(error.to_string()))?;
        if wire.format != TEST_PLAN_FORMAT {
            return Err(TestPlanError::InvalidField {
                field: "format",
                message: format!("expected `{TEST_PLAN_FORMAT}`, got `{}`", wire.format),
            });
        }
        if wire.project.manifest_hash != project.manifest_hash() {
            return Err(TestPlanError::ProjectMismatch {
                field: "manifest_hash",
                expected: project.manifest_hash().to_owned(),
                actual: wire.project.manifest_hash,
            });
        }
        if wire.project.lockfile_hash != project.lockfile_hash() {
            return Err(TestPlanError::ProjectMismatch {
                field: "lockfile_hash",
                expected: project.lockfile_hash().to_owned(),
                actual: wire.project.lockfile_hash,
            });
        }
        validate_sha256(&wire.project.manifest_hash).map_err(|error| {
            TestPlanError::InvalidField {
                field: "project.manifest_hash",
                message: error.to_string(),
            }
        })?;
        validate_sha256(&wire.project.lockfile_hash).map_err(|error| {
            TestPlanError::InvalidField {
                field: "project.lockfile_hash",
                message: error.to_string(),
            }
        })?;

        let repository_root = canonical_repository_root(&wire.repository_root)?;
        let roots = normalize_roots(wire.roots)?;
        let package_ids = project
            .package_ids()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let sources = normalize_sources(wire.sources, project, &package_ids, &roots)?;
        let dev_dependencies = normalize_dev_dependencies(wire.dev_dependencies)?;
        let codeowners = normalize_codeowners(wire.codeowners)?;
        let selector = normalize_selector(wire.selector)?;
        let shard = normalize_shard(wire.shard)?;
        let order = normalize_order(wire.order)?;
        let policy = normalize_policy(wire.policy)?;
        let reporters = normalize_reporters(wire.reporters)?;
        let artifact_store = normalize_artifact_store(wire.artifact_store)?;
        let snapshot_stores = normalize_snapshot_stores(wire.snapshot_stores)?;
        let target = normalize_target(wire.target, project)?;
        let time_catalog = normalize_time_catalog(wire.time_catalog)?;
        let limits = normalize_limits(wire.limits)?;

        Ok(Self {
            manifest_hash: project.manifest_hash().to_owned(),
            lockfile_hash: project.lockfile_hash().to_owned(),
            repository_root,
            roots,
            sources,
            dev_dependencies,
            codeowners,
            selector,
            shard,
            order,
            policy,
            reporters,
            artifact_store,
            snapshot_stores,
            snapshot_stores_implicit: false,
            target,
            time_catalog,
            limits,
        })
    }

    pub fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    pub fn lockfile_hash(&self) -> &str {
        &self.lockfile_hash
    }

    pub fn repository_root(&self) -> &str {
        &self.repository_root
    }

    pub fn roots(&self) -> &[TestSourceRoot] {
        &self.roots
    }

    pub fn sources(&self) -> &[TestSource] {
        &self.sources
    }

    /// Names referenced by source records. Host and generated inputs may add
    /// descriptors in the later input-plan phase, but every source reference
    /// must be covered before a worker can be created.
    pub fn input_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.sources.iter().map(|source| source.input.as_str())
    }

    pub fn dev_dependencies(&self) -> &[TestDevDependency] {
        &self.dev_dependencies
    }

    pub fn codeowners(&self) -> &CodeownersMode {
        &self.codeowners
    }

    pub fn selector(&self) -> &TestSelector {
        &self.selector
    }

    pub fn shard(&self) -> Option<TestShard> {
        self.shard
    }

    pub fn order(&self) -> &TestOrder {
        &self.order
    }

    pub const fn policy(&self) -> TestPolicy {
        self.policy
    }

    pub fn reporters(&self) -> &[String] {
        &self.reporters
    }

    pub fn artifact_store(&self) -> &TestArtifactStore {
        &self.artifact_store
    }

    pub fn snapshot_stores(&self) -> &[TestSnapshotStore] {
        &self.snapshot_stores
    }

    /// Whether the conventional snapshot store came from the opinionated
    /// plan rather than from a user-maintained sidecar.
    pub const fn snapshot_stores_implicit(&self) -> bool {
        self.snapshot_stores_implicit
    }

    pub fn target(&self) -> &TestTarget {
        &self.target
    }

    pub fn time_catalog(&self) -> &TimeCatalog {
        &self.time_catalog
    }

    pub const fn limits(&self) -> TestLimits {
        self.limits
    }

    /// Return deterministic compact JSON for the normalized plan.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TestPlanError> {
        let wire = TestPlanWire::from_plan(self);
        serde_json::to_vec(&wire).map_err(|error| TestPlanError::Serialization(error.to_string()))
    }
}

#[derive(Debug)]
pub enum TestPlanError {
    InvalidJson(String),
    InvalidField {
        field: &'static str,
        message: String,
    },
    ProjectMismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    Duplicate {
        kind: &'static str,
        value: String,
    },
    Serialization(String),
}

impl fmt::Display for TestPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(formatter, "invalid test plan JSON: {message}"),
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::ProjectMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "test plan {field} `{actual}` does not match project `{expected}`"
            ),
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind} `{value}`"),
            Self::Serialization(message) => write!(formatter, "cannot encode test plan: {message}"),
        }
    }
}

impl Error for TestPlanError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestPlanWire {
    format: String,
    project: ProjectIdentityWire,
    repository_root: String,
    roots: Vec<SourceRootWire>,
    sources: Vec<TestSourceWire>,
    dev_dependencies: Vec<DevDependencyWire>,
    codeowners: CodeownersWire,
    selector: SelectorWire,
    shard: Option<ShardWire>,
    order: OrderWire,
    policy: PolicyWire,
    reporters: Vec<String>,
    artifact_store: StoreWire,
    snapshot_stores: Vec<SnapshotStoreWire>,
    target: TestTargetWire,
    time_catalog: TimeCatalogWire,
    limits: LimitsWire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectIdentityWire {
    manifest_hash: String,
    lockfile_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRootWire {
    class: String,
    physical_path: String,
    logical_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestSourceWire {
    class: String,
    package: String,
    physical_path: String,
    logical_path: String,
    module: String,
    input: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DevDependencyWire {
    alias: String,
    package: String,
    interface_path: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodeownersWire {
    mode: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectorWire {
    kind: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShardWire {
    index: u32,
    count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderWire {
    kind: String,
    #[serde(default)]
    seed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyWire {
    jobs: u32,
    allow_empty: bool,
    fail_fast: bool,
    retry: u32,
    repeat: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreWire {
    path: String,
    content_addressed: bool,
    max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotStoreWire {
    name: String,
    path: String,
    update: bool,
    max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestTargetWire {
    name: String,
    profile: String,
    capability_registry: String,
    capabilities: Vec<String>,
    features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeCatalogWire {
    package: String,
    module: String,
    api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsWire {
    timeout_ms: u64,
    setup_timeout_ms: u64,
    teardown_timeout_ms: u64,
    output_bytes: u64,
    artifact_bytes: u64,
    snapshot_bytes: u64,
    memory_bytes: u64,
    instructions: u64,
    virtual_timers: u64,
}

impl TestPlanWire {
    fn from_plan(plan: &TestProjectPlan) -> Self {
        Self {
            format: TEST_PLAN_FORMAT.into(),
            project: ProjectIdentityWire {
                manifest_hash: plan.manifest_hash.clone(),
                lockfile_hash: plan.lockfile_hash.clone(),
            },
            repository_root: plan.repository_root.clone(),
            roots: plan
                .roots
                .iter()
                .map(|root| SourceRootWire {
                    class: root.class.as_str().into(),
                    physical_path: root.physical_path.clone(),
                    logical_path: root.logical_path.clone(),
                })
                .collect(),
            sources: plan
                .sources
                .iter()
                .map(|source| TestSourceWire {
                    class: source.class.as_str().into(),
                    package: source.package.clone(),
                    physical_path: source.physical_path.clone(),
                    logical_path: source.logical_path.clone(),
                    module: source.module.clone(),
                    input: source.input.clone(),
                })
                .collect(),
            dev_dependencies: plan
                .dev_dependencies
                .iter()
                .map(|dependency| DevDependencyWire {
                    alias: dependency.alias.clone(),
                    package: dependency.package.clone(),
                    interface_path: dependency.interface_path.clone(),
                    sha256: dependency.sha256.clone(),
                })
                .collect(),
            codeowners: CodeownersWire {
                mode: plan.codeowners.as_str().into(),
                path: plan.codeowners.path().map(str::to_owned),
            },
            selector: SelectorWire {
                kind: plan.selector.kind().into(),
                value: plan.selector.value().map(str::to_owned),
            },
            shard: plan.shard.map(|shard| ShardWire {
                index: shard.index,
                count: shard.count,
            }),
            order: OrderWire {
                kind: plan.order.kind().into(),
                seed: plan.order.seed().map(str::to_owned),
            },
            policy: PolicyWire {
                jobs: plan.policy.jobs,
                allow_empty: plan.policy.allow_empty,
                fail_fast: plan.policy.fail_fast,
                retry: plan.policy.retry,
                repeat: plan.policy.repeat,
            },
            reporters: plan.reporters.clone(),
            artifact_store: StoreWire {
                path: plan.artifact_store.path.clone(),
                content_addressed: true,
                max_bytes: plan.artifact_store.max_bytes,
            },
            snapshot_stores: plan
                .snapshot_stores
                .iter()
                .map(|store| SnapshotStoreWire {
                    name: store.name.clone(),
                    path: store.path.clone(),
                    update: store.update,
                    max_bytes: store.max_bytes,
                })
                .collect(),
            target: TestTargetWire {
                name: plan.target.name.clone(),
                profile: plan.target.profile.clone(),
                capability_registry: CAPABILITY_REGISTRY.into(),
                capabilities: plan.target.capabilities.clone(),
                features: plan.target.features.clone(),
            },
            time_catalog: TimeCatalogWire {
                package: plan.time_catalog.package.clone(),
                module: plan.time_catalog.module.clone(),
                api: plan.time_catalog.api.clone(),
            },
            limits: LimitsWire {
                timeout_ms: plan.limits.timeout_ms,
                setup_timeout_ms: plan.limits.setup_timeout_ms,
                teardown_timeout_ms: plan.limits.teardown_timeout_ms,
                output_bytes: plan.limits.output_bytes,
                artifact_bytes: plan.limits.artifact_bytes,
                snapshot_bytes: plan.limits.snapshot_bytes,
                memory_bytes: plan.limits.memory_bytes,
                instructions: plan.limits.instructions,
                virtual_timers: plan.limits.virtual_timers,
            },
        }
    }
}

fn normalize_roots(wire: Vec<SourceRootWire>) -> Result<Vec<TestSourceRoot>, TestPlanError> {
    if wire.is_empty() {
        return Err(TestPlanError::InvalidField {
            field: "roots",
            message: "at least one explicit source root is required".into(),
        });
    }
    let mut roots = wire
        .into_iter()
        .map(|root| {
            let class = TestSourceClass::parse(&root.class)?;
            Ok(TestSourceRoot {
                class,
                physical_path: canonical_root_path("roots.physical_path", &root.physical_path)?,
                logical_path: canonical_root_path("roots.logical_path", &root.logical_path)?,
            })
        })
        .collect::<Result<Vec<_>, TestPlanError>>()?;
    roots.sort_by(|left, right| {
        (
            left.class,
            left.logical_path.as_str(),
            left.physical_path.as_str(),
        )
            .cmp(&(
                right.class,
                right.logical_path.as_str(),
                right.physical_path.as_str(),
            ))
    });
    let mut seen = BTreeSet::new();
    for root in &roots {
        let key = (
            root.class,
            root.physical_path.clone(),
            root.logical_path.clone(),
        );
        if !seen.insert(key) {
            return Err(TestPlanError::Duplicate {
                kind: "source root",
                value: format!("{}:{}", root.class.as_str(), root.logical_path),
            });
        }
    }
    Ok(roots)
}

fn normalize_sources(
    wire: Vec<TestSourceWire>,
    project: &ProjectPlan,
    package_ids: &BTreeSet<String>,
    roots: &[TestSourceRoot],
) -> Result<Vec<TestSource>, TestPlanError> {
    if wire.is_empty() {
        return Err(TestPlanError::InvalidField {
            field: "sources",
            message: "at least one source is required".into(),
        });
    }
    let mut sources = Vec::with_capacity(wire.len());
    let mut physical_paths = BTreeSet::new();
    let mut logical_nodes = BTreeSet::new();
    let mut inputs = BTreeSet::new();
    for source in wire {
        let class = TestSourceClass::parse(&source.class)?;
        let package = canonical_identity("sources.package", &source.package)?;
        if !package_ids.contains(&package) && class != TestSourceClass::IntegrationTest {
            return Err(TestPlanError::InvalidField {
                field: "sources.package",
                message: format!("package `{package}` is not in the closed project graph"),
            });
        }
        let physical_path = canonical_path("sources.physical_path", &source.physical_path, true)?;
        let logical_path = canonical_path("sources.logical_path", &source.logical_path, true)?;
        let module =
            ModulePath::new(&source.module).map_err(|error| TestPlanError::InvalidField {
                field: "sources.module",
                message: error.to_string(),
            })?;
        let input = canonical_identity("sources.input", &source.input)?;
        if !physical_paths.insert(physical_path.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "source physical path",
                value: physical_path,
            });
        }
        if !inputs.insert(input.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "source input",
                value: input,
            });
        }
        let node_key = (class, logical_path.clone(), module.to_string());
        if !logical_nodes.insert(node_key) {
            return Err(TestPlanError::Duplicate {
                kind: "source module node",
                value: format!("{}::{logical_path}::{}", class.as_str(), module),
            });
        }
        if !roots.iter().any(|root| {
            root.class == class
                && path_within(&physical_path, &root.physical_path)
                && path_within(&logical_path, &root.logical_path)
        }) {
            return Err(TestPlanError::InvalidField {
                field: "sources",
                message: format!(
                    "source `{physical_path}` is not covered by an explicit {} root",
                    class.as_str()
                ),
            });
        }
        sources.push(TestSource {
            class,
            package,
            physical_path,
            logical_path,
            module: module.to_string(),
            input,
        });
    }
    sources.sort_by(|left, right| {
        (
            left.class,
            left.logical_path.as_str(),
            left.module.as_str(),
            left.physical_path.as_str(),
            left.package.as_str(),
        )
            .cmp(&(
                right.class,
                right.logical_path.as_str(),
                right.module.as_str(),
                right.physical_path.as_str(),
                right.package.as_str(),
            ))
    });

    let production = sources
        .iter()
        .filter(|source| source.class == TestSourceClass::Production)
        .map(|source| source.physical_path.as_str())
        .collect::<BTreeSet<_>>();
    let project_sources = project.selected_source_paths().collect::<BTreeSet<_>>();
    if production != project_sources {
        return Err(TestPlanError::InvalidField {
            field: "sources",
            message: "production sources must exactly match the active project sources".into(),
        });
    }
    Ok(sources)
}

fn normalize_dev_dependencies(
    wire: Vec<DevDependencyWire>,
) -> Result<Vec<TestDevDependency>, TestPlanError> {
    let mut dependencies = Vec::with_capacity(wire.len());
    let mut aliases = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for dependency in wire {
        let alias = canonical_identifier("dev_dependencies.alias", &dependency.alias)?;
        let package = canonical_identity("dev_dependencies.package", &dependency.package)?;
        let interface_path = canonical_path(
            "dev_dependencies.interface_path",
            &dependency.interface_path,
            true,
        )?;
        validate_sha256(&dependency.sha256).map_err(|error| TestPlanError::InvalidField {
            field: "dev_dependencies.sha256",
            message: error.to_string(),
        })?;
        if !aliases.insert(alias.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "dev-dependency alias",
                value: alias,
            });
        }
        if !packages.insert(package.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "dev-dependency package",
                value: package,
            });
        }
        dependencies.push(TestDevDependency {
            alias,
            package,
            interface_path,
            sha256: dependency.sha256,
        });
    }
    dependencies.sort_by(|left, right| left.alias.cmp(&right.alias));
    Ok(dependencies)
}

fn normalize_codeowners(wire: CodeownersWire) -> Result<CodeownersMode, TestPlanError> {
    match wire.mode.as_str() {
        "auto" if wire.path.is_none() => Ok(CodeownersMode::Auto),
        "none" if wire.path.is_none() => Ok(CodeownersMode::None),
        "path" => Ok(CodeownersMode::Path(canonical_path(
            "codeowners.path",
            wire.path
                .as_deref()
                .ok_or_else(|| TestPlanError::InvalidField {
                    field: "codeowners.path",
                    message: "path mode requires a path".into(),
                })?,
            true,
        )?)),
        "auto" | "none" => Err(TestPlanError::InvalidField {
            field: "codeowners",
            message: "auto and none do not accept a path".into(),
        }),
        _ => Err(TestPlanError::InvalidField {
            field: "codeowners.mode",
            message: format!("unsupported mode `{}`", wire.mode),
        }),
    }
}

fn normalize_selector(wire: SelectorWire) -> Result<TestSelector, TestPlanError> {
    match wire.kind.as_str() {
        "none" if wire.value.is_none() => Ok(TestSelector::None),
        "filter" => Ok(TestSelector::Filter(non_empty_text(
            "selector.value",
            wire.value,
        )?)),
        "glob" => Ok(TestSelector::Glob(non_empty_text(
            "selector.value",
            wire.value,
        )?)),
        "exact" => Ok(TestSelector::Exact(non_empty_text(
            "selector.value",
            wire.value,
        )?)),
        "none" => Err(TestPlanError::InvalidField {
            field: "selector",
            message: "none does not accept a value".into(),
        }),
        _ => Err(TestPlanError::InvalidField {
            field: "selector.kind",
            message: format!("unsupported selector `{}`", wire.kind),
        }),
    }
}

fn normalize_shard(wire: Option<ShardWire>) -> Result<Option<TestShard>, TestPlanError> {
    let Some(shard) = wire else { return Ok(None) };
    if shard.count == 0 || shard.index == 0 || shard.index > shard.count {
        return Err(TestPlanError::InvalidField {
            field: "shard",
            message: "index and count must satisfy 1 <= index <= count".into(),
        });
    }
    Ok(Some(TestShard {
        index: shard.index,
        count: shard.count,
    }))
}

fn normalize_order(wire: OrderWire) -> Result<TestOrder, TestPlanError> {
    match wire.kind.as_str() {
        "canonical" if wire.seed.is_none() => Ok(TestOrder::Canonical),
        "canonical" => Err(TestPlanError::InvalidField {
            field: "order.seed",
            message: "canonical order does not accept a seed".into(),
        }),
        "random" => Ok(TestOrder::Random {
            seed: wire.seed.map(|seed| normalize_seed(&seed)).transpose()?,
        }),
        _ => Err(TestPlanError::InvalidField {
            field: "order.kind",
            message: format!("unsupported order `{}`", wire.kind),
        }),
    }
}

fn normalize_policy(wire: PolicyWire) -> Result<TestPolicy, TestPlanError> {
    if wire.jobs == 0 || wire.repeat == 0 {
        return Err(TestPlanError::InvalidField {
            field: "policy",
            message: "jobs and repeat must be positive".into(),
        });
    }
    Ok(TestPolicy {
        jobs: wire.jobs,
        allow_empty: wire.allow_empty,
        fail_fast: wire.fail_fast,
        retry: wire.retry,
        repeat: wire.repeat,
    })
}

fn normalize_reporters(wire: Vec<String>) -> Result<Vec<String>, TestPlanError> {
    let allowed = ["human", "json", "junit"];
    let mut reporters = BTreeSet::new();
    for reporter in wire {
        if !allowed.contains(&reporter.as_str()) {
            return Err(TestPlanError::InvalidField {
                field: "reporters",
                message: format!("unsupported reporter `{reporter}`"),
            });
        }
        if !reporters.insert(reporter.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "reporter",
                value: reporter,
            });
        }
    }
    if reporters.is_empty() {
        return Err(TestPlanError::InvalidField {
            field: "reporters",
            message: "at least one reporter is required".into(),
        });
    }
    Ok(reporters.into_iter().collect())
}

fn normalize_artifact_store(wire: StoreWire) -> Result<TestArtifactStore, TestPlanError> {
    if !wire.content_addressed {
        return Err(TestPlanError::InvalidField {
            field: "artifact_store.content_addressed",
            message: "the store must be content-addressed".into(),
        });
    }
    Ok(TestArtifactStore {
        path: canonical_path("artifact_store.path", &wire.path, true)?,
        max_bytes: positive("artifact_store.max_bytes", wire.max_bytes)?,
    })
}

fn normalize_snapshot_stores(
    wire: Vec<SnapshotStoreWire>,
) -> Result<Vec<TestSnapshotStore>, TestPlanError> {
    let mut stores = Vec::with_capacity(wire.len());
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for store in wire {
        let name = canonical_identifier("snapshot_stores.name", &store.name)?;
        let path = canonical_path("snapshot_stores.path", &store.path, true)?;
        if !names.insert(name.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "snapshot store",
                value: name,
            });
        }
        if !paths.insert(path.clone()) {
            return Err(TestPlanError::Duplicate {
                kind: "snapshot store path",
                value: path,
            });
        }
        stores.push(TestSnapshotStore {
            name,
            path,
            update: store.update,
            max_bytes: positive("snapshot_stores.max_bytes", store.max_bytes)?,
        });
    }
    stores.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(stores)
}

fn normalize_target(
    wire: TestTargetWire,
    project: &ProjectPlan,
) -> Result<TestTarget, TestPlanError> {
    if wire.name != project.target_name() {
        return Err(TestPlanError::ProjectMismatch {
            field: "target.name",
            expected: project.target_name().into(),
            actual: wire.name,
        });
    }
    if wire.profile != project.profile().as_str() {
        return Err(TestPlanError::ProjectMismatch {
            field: "target.profile",
            expected: project.profile().as_str().into(),
            actual: wire.profile,
        });
    }
    if wire.capability_registry != CAPABILITY_REGISTRY {
        return Err(TestPlanError::InvalidField {
            field: "target.capability_registry",
            message: format!("unsupported registry `{}`", wire.capability_registry),
        });
    }
    let capabilities = sorted_unique_strings("target.capabilities", wire.capabilities)?;
    let expected_capabilities = project
        .capabilities()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if capabilities != expected_capabilities {
        return Err(TestPlanError::ProjectMismatch {
            field: "target.capabilities",
            expected: expected_capabilities.join(","),
            actual: capabilities.join(","),
        });
    }
    let features = sorted_unique_strings("target.features", wire.features)?;
    let expected_features = project
        .features()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if features != expected_features {
        return Err(TestPlanError::ProjectMismatch {
            field: "target.features",
            expected: expected_features.join(","),
            actual: features.join(","),
        });
    }
    Ok(TestTarget {
        name: project.target_name().into(),
        profile: project.profile().as_str().into(),
        capabilities,
        features,
    })
}

fn normalize_time_catalog(wire: TimeCatalogWire) -> Result<TimeCatalog, TestPlanError> {
    if wire.package != TIME_CATALOG_PACKAGE
        || wire.module != TIME_CATALOG_MODULE
        || wire.api != TIME_CATALOG_API
    {
        return Err(TestPlanError::InvalidField {
            field: "time_catalog",
            message: format!(
                "expected `{TIME_CATALOG_PACKAGE}.{TIME_CATALOG_MODULE}@{TIME_CATALOG_API}`"
            ),
        });
    }
    Ok(TimeCatalog {
        package: wire.package,
        module: wire.module,
        api: wire.api,
    })
}

fn normalize_limits(wire: LimitsWire) -> Result<TestLimits, TestPlanError> {
    Ok(TestLimits {
        timeout_ms: positive("limits.timeout_ms", wire.timeout_ms)?,
        setup_timeout_ms: positive("limits.setup_timeout_ms", wire.setup_timeout_ms)?,
        teardown_timeout_ms: positive("limits.teardown_timeout_ms", wire.teardown_timeout_ms)?,
        output_bytes: positive("limits.output_bytes", wire.output_bytes)?,
        artifact_bytes: positive("limits.artifact_bytes", wire.artifact_bytes)?,
        snapshot_bytes: positive("limits.snapshot_bytes", wire.snapshot_bytes)?,
        memory_bytes: positive("limits.memory_bytes", wire.memory_bytes)?,
        instructions: positive("limits.instructions", wire.instructions)?,
        virtual_timers: positive("limits.virtual_timers", wire.virtual_timers)?,
    })
}

fn canonical_repository_root(value: &str) -> Result<String, TestPlanError> {
    if value == "." || value.is_empty() {
        return Ok(String::new());
    }
    canonical_path("repository_root", value, true)
}

fn canonical_root_path(field: &'static str, value: &str) -> Result<String, TestPlanError> {
    if value.is_empty() || value == "." {
        return Ok(String::new());
    }
    canonical_path(field, value, false)
}

fn canonical_path(
    field: &'static str,
    value: &str,
    allow_file: bool,
) -> Result<String, TestPlanError> {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') || value.contains('\n') {
        return Err(TestPlanError::InvalidField {
            field,
            message: "path must be a non-empty relative slash-separated path".into(),
        });
    }
    let mut components = Vec::new();
    for component in value.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(TestPlanError::InvalidField {
                field,
                message: "path contains an empty, `.` or `..` component".into(),
            });
        }
        if component.starts_with('@') && component == "@generated" {
            return Err(TestPlanError::InvalidField {
                field,
                message: "`@generated` is reserved".into(),
            });
        }
        components.push(component);
    }
    let normalized = components.join("/");
    if !allow_file && normalized.ends_with(".to") {
        return Err(TestPlanError::InvalidField {
            field,
            message: "a source root cannot be a source file".into(),
        });
    }
    Ok(normalized)
}

fn path_within(path: &str, root: &str) -> bool {
    root.is_empty()
        || path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn source_parent(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_owned())
        .unwrap_or_default()
}

fn canonical_identity(field: &'static str, value: &str) -> Result<String, TestPlanError> {
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(TestPlanError::InvalidField {
            field,
            message: "identity must be non-empty and contain no line breaks".into(),
        });
    }
    Ok(value.to_owned())
}

fn canonical_identifier(field: &'static str, value: &str) -> Result<String, TestPlanError> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || !value.as_bytes()[0].is_ascii_lowercase()
    {
        return Err(TestPlanError::InvalidField {
            field,
            message: "expected a lowercase ASCII identifier".into(),
        });
    }
    Ok(value.to_owned())
}

fn non_empty_text(field: &'static str, value: Option<String>) -> Result<String, TestPlanError> {
    let value = value.ok_or_else(|| TestPlanError::InvalidField {
        field,
        message: "a non-empty value is required".into(),
    })?;
    if value.is_empty() || value.contains('\n') || value.contains('\r') {
        return Err(TestPlanError::InvalidField {
            field,
            message: "value must be non-empty and contain no line breaks".into(),
        });
    }
    Ok(value)
}

fn positive(field: &'static str, value: u64) -> Result<u64, TestPlanError> {
    if value == 0 {
        return Err(TestPlanError::InvalidField {
            field,
            message: "value must be positive".into(),
        });
    }
    Ok(value)
}

fn normalize_seed(value: &str) -> Result<String, TestPlanError> {
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(TestPlanError::InvalidField {
            field: "order.seed",
            message: "seed must contain one to sixteen ASCII hexadecimal digits".into(),
        });
    }
    let parsed = u64::from_str_radix(value, 16).map_err(|_| TestPlanError::InvalidField {
        field: "order.seed",
        message: "seed is outside the U64 range".into(),
    })?;
    Ok(format!("{parsed:016x}"))
}

fn sorted_unique_strings(
    field: &'static str,
    values: Vec<String>,
) -> Result<Vec<String>, TestPlanError> {
    let mut output = BTreeSet::new();
    for value in values {
        canonical_identity(field, &value)?;
        if !output.insert(value.clone()) {
            return Err(TestPlanError::Duplicate { kind: field, value });
        }
    }
    Ok(output.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{CAPABILITY_REGISTRY, sha256};
    use crate::package::PackageId;
    use crate::project::{LockedSourceWire, ProjectPlan, package_content_hash};
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn project_fixture() -> (ProjectPlan, String, String) {
        let package = "workspace:app@1";
        let source = b"fn main() {}\n";
        let manifest = serde_json::to_vec(&json!({
            "format": "tondo-manifest-draft",
            "target": {
                "name": "tondo-vm-hosted",
                "profile": "hosted",
                "capability_registry": CAPABILITY_REGISTRY,
                "capabilities": ["console", "process"],
                "features": ["fast"]
            },
            "root": {"package": package, "source": "app/src/main.to", "form": "module"},
            "standard": "toolchain:std:0.1-bootstrap",
            "packages": [{
                "id": package,
                "local_name": "app",
                "edition": "0.1",
                "dependencies": [],
                "source_sets": [{
                    "id": "common",
                    "sources": [{
                        "physical_path": "app/src/main.to",
                        "logical_path": "src/main.to",
                        "module": "main"
                    }]
                }]
            }],
            "generator_inputs": [],
            "privileged_units": []
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
            "format": "tondo-lock-draft",
            "manifest_hash": sha256(&manifest),
            "standard": {
                "package_id": "toolchain:std:0.1-bootstrap",
                "content_hash": crate::project::bootstrap_standard_hash()
            },
            "packages": [{
                "id": package,
                "content_hash": content_hash,
                "dependencies": [],
                "sources": [source_record],
                "interface": null
            }],
            "generator_inputs": [],
            "privileged_units": []
        }))
        .unwrap();
        let project = ProjectPlan::parse(&manifest, &lockfile)
            .unwrap_or_else(|error| panic!("fixture project failed: {error}"));
        (project, sha256(&manifest), sha256(&lockfile))
    }

    fn plan_json(manifest_hash: &str, lockfile_hash: &str) -> Value {
        json!({
            "format": TEST_PLAN_FORMAT,
            "project": {"manifest_hash": manifest_hash, "lockfile_hash": lockfile_hash},
            "repository_root": ".",
            "roots": [
                {"class": "production", "physical_path": "app/src", "logical_path": "src"},
                {"class": "unit-test", "physical_path": "app/src", "logical_path": "src"},
                {"class": "integration-test", "physical_path": "tests", "logical_path": "tests"}
            ],
            "sources": [
                {"class": "production", "package": "workspace:app@1", "physical_path": "app/src/main.to", "logical_path": "src/main.to", "module": "main", "input": "source:production:app/src/main.to"},
                {"class": "unit-test", "package": "workspace:app@1", "physical_path": "app/src/main_test.to", "logical_path": "src/main_test.to", "module": "main", "input": "source:unit-test:app/src/main_test.to"},
                {"class": "integration-test", "package": "test:integration:app:smoke", "physical_path": "tests/smoke.to", "logical_path": "tests/smoke.to", "module": "smoke", "input": "source:integration-test:tests/smoke.to"}
            ],
            "dev_dependencies": [],
            "codeowners": {"mode": "auto"},
            "selector": {"kind": "none"},
            "shard": null,
            "order": {"kind": "canonical"},
            "policy": {"jobs": 1, "allow_empty": false, "fail_fast": false, "retry": 0, "repeat": 1},
            "reporters": ["human", "json"],
            "artifact_store": {"path": "target/test-artifacts", "content_addressed": true, "max_bytes": 1048576},
            "snapshot_stores": [{"name": "default", "path": "tests/snapshots", "update": false, "max_bytes": 1048576}],
            "target": {"name": "tondo-vm-hosted", "profile": "hosted", "capability_registry": CAPABILITY_REGISTRY, "capabilities": ["console", "process"], "features": ["fast"]},
            "time_catalog": {"package": "std", "module": "time", "api": "monotonic-v1"},
            "limits": {"timeout_ms": 1000, "setup_timeout_ms": 1000, "teardown_timeout_ms": 1000, "output_bytes": 65536, "artifact_bytes": 1048576, "snapshot_bytes": 1048576, "memory_bytes": 67108864, "instructions": 1000000, "virtual_timers": 1024}
        })
    }

    #[test]
    fn plan_normalizes_all_source_classes_and_execution_metadata() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let bytes = serde_json::to_vec(&plan_json(&manifest_hash, &lockfile_hash)).unwrap();
        let plan = TestProjectPlan::parse(&project, &bytes).unwrap();
        assert_eq!(plan.manifest_hash(), manifest_hash);
        assert_eq!(plan.lockfile_hash(), lockfile_hash);
        assert_eq!(plan.repository_root(), "");
        assert_eq!(plan.roots().len(), 3);
        assert_eq!(plan.roots()[0].class(), TestSourceClass::Production);
        assert_eq!(plan.roots()[0].physical_path(), "app/src");
        assert_eq!(plan.roots()[0].logical_path(), "src");
        assert_eq!(plan.sources().len(), 3);
        assert_eq!(plan.sources()[0].class(), TestSourceClass::Production);
        assert_eq!(plan.sources()[0].package(), "workspace:app@1");
        assert_eq!(plan.sources()[0].physical_path(), "app/src/main.to");
        assert_eq!(plan.sources()[0].logical_path(), "src/main.to");
        assert_eq!(plan.sources()[0].module(), "main");
        assert_eq!(
            plan.sources()[0].input(),
            "source:production:app/src/main.to"
        );
        assert_eq!(plan.sources()[1].class(), TestSourceClass::UnitTest);
        assert_eq!(plan.sources()[2].class(), TestSourceClass::IntegrationTest);
        assert_eq!(
            plan.input_names().collect::<Vec<_>>(),
            vec![
                "source:production:app/src/main.to",
                "source:unit-test:app/src/main_test.to",
                "source:integration-test:tests/smoke.to"
            ]
        );
        assert!(plan.dev_dependencies().is_empty());
        assert_eq!(plan.codeowners().as_str(), "auto");
        assert_eq!(plan.selector().kind(), "none");
        assert_eq!(plan.selector().value(), None);
        assert_eq!(plan.shard(), None);
        assert_eq!(plan.order().kind(), "canonical");
        assert_eq!(plan.order().seed(), None);
        assert_eq!(plan.policy().jobs(), 1);
        assert!(!plan.policy().allow_empty());
        assert!(!plan.policy().fail_fast());
        assert_eq!(plan.policy().retry(), 0);
        assert_eq!(plan.policy().repeat(), 1);
        assert_eq!(plan.reporters(), &["human".to_owned(), "json".to_owned()]);
        assert_eq!(plan.artifact_store().path(), "target/test-artifacts");
        assert_eq!(plan.artifact_store().max_bytes(), 1_048_576);
        assert_eq!(plan.snapshot_stores().len(), 1);
        assert_eq!(plan.snapshot_stores()[0].name(), "default");
        assert_eq!(plan.snapshot_stores()[0].path(), "tests/snapshots");
        assert!(!plan.snapshot_stores_implicit());
        assert!(!plan.snapshot_stores()[0].update());
        assert_eq!(plan.snapshot_stores()[0].max_bytes(), 1_048_576);
        assert_eq!(plan.target().name(), "tondo-vm-hosted");
        assert_eq!(plan.target().profile(), "hosted");
        assert_eq!(
            plan.target().capabilities(),
            &["console".to_owned(), "process".to_owned()]
        );
        assert_eq!(plan.target().features(), &["fast"]);
        assert_eq!(plan.time_catalog().package(), "std");
        assert_eq!(plan.time_catalog().module(), "time");
        assert_eq!(plan.time_catalog().api(), "monotonic-v1");
        assert_eq!(plan.limits().timeout_ms(), 1000);
        assert_eq!(plan.limits().setup_timeout_ms(), 1000);
        assert_eq!(plan.limits().teardown_timeout_ms(), 1000);
        assert_eq!(plan.limits().output_bytes(), 65_536);
        assert_eq!(plan.limits().artifact_bytes(), 1_048_576);
        assert_eq!(plan.limits().snapshot_bytes(), 1_048_576);
        assert_eq!(plan.limits().memory_bytes(), 67_108_864);
        assert_eq!(plan.limits().instructions(), 1_000_000);
        assert_eq!(plan.limits().virtual_timers(), 1024);
    }

    #[test]
    fn defaults_materialize_from_the_closed_project_without_a_sidecar() {
        let (project, _, _) = project_fixture();
        let plan = TestProjectPlan::defaults(&project, 4);
        assert_eq!(plan.manifest_hash(), project.manifest_hash());
        assert_eq!(plan.lockfile_hash(), project.lockfile_hash());
        assert_eq!(plan.roots().len(), 1);
        assert_eq!(plan.roots()[0].physical_path(), "app/src");
        assert_eq!(plan.roots()[0].logical_path(), "src");
        assert_eq!(plan.sources().len(), 1);
        assert_eq!(plan.sources()[0].class(), TestSourceClass::Production);
        assert_eq!(
            plan.sources()[0].input(),
            "source:production:app/src/main.to"
        );
        assert_eq!(plan.policy().jobs(), 4);
        assert_eq!(plan.policy().retry(), 0);
        assert_eq!(plan.policy().repeat(), 1);
        assert_eq!(plan.limits().timeout_ms(), 30_000);
        assert_eq!(plan.artifact_store().max_bytes(), 16 * 1024 * 1024);
        assert_eq!(plan.snapshot_stores().len(), 1);
        assert_eq!(plan.snapshot_stores()[0].name(), "default");
        assert_eq!(plan.snapshot_stores()[0].path(), "tests/snapshots.json");
        assert!(plan.snapshot_stores_implicit());
        assert!(
            project
                .parse_test_plan(&plan.canonical_bytes().unwrap())
                .is_ok()
        );
    }

    #[test]
    fn canonical_bytes_are_stable_and_normalize_seed_and_order() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut value = plan_json(&manifest_hash, &lockfile_hash);
        value["order"] = json!({"kind": "random", "seed": "5EEd"});
        value["reporters"] = json!(["json", "human"]);
        let plan = TestProjectPlan::parse(&project, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(plan.order().seed(), Some("0000000000005eed"));
        let canonical = String::from_utf8(plan.canonical_bytes().unwrap()).unwrap();
        assert!(canonical.contains("0000000000005eed"));
        assert!(canonical.contains("[\"human\",\"json\"]"));
    }

    #[test]
    fn plan_rejects_unknown_fields_and_project_mismatches() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut unknown = plan_json(&manifest_hash, &lockfile_hash);
        unknown["unexpected"] = json!(true);
        assert!(matches!(
            TestProjectPlan::parse(&project, &serde_json::to_vec(&unknown).unwrap()),
            Err(TestPlanError::InvalidJson(_))
        ));
        let mut mismatch = plan_json(&manifest_hash, &lockfile_hash);
        mismatch["project"]["manifest_hash"] = json!(sha256(b"different"));
        assert!(matches!(
            TestProjectPlan::parse(&project, &serde_json::to_vec(&mismatch).unwrap()),
            Err(TestPlanError::ProjectMismatch {
                field: "manifest_hash",
                ..
            })
        ));
    }

    #[test]
    fn plan_rejects_inferred_roots_duplicate_inputs_and_production_drift() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut no_root = plan_json(&manifest_hash, &lockfile_hash);
        no_root["roots"] = json!([]);
        assert!(matches!(
            TestProjectPlan::parse(&project, &serde_json::to_vec(&no_root).unwrap()),
            Err(TestPlanError::InvalidField { field: "roots", .. })
        ));
        let mut duplicate = plan_json(&manifest_hash, &lockfile_hash);
        duplicate["sources"][1]["input"] = duplicate["sources"][0]["input"].clone();
        assert!(matches!(
            TestProjectPlan::parse(&project, &serde_json::to_vec(&duplicate).unwrap()),
            Err(TestPlanError::Duplicate {
                kind: "source input",
                ..
            })
        ));
        let mut drift = plan_json(&manifest_hash, &lockfile_hash);
        drift["sources"][0]["physical_path"] = json!("app/src/other.to");
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&drift).unwrap()).is_err());
    }

    #[test]
    fn plan_rejects_ambiguous_policy_selection_and_budget_values() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["selector"] = json!({"kind": "none", "value": "unexpected"});
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["shard"] = json!({"index": 2, "count": 1});
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["policy"]["jobs"] = json!(0);
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["limits"]["timeout_ms"] = json!(0);
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    #[test]
    fn plan_rejects_target_time_and_store_contract_drift() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        for (field, value) in [
            (
                "target",
                json!({"name":"wrong","profile":"hosted","capability_registry":CAPABILITY_REGISTRY,"capabilities":["console","process"],"features":["fast"]}),
            ),
            (
                "time_catalog",
                json!({"package":"std","module":"wall","api":"monotonic-v1"}),
            ),
            (
                "artifact_store",
                json!({"path":"target/test-artifacts","content_addressed":false,"max_bytes":1}),
            ),
        ] {
            let mut invalid = plan_json(&manifest_hash, &lockfile_hash);
            invalid[field] = value;
            assert!(
                TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err(),
                "field {field} unexpectedly accepted"
            );
        }
    }

    #[test]
    fn plan_rejects_invalid_dev_dependency_hash_and_codeowners_mode() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["dev_dependencies"] = json!([{
            "alias": "testing",
            "package": "workspace:test-support@1",
            "interface_path": "interfaces/testing.json",
            "sha256": "bad"
        }]);
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid = plan_json(&manifest_hash, &lockfile_hash);
        invalid["codeowners"] = json!({"mode":"path"});
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&invalid).unwrap()).is_err());
    }

    #[test]
    fn plan_has_no_ambient_input_path_or_source_order_dependence() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let mut value = plan_json(&manifest_hash, &lockfile_hash);
        value["repository_root"] = json!("/tmp/repo");
        assert!(TestProjectPlan::parse(&project, &serde_json::to_vec(&value).unwrap()).is_err());
        let mut shuffled = plan_json(&manifest_hash, &lockfile_hash);
        let sources = shuffled["sources"].as_array_mut().unwrap();
        sources.swap(0, 2);
        let plan =
            TestProjectPlan::parse(&project, &serde_json::to_vec(&shuffled).unwrap()).unwrap();
        assert_eq!(plan.sources()[0].class(), TestSourceClass::Production);
        assert_eq!(plan.sources()[2].class(), TestSourceClass::IntegrationTest);
    }

    #[test]
    fn public_api_does_not_expose_supplied_source_bytes() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let plan = TestProjectPlan::parse(
            &project,
            &serde_json::to_vec(&plan_json(&manifest_hash, &lockfile_hash)).unwrap(),
        )
        .unwrap();
        let canonical = plan.canonical_bytes().unwrap();
        assert!(
            !canonical
                .windows(12)
                .any(|window| window == b"fn main() {}")
        );
        let _ = Arc::<[u8]>::from(canonical);
    }

    #[test]
    fn public_plan_views_are_stable_and_do_not_expose_source_contents() {
        let (project, manifest_hash, lockfile_hash) = project_fixture();
        let plan = TestProjectPlan::parse(
            &project,
            &serde_json::to_vec(&plan_json(&manifest_hash, &lockfile_hash)).unwrap(),
        )
        .unwrap();
        assert_eq!(std::hint::black_box(plan.manifest_hash()), manifest_hash);
        assert_eq!(std::hint::black_box(plan.lockfile_hash()), lockfile_hash);
        assert_eq!(std::hint::black_box(plan.repository_root()), "");
        for root in std::hint::black_box(plan.roots()) {
            let _ = (root.class(), root.physical_path(), root.logical_path());
        }
        for source in std::hint::black_box(plan.sources()) {
            let _ = (
                source.class(),
                source.package(),
                source.physical_path(),
                source.logical_path(),
                source.module(),
                source.input(),
            );
        }
        assert_eq!(
            std::hint::black_box(plan.input_names())
                .collect::<Vec<_>>()
                .len(),
            3
        );
        assert!(std::hint::black_box(plan.dev_dependencies()).is_empty());
        assert_eq!(std::hint::black_box(plan.codeowners()).as_str(), "auto");
        assert_eq!(std::hint::black_box(plan.selector()).kind(), "none");
        assert_eq!(std::hint::black_box(plan.shard()), None);
        assert_eq!(std::hint::black_box(plan.order()).kind(), "canonical");
        let policy = std::hint::black_box(plan.policy());
        assert_eq!((policy.jobs(), policy.retry(), policy.repeat()), (1, 0, 1));
        assert!(!policy.allow_empty() && !policy.fail_fast());
        assert_eq!(std::hint::black_box(plan.reporters()), &["human", "json"]);
        assert_eq!(
            std::hint::black_box(plan.artifact_store()).path(),
            "target/test-artifacts"
        );
        assert_eq!(std::hint::black_box(plan.snapshot_stores()).len(), 1);
        assert_eq!(
            std::hint::black_box(plan.target()).name(),
            "tondo-vm-hosted"
        );
        assert_eq!(std::hint::black_box(plan.target()).profile(), "hosted");
        assert_eq!(std::hint::black_box(plan.target()).capabilities().len(), 2);
        assert_eq!(std::hint::black_box(plan.target()).features(), &["fast"]);
        assert_eq!(std::hint::black_box(plan.time_catalog()).package(), "std");
        let limits = std::hint::black_box(plan.limits());
        assert_eq!(limits.timeout_ms(), 1000);
        assert_eq!(limits.setup_timeout_ms(), 1000);
        assert_eq!(limits.teardown_timeout_ms(), 1000);
        assert_eq!(limits.output_bytes(), 65_536);
        assert_eq!(limits.artifact_bytes(), 1_048_576);
        assert_eq!(limits.snapshot_bytes(), 1_048_576);
        assert_eq!(limits.memory_bytes(), 67_108_864);
        assert_eq!(limits.instructions(), 1_000_000);
        assert_eq!(limits.virtual_timers(), 1024);
        assert!(
            !plan
                .canonical_bytes()
                .unwrap()
                .windows(12)
                .any(|window| window == b"fn main() {}")
        );
    }

    #[test]
    fn closed_value_helpers_cover_all_selector_policy_and_storage_shapes() {
        assert_eq!(TestSourceClass::Production.as_str(), "production");
        assert_eq!(TestSourceClass::UnitTest.as_str(), "unit-test");
        assert_eq!(
            TestSourceClass::IntegrationTest.as_str(),
            "integration-test"
        );
        assert!(TestSourceClass::parse("unknown").is_err());

        let codeowners = normalize_codeowners(CodeownersWire {
            mode: "path".into(),
            path: Some(".github/CODEOWNERS".into()),
        })
        .unwrap();
        assert_eq!(codeowners.as_str(), "path");
        assert_eq!(codeowners.path(), Some(".github/CODEOWNERS"));
        assert_eq!(CodeownersMode::None.as_str(), "none");
        assert_eq!(CodeownersMode::Auto.path(), None);
        assert_eq!(CodeownersMode::Path("owners".into()).path(), Some("owners"));
        assert!(
            normalize_codeowners(CodeownersWire {
                mode: "invalid".into(),
                path: None
            })
            .is_err()
        );
        assert!(
            normalize_codeowners(CodeownersWire {
                mode: "none".into(),
                path: Some("owners".into())
            })
            .is_err()
        );

        for (kind, expected) in [
            ("none", TestSelector::None),
            ("filter", TestSelector::Filter("slow".into())),
            ("glob", TestSelector::Glob("tests/**".into())),
            ("exact", TestSelector::Exact("suite::one".into())),
        ] {
            let value = normalize_selector(SelectorWire {
                kind: kind.into(),
                value: expected.value().map(str::to_owned),
            })
            .unwrap();
            assert_eq!(value.kind(), kind);
            assert_eq!(value.value(), expected.value());
        }
        assert!(
            normalize_selector(SelectorWire {
                kind: "filter".into(),
                value: None
            })
            .is_err()
        );
        assert!(
            normalize_selector(SelectorWire {
                kind: "other".into(),
                value: None
            })
            .is_err()
        );

        let shard = normalize_shard(Some(ShardWire { index: 2, count: 3 }))
            .unwrap()
            .unwrap();
        assert_eq!((shard.index(), shard.count()), (2, 3));
        assert!(normalize_shard(Some(ShardWire { index: 0, count: 1 })).is_err());
        assert_eq!(normalize_shard(None).unwrap(), None);
        assert_eq!(
            normalize_order(OrderWire {
                kind: "canonical".into(),
                seed: None
            })
            .unwrap()
            .kind(),
            "canonical"
        );
        assert_eq!(
            normalize_order(OrderWire {
                kind: "random".into(),
                seed: Some("a".into())
            })
            .unwrap()
            .seed(),
            Some("000000000000000a")
        );
        assert!(
            normalize_order(OrderWire {
                kind: "canonical".into(),
                seed: Some("a".into())
            })
            .is_err()
        );
        assert!(
            normalize_order(OrderWire {
                kind: "other".into(),
                seed: None
            })
            .is_err()
        );

        let policy = normalize_policy(PolicyWire {
            jobs: 2,
            allow_empty: true,
            fail_fast: true,
            retry: 3,
            repeat: 1,
        })
        .unwrap();
        assert_eq!((policy.jobs(), policy.retry(), policy.repeat()), (2, 3, 1));
        assert!(policy.allow_empty() && policy.fail_fast());
        assert!(
            normalize_policy(PolicyWire {
                jobs: 0,
                allow_empty: false,
                fail_fast: false,
                retry: 0,
                repeat: 1
            })
            .is_err()
        );
        assert_eq!(
            normalize_reporters(vec!["junit".into(), "human".into()]).unwrap(),
            vec!["human", "junit"]
        );
        assert!(normalize_reporters(Vec::new()).is_err());
        assert!(normalize_reporters(vec!["human".into(), "human".into()]).is_err());

        let store = normalize_snapshot_stores(vec![SnapshotStoreWire {
            name: "snap".into(),
            path: "tests/snapshots".into(),
            update: true,
            max_bytes: 9,
        }])
        .unwrap();
        assert_eq!(
            (
                store[0].name(),
                store[0].path(),
                store[0].update(),
                store[0].max_bytes()
            ),
            ("snap", "tests/snapshots", true, 9)
        );
        assert!(
            normalize_snapshot_stores(vec![
                SnapshotStoreWire {
                    name: "snap".into(),
                    path: "a".into(),
                    update: false,
                    max_bytes: 1
                },
                SnapshotStoreWire {
                    name: "snap".into(),
                    path: "b".into(),
                    update: false,
                    max_bytes: 1
                },
            ])
            .is_err()
        );
        assert!(
            normalize_artifact_store(StoreWire {
                path: "a".into(),
                content_addressed: false,
                max_bytes: 1
            })
            .is_err()
        );
        assert!(
            normalize_artifact_store(StoreWire {
                path: "a".into(),
                content_addressed: true,
                max_bytes: 0
            })
            .is_err()
        );

        assert!(canonical_repository_root(".").unwrap().is_empty());
        assert!(canonical_identity("identity", "bad\nvalue").is_err());
        assert!(canonical_identifier("identifier", "Bad").is_err());
        assert!(non_empty_text("value", None).is_err());
        assert!(non_empty_text("value", Some("bad\nvalue".into())).is_err());
        assert_eq!(normalize_seed("f").unwrap(), "000000000000000f");
        assert!(normalize_seed("").is_err());
        assert!(normalize_seed("10000000000000000").is_err());

        for error in [
            TestPlanError::InvalidJson("bad".into()),
            TestPlanError::InvalidField {
                field: "x",
                message: "bad".into(),
            },
            TestPlanError::ProjectMismatch {
                field: "x",
                expected: "a".into(),
                actual: "b".into(),
            },
            TestPlanError::Duplicate {
                kind: "x",
                value: "v".into(),
            },
            TestPlanError::Serialization("bad".into()),
        ] {
            assert!(!error.to_string().is_empty());
        }
    }
}
