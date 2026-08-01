//! Canonical JSON reports and test lists.
//!
//! `TestResultTree` is the validated execution model.  This module adds the
//! one versioned JSON envelope shared by the interactive JSON output and file
//! reporters.  It deliberately keeps all metadata explicit and serializes
//! through small wire structs so structural node kind (`suite`/`test`) cannot
//! be confused with the source class (`unit`/`integration`) required by the
//! public schema.

#![allow(clippy::large_enum_variant, clippy::result_large_err)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::artifact::validate_sha256;
use crate::test_result::{
    AggregateStatus, ResultModelError, ResultPolicy, ResultSummary, RetryUnit, TestAttempt,
    TestNode, TestResultTree,
};

pub const TEST_JSON_FORMAT: &str = "tondo-test-json-v1";
pub const TEST_REPORT_FORMAT: &str = "tondo-test-report-0.1/7";
pub const TEST_LIST_FORMAT: &str = "tondo-test-list-0.1/6";
pub const TEST_ARTIFACT_FORMAT: &str = "tondo-test-artifacts-0.1/1";
pub const TEST_SNAPSHOT_FORMAT: &str = "tondo-snapshot-store-0.1/1";
pub const ARTIFACT_ALGORITHM: &str = "sha256-objects-v1";
pub const CANONICAL_ORDER_ALGORITHM: &str = "id-byte-order-v1";
pub const RANDOM_ORDER_ALGORITHM: &str = "sha256-tree-v1";
pub const SHARD_ALGORITHM: &str = "sha256-mod-v1";
pub const RETRY_ISOLATION: &str = "fresh-worker-v1";
pub const REPEAT_ISOLATION: &str = "fresh-worker-per-iteration-v1";

/// Selection mode recorded in a report.  `value` is null only for `all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionKind {
    All,
    Filter,
    Glob,
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSelection {
    pub kind: SelectionKind,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OwnershipMode {
    Auto,
    Explicit,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportOwnership {
    pub mode: OwnershipMode,
    pub source: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputReproducibility {
    Closed,
    SecretDependentVersioned,
    SecretDependentUnversioned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportInputs {
    pub public_sha256: String,
    pub secret_profile_sha256: Option<String>,
    pub secret_count: u32,
    pub reproducibility: InputReproducibility,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportTarget {
    pub name: String,
    pub profile: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportShard {
    pub index: u32,
    pub count: u32,
    pub algorithm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderMode {
    Canonical,
    Random,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportOrder {
    pub mode: OrderMode,
    pub seed: Option<String>,
    pub algorithm: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRetryRound {
    pub round: u32,
    pub units: Vec<RetryUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRetry {
    pub max_additional_rounds: u32,
    pub isolation: String,
    pub rounds: Vec<ReportRetryRound>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRepeat {
    pub count: u32,
    pub isolation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportArtifactStore {
    pub format: String,
    pub algorithm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnapshotMode {
    Check,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSnapshotPolicy {
    pub format: String,
    pub mode: SnapshotMode,
    pub before_sha256: String,
    pub after_sha256: String,
    pub published: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportPolicy {
    pub deny_skips: bool,
    pub allow_flaky: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportLimits {
    pub jobs: u32,
    pub timeout_ms: Option<u64>,
    pub resource_profile_sha256: String,
}

/// Metadata shared by report and list output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportMetadata {
    pub edition: String,
    pub target: ReportTarget,
    pub compiled: bool,
    pub selection: ReportSelection,
    pub ownership: ReportOwnership,
    pub inputs: ReportInputs,
    pub shard: Option<ReportShard>,
    pub order: ReportOrder,
    pub retry: ReportRetry,
    pub repeat: ReportRepeat,
    pub artifact_store: ReportArtifactStore,
    pub snapshot_policy: ReportSnapshotPolicy,
    pub policy: ReportPolicy,
    pub limits: ReportLimits,
}

impl Default for ReportMetadata {
    fn default() -> Self {
        let hash = "0".repeat(64);
        Self {
            edition: "0.1".into(),
            target: ReportTarget {
                name: "tondo-vm-hosted".into(),
                profile: "hosted".into(),
                capabilities: Vec::new(),
            },
            compiled: true,
            selection: ReportSelection {
                kind: SelectionKind::All,
                value: None,
            },
            ownership: ReportOwnership {
                mode: OwnershipMode::None,
                source: None,
                sha256: None,
            },
            inputs: ReportInputs {
                public_sha256: hash.clone(),
                secret_profile_sha256: None,
                secret_count: 0,
                reproducibility: InputReproducibility::Closed,
            },
            shard: None,
            order: ReportOrder {
                mode: OrderMode::Canonical,
                seed: None,
                algorithm: CANONICAL_ORDER_ALGORITHM.into(),
            },
            retry: ReportRetry {
                max_additional_rounds: 0,
                isolation: RETRY_ISOLATION.into(),
                rounds: Vec::new(),
            },
            repeat: ReportRepeat {
                count: 1,
                isolation: REPEAT_ISOLATION.into(),
            },
            artifact_store: ReportArtifactStore {
                format: TEST_ARTIFACT_FORMAT.into(),
                algorithm: ARTIFACT_ALGORITHM.into(),
            },
            snapshot_policy: ReportSnapshotPolicy {
                format: TEST_SNAPSHOT_FORMAT.into(),
                mode: SnapshotMode::Check,
                before_sha256: hash.clone(),
                after_sha256: hash,
                published: None,
            },
            policy: ReportPolicy {
                deny_skips: false,
                allow_flaky: false,
            },
            limits: ReportLimits {
                jobs: 1,
                timeout_ms: Some(30_000),
                resource_profile_sha256: "0".repeat(64),
            },
        }
    }
}

impl ReportMetadata {
    pub fn validate(&self) -> Result<(), ReportError> {
        validate_text("edition", &self.edition)?;
        if self.edition != "0.1" {
            return Err(invalid("edition", "expected `0.1`"));
        }
        validate_text("target.name", &self.target.name)?;
        validate_text("target.profile", &self.target.profile)?;
        validate_unique_sorted_or_sortable("target.capabilities", &self.target.capabilities)?;
        validate_selection(&self.selection)?;
        validate_ownership(&self.ownership)?;
        validate_hash("inputs.public_sha256", &self.inputs.public_sha256)?;
        if let Some(hash) = &self.inputs.secret_profile_sha256 {
            validate_hash("inputs.secret_profile_sha256", hash)?;
            if self.inputs.secret_count == 0 {
                return Err(invalid(
                    "inputs.secret_count",
                    "must be positive when a secret profile is present",
                ));
            }
        } else if self.inputs.secret_count != 0 {
            return Err(invalid(
                "inputs.secret_count",
                "must be zero without a secret profile",
            ));
        }
        validate_shard(self.shard.as_ref())?;
        validate_order(&self.order)?;
        validate_retry(&self.retry)?;
        if self.repeat.count == 0 || self.repeat.isolation != REPEAT_ISOLATION {
            return Err(invalid(
                "repeat",
                "count must be positive and isolation is fixed",
            ));
        }
        if self.repeat.count > 1 && self.retry.max_additional_rounds != 0 {
            return Err(invalid(
                "repeat",
                "retry and repeat cannot be active together",
            ));
        }
        if self.artifact_store.format != TEST_ARTIFACT_FORMAT
            || self.artifact_store.algorithm != ARTIFACT_ALGORITHM
        {
            return Err(invalid("artifact_store", "format and algorithm are fixed"));
        }
        validate_snapshot_policy(&self.snapshot_policy)?;
        if self.limits.jobs == 0 {
            return Err(invalid("limits.jobs", "must be positive"));
        }
        if self.limits.timeout_ms == Some(0) {
            return Err(invalid("limits.timeout_ms", "must be positive or null"));
        }
        validate_hash(
            "limits.resource_profile_sha256",
            &self.limits.resource_profile_sha256,
        )?;
        Ok(())
    }

    fn result_policy(&self) -> ResultPolicy {
        ResultPolicy {
            jobs: self.limits.jobs,
            deny_skips: self.policy.deny_skips,
            allow_flaky: self.policy.allow_flaky,
            max_additional_rounds: self.retry.max_additional_rounds,
            repeat_count: self.repeat.count,
        }
    }

    fn canonicalized(&self) -> Self {
        let mut value = self.clone();
        value.target.capabilities.sort();
        value
    }
}

/// Canonical, lossless report consumed by human and JUnit reporters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    metadata: ReportMetadata,
    tree: TestResultTree,
}

impl TestReport {
    pub fn assemble(
        metadata: ReportMetadata,
        execution_plan: Vec<String>,
        suites: Vec<TestNode>,
        tests: Vec<TestNode>,
    ) -> Result<Self, ReportError> {
        metadata.validate()?;
        let policy = metadata.result_policy();
        let tree = TestResultTree::assemble(execution_plan, policy, suites, tests)?;
        Self::from_tree(metadata, tree)
    }

    pub fn from_tree(metadata: ReportMetadata, tree: TestResultTree) -> Result<Self, ReportError> {
        metadata.validate()?;
        tree.validate()?;
        let expected_policy = metadata.result_policy();
        if tree.policy != expected_policy {
            return Err(invalid(
                "policy",
                "tree policy does not match report metadata",
            ));
        }
        let canonical_tree = TestResultTree::assemble(
            tree.execution_plan.clone(),
            tree.policy.clone(),
            tree.suites.clone(),
            tree.tests.clone(),
        )?;
        Ok(Self {
            metadata: metadata.canonicalized(),
            tree: canonical_tree,
        })
    }

    pub fn metadata(&self) -> &ReportMetadata {
        &self.metadata
    }

    pub fn tree(&self) -> &TestResultTree {
        &self.tree
    }

    pub fn suites(&self) -> &[TestNode] {
        self.tree.suites()
    }

    pub fn tests(&self) -> &[TestNode] {
        self.tree.tests()
    }

    pub fn summary(&self) -> &ResultSummary {
        self.tree.summary()
    }

    pub fn execution_plan(&self) -> &[String] {
        &self.tree.execution_plan
    }

    /// Serialize one compact JSON object and exactly one trailing LF.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReportError> {
        compact_line(&ReportWire::from_report(self)?)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ReportError> {
        let value = parse_line::<ReportWire>(bytes)?;
        value.into_report()
    }
}

/// Read-only list emitted by `--list --test-format json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestList {
    metadata: ReportMetadata,
    snapshot_store: SnapshotStoreIdentity,
    execution_plan: Vec<String>,
    suites: Vec<TestNode>,
    tests: Vec<TestNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotStoreIdentity {
    pub format: String,
    pub sha256: String,
}

impl TestList {
    pub fn new(
        metadata: ReportMetadata,
        snapshot_store: SnapshotStoreIdentity,
        execution_plan: Vec<String>,
        suites: Vec<TestNode>,
        tests: Vec<TestNode>,
    ) -> Result<Self, ReportError> {
        metadata.validate()?;
        validate_snapshot_identity(&snapshot_store)?;
        validate_list_nodes(&execution_plan, &suites, &tests)?;
        let mut value = Self {
            metadata: metadata.canonicalized(),
            snapshot_store,
            execution_plan,
            suites,
            tests,
        };
        value.suites.sort_by(|left, right| left.id.cmp(&right.id));
        value.tests.sort_by(|left, right| left.id.cmp(&right.id));
        value.metadata.target.capabilities.sort();
        Ok(value)
    }

    pub fn metadata(&self) -> &ReportMetadata {
        &self.metadata
    }

    pub fn snapshot_store(&self) -> &SnapshotStoreIdentity {
        &self.snapshot_store
    }

    pub fn execution_plan(&self) -> &[String] {
        &self.execution_plan
    }

    pub fn suites(&self) -> &[TestNode] {
        &self.suites
    }

    pub fn tests(&self) -> &[TestNode] {
        &self.tests
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReportError> {
        compact_line(&ListWire::from_list(self)?)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, ReportError> {
        let value = parse_line::<ListWire>(bytes)?;
        value.into_list()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportWire {
    format: String,
    edition: String,
    target: ReportTarget,
    compiled: bool,
    selection: ReportSelection,
    ownership: ReportOwnership,
    inputs: ReportInputs,
    shard: Option<ReportShard>,
    order: ReportOrder,
    execution_plan: Vec<String>,
    retry: ReportRetry,
    repeat: ReportRepeat,
    artifact_store: ReportArtifactStore,
    snapshot_policy: ReportSnapshotPolicy,
    policy: ReportPolicy,
    limits: ReportLimits,
    suites: Vec<ReportNodeWire>,
    tests: Vec<ReportNodeWire>,
    summary: ResultSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportNodeWire {
    id: String,
    parent: Option<String>,
    package: String,
    kind: String,
    module: String,
    path: Vec<String>,
    name: String,
    source: Option<crate::test_result::SourceSpan>,
    owners: Vec<String>,
    status: AggregateStatus,
    decisive_attempt: u32,
    attempts: Vec<TestAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListWire {
    format: String,
    edition: String,
    target: ReportTarget,
    compiled: bool,
    selection: ReportSelection,
    ownership: ReportOwnership,
    inputs: ReportInputs,
    snapshot_store: SnapshotStoreIdentity,
    shard: Option<ReportShard>,
    order: ReportOrder,
    execution_plan: Vec<String>,
    suites: Vec<ListNodeWire>,
    tests: Vec<ListNodeWire>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListNodeWire {
    id: String,
    parent: Option<String>,
    package: String,
    kind: String,
    module: String,
    path: Vec<String>,
    name: String,
    source: Option<crate::test_result::SourceSpan>,
    owners: Vec<String>,
}

impl ReportWire {
    fn from_report(report: &TestReport) -> Result<Self, ReportError> {
        Ok(Self {
            format: TEST_REPORT_FORMAT.into(),
            edition: report.metadata.edition.clone(),
            target: report.metadata.target.clone(),
            compiled: report.metadata.compiled,
            selection: report.metadata.selection.clone(),
            ownership: report.metadata.ownership.clone(),
            inputs: report.metadata.inputs.clone(),
            shard: report.metadata.shard.clone(),
            order: report.metadata.order.clone(),
            execution_plan: report.tree.execution_plan.clone(),
            retry: report.metadata.retry.clone(),
            repeat: report.metadata.repeat.clone(),
            artifact_store: report.metadata.artifact_store.clone(),
            snapshot_policy: report.metadata.snapshot_policy.clone(),
            policy: report.metadata.policy.clone(),
            limits: report.metadata.limits.clone(),
            suites: report
                .tree
                .suites
                .iter()
                .map(ReportNodeWire::from_node)
                .collect::<Result<_, _>>()?,
            tests: report
                .tree
                .tests
                .iter()
                .map(ReportNodeWire::from_node)
                .collect::<Result<_, _>>()?,
            summary: report.tree.summary.clone(),
        })
    }

    fn into_report(self) -> Result<TestReport, ReportError> {
        if self.format != TEST_REPORT_FORMAT {
            return Err(invalid("format", "unexpected test report format"));
        }
        let suites = self
            .suites
            .into_iter()
            .map(|node| node.into_node(crate::test_result::ResultNodeKind::Suite))
            .collect::<Result<Vec<_>, _>>()?;
        let tests = self
            .tests
            .into_iter()
            .map(|node| node.into_node(crate::test_result::ResultNodeKind::Test))
            .collect::<Result<Vec<_>, _>>()?;
        let metadata = ReportMetadata {
            edition: self.edition,
            target: self.target,
            compiled: self.compiled,
            selection: self.selection,
            ownership: self.ownership,
            inputs: self.inputs,
            shard: self.shard,
            order: self.order,
            retry: self.retry,
            repeat: self.repeat,
            artifact_store: self.artifact_store,
            snapshot_policy: self.snapshot_policy,
            policy: self.policy,
            limits: self.limits,
        };
        let tree = TestResultTree {
            format: TEST_REPORT_FORMAT.into(),
            edition: metadata.edition.clone(),
            execution_plan: self.execution_plan,
            policy: metadata.result_policy(),
            suites,
            tests,
            summary: self.summary,
        };
        tree.validate()?;
        TestReport::from_tree(metadata, tree)
    }
}

impl ReportNodeWire {
    fn from_node(node: &TestNode) -> Result<Self, ReportError> {
        Ok(Self {
            id: node.id.clone(),
            parent: node.parent.clone(),
            package: node.package.clone(),
            kind: source_kind(&node.id)?.into(),
            module: node.module.clone(),
            path: node.path.clone(),
            name: node.name.clone(),
            source: node.source.clone(),
            owners: node.owners.clone(),
            status: node.status,
            decisive_attempt: node.decisive_attempt,
            attempts: node.attempts.clone(),
        })
    }

    fn into_node(
        self,
        structural_kind: crate::test_result::ResultNodeKind,
    ) -> Result<TestNode, ReportError> {
        let expected = source_kind(&self.id)?;
        if expected != self.kind {
            return Err(invalid(
                "node.kind",
                "source class does not match node identity",
            ));
        }
        Ok(TestNode {
            id: self.id,
            parent: self.parent,
            package: self.package,
            kind: structural_kind,
            module: self.module,
            path: self.path,
            name: self.name,
            source: self.source,
            owners: self.owners,
            status: self.status,
            decisive_attempt: self.decisive_attempt,
            attempts: self.attempts,
        })
    }

    fn list(&self) -> ListNodeWire {
        ListNodeWire {
            id: self.id.clone(),
            parent: self.parent.clone(),
            package: self.package.clone(),
            kind: self.kind.clone(),
            module: self.module.clone(),
            path: self.path.clone(),
            name: self.name.clone(),
            source: self.source.clone(),
            owners: self.owners.clone(),
        }
    }
}

impl ListWire {
    fn from_list(list: &TestList) -> Result<Self, ReportError> {
        Ok(Self {
            format: TEST_LIST_FORMAT.into(),
            edition: list.metadata.edition.clone(),
            target: list.metadata.target.clone(),
            compiled: list.metadata.compiled,
            selection: list.metadata.selection.clone(),
            ownership: list.metadata.ownership.clone(),
            inputs: list.metadata.inputs.clone(),
            snapshot_store: list.snapshot_store.clone(),
            shard: list.metadata.shard.clone(),
            order: list.metadata.order.clone(),
            execution_plan: list.execution_plan.clone(),
            suites: list
                .suites
                .iter()
                .map(ReportNodeWire::from_node)
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .map(ReportNodeWire::list)
                .collect(),
            tests: list
                .tests
                .iter()
                .map(ReportNodeWire::from_node)
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .map(ReportNodeWire::list)
                .collect(),
        })
    }

    fn into_list(self) -> Result<TestList, ReportError> {
        if self.format != TEST_LIST_FORMAT {
            return Err(invalid("format", "unexpected test list format"));
        }
        let suites = self
            .suites
            .into_iter()
            .map(ListNodeWire::into_node_suite)
            .collect::<Result<Vec<_>, _>>()?;
        let tests = self
            .tests
            .into_iter()
            .map(ListNodeWire::into_node_test)
            .collect::<Result<Vec<_>, _>>()?;
        TestList::new(
            ReportMetadata {
                edition: self.edition,
                target: self.target,
                compiled: self.compiled,
                selection: self.selection,
                ownership: self.ownership,
                inputs: self.inputs,
                shard: self.shard,
                order: self.order,
                ..ReportMetadata::default()
            },
            self.snapshot_store,
            self.execution_plan,
            suites,
            tests,
        )
    }
}

impl ListNodeWire {
    fn into_node_suite(self) -> Result<TestNode, ReportError> {
        let expected = source_kind(&self.id)?;
        if expected != self.kind {
            return Err(invalid(
                "node.kind",
                "source class does not match node identity",
            ));
        }
        Ok(TestNode {
            id: self.id,
            parent: self.parent,
            package: self.package,
            kind: crate::test_result::ResultNodeKind::Suite,
            module: self.module,
            path: self.path,
            name: self.name,
            source: self.source,
            owners: self.owners,
            status: AggregateStatus::Passed,
            decisive_attempt: 0,
            attempts: Vec::new(),
        })
    }

    fn into_node_test(self) -> Result<TestNode, ReportError> {
        let mut node = self.into_node_suite()?;
        node.kind = crate::test_result::ResultNodeKind::Test;
        Ok(node)
    }
}

fn compact_line<T: Serialize>(value: &T) -> Result<Vec<u8>, ReportError> {
    let mut bytes =
        serde_json::to_vec(value).map_err(|error| ReportError::Serialization(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn parse_line<T>(bytes: &[u8]) -> Result<T, ReportError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    if bytes.len() < 2 || !bytes.ends_with(b"\n") || bytes[..bytes.len() - 1].contains(&b'\n') {
        return Err(invalid(
            "json",
            "canonical output requires exactly one trailing LF",
        ));
    }
    let payload = &bytes[..bytes.len() - 1];
    let value: T = serde_json::from_slice(payload)
        .map_err(|error| ReportError::InvalidJson(error.to_string()))?;
    let canonical = serde_json::to_vec(&value)
        .map_err(|error| ReportError::Serialization(error.to_string()))?;
    if canonical != payload {
        return Err(invalid("json", "bytes are not canonical"));
    }
    Ok(value)
}

fn validate_selection(selection: &ReportSelection) -> Result<(), ReportError> {
    match (selection.kind, selection.value.as_deref()) {
        (SelectionKind::All, None) => Ok(()),
        (SelectionKind::All, Some(_)) => Err(invalid("selection.value", "all must use null")),
        (_, Some(value)) if !value.is_empty() && !value.contains(['\n', '\r']) => Ok(()),
        _ => Err(invalid(
            "selection.value",
            "selector value must be non-empty text",
        )),
    }
}

fn validate_ownership(ownership: &ReportOwnership) -> Result<(), ReportError> {
    match ownership.mode {
        OwnershipMode::None if ownership.source.is_none() && ownership.sha256.is_none() => Ok(()),
        OwnershipMode::Auto | OwnershipMode::Explicit
            if ownership.source.is_some() && ownership.sha256.is_some() =>
        {
            validate_text("ownership.source", ownership.source.as_deref().unwrap())?;
            validate_hash("ownership.sha256", ownership.sha256.as_deref().unwrap())
        }
        _ => Err(invalid("ownership", "mode and source/hash must agree")),
    }
}

fn validate_shard(shard: Option<&ReportShard>) -> Result<(), ReportError> {
    if let Some(shard) = shard {
        if shard.index == 0 || shard.count == 0 || shard.index > shard.count {
            return Err(invalid("shard", "index must be in one-based count range"));
        }
        if shard.algorithm != SHARD_ALGORITHM {
            return Err(invalid("shard.algorithm", "algorithm is fixed"));
        }
    }
    Ok(())
}

fn validate_order(order: &ReportOrder) -> Result<(), ReportError> {
    match order.mode {
        OrderMode::Canonical
            if order.seed.is_none() && order.algorithm == CANONICAL_ORDER_ALGORITHM =>
        {
            Ok(())
        }
        OrderMode::Random if order.seed.is_some() && order.algorithm == RANDOM_ORDER_ALGORITHM => {
            let seed = order.seed.as_deref().unwrap();
            if (1..=16).contains(&seed.len())
                && seed
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                Ok(())
            } else {
                Err(invalid(
                    "order.seed",
                    "must be one to sixteen lowercase hex digits",
                ))
            }
        }
        _ => Err(invalid("order", "mode, seed and algorithm do not agree")),
    }
}

fn validate_retry(retry: &ReportRetry) -> Result<(), ReportError> {
    if retry.isolation != RETRY_ISOLATION {
        return Err(invalid("retry.isolation", "isolation is fixed"));
    }
    for (position, round) in retry.rounds.iter().enumerate() {
        if round.round != position as u32 + 1 || round.round > retry.max_additional_rounds {
            return Err(invalid(
                "retry.rounds",
                "rounds must be contiguous and bounded",
            ));
        }
        if round.units.is_empty() {
            return Err(invalid("retry.rounds.units", "units must not be empty"));
        }
        let mut ids = BTreeSet::new();
        for unit in &round.units {
            if unit.id.is_empty() || unit.execution_plan.is_empty() || !ids.insert(&unit.id) {
                return Err(invalid(
                    "retry.rounds.units",
                    "units must be unique and non-empty",
                ));
            }
            if unit.execution_plan.iter().any(String::is_empty) {
                return Err(invalid(
                    "retry.rounds.units.execution_plan",
                    "IDs must be non-empty",
                ));
            }
        }
    }
    Ok(())
}

fn validate_snapshot_policy(policy: &ReportSnapshotPolicy) -> Result<(), ReportError> {
    if policy.format != TEST_SNAPSHOT_FORMAT {
        return Err(invalid("snapshot_policy.format", "format is fixed"));
    }
    validate_hash("snapshot_policy.before_sha256", &policy.before_sha256)?;
    validate_hash("snapshot_policy.after_sha256", &policy.after_sha256)?;
    match policy.mode {
        SnapshotMode::Check if policy.published.is_none() => Ok(()),
        SnapshotMode::Update if policy.published.is_some() => Ok(()),
        _ => Err(invalid(
            "snapshot_policy.published",
            "value must match mode",
        )),
    }
}

fn validate_snapshot_identity(identity: &SnapshotStoreIdentity) -> Result<(), ReportError> {
    if identity.format != TEST_SNAPSHOT_FORMAT {
        return Err(invalid("snapshot_store.format", "format is fixed"));
    }
    validate_hash("snapshot_store.sha256", &identity.sha256)
}

fn validate_list_nodes(
    execution_plan: &[String],
    suites: &[TestNode],
    tests: &[TestNode],
) -> Result<(), ReportError> {
    let mut ids = BTreeSet::new();
    let suite_ids = suites
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    let test_ids = tests
        .iter()
        .map(|node| node.id.as_str())
        .collect::<BTreeSet<_>>();
    for node in suites.iter().chain(tests.iter()) {
        if node.id.is_empty() || !ids.insert(node.id.as_str()) {
            return Err(invalid("nodes", "IDs must be unique and non-empty"));
        }
        if suites.iter().any(|suite| suite.id == node.id)
            && node.kind != crate::test_result::ResultNodeKind::Suite
        {
            return Err(invalid("suites", "suite descriptors must have suite kind"));
        }
        if tests.iter().any(|test| test.id == node.id)
            && node.kind != crate::test_result::ResultNodeKind::Test
        {
            return Err(invalid("tests", "test descriptors must have test kind"));
        }
        source_kind(&node.id)?;
        if let Some(parent) = &node.parent
            && !suite_ids.contains(parent.as_str())
        {
            return Err(invalid(
                "node.parent",
                "parent must reference a listed suite",
            ));
        }
    }
    let mut plan = BTreeSet::new();
    for id in execution_plan {
        if id.is_empty() || !plan.insert(id.as_str()) || !test_ids.contains(id.as_str()) {
            return Err(invalid(
                "execution_plan",
                "must contain each listed test once",
            ));
        }
    }
    if plan.len() != tests.len() {
        return Err(invalid("execution_plan", "must contain all listed tests"));
    }
    Ok(())
}

fn source_kind(id: &str) -> Result<&'static str, ReportError> {
    let value = id.split("::").nth(1).unwrap_or("unit");
    match value {
        "unit" => Ok("unit"),
        "integration" => Ok("integration"),
        _ => Err(invalid(
            "node.id",
            "identity must contain unit or integration source class",
        )),
    }
}

fn validate_unique_sorted_or_sortable(
    field: &'static str,
    values: &[String],
) -> Result<(), ReportError> {
    let mut unique = BTreeSet::new();
    for value in values {
        validate_text(field, value)?;
        if !unique.insert(value) {
            return Err(invalid(field, "values must be unique"));
        }
    }
    Ok(())
}

fn validate_hash(field: &'static str, value: &str) -> Result<(), ReportError> {
    validate_text(field, value)?;
    validate_sha256(&format!("sha256:{value}")).map_err(|error| invalid(field, error.to_string()))
}

fn validate_text(field: &'static str, value: &str) -> Result<(), ReportError> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        Err(invalid(field, "must be non-empty and line-break free"))
    } else {
        Ok(())
    }
}

fn invalid(field: &'static str, message: impl Into<String>) -> ReportError {
    ReportError::InvalidField {
        field,
        message: message.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    InvalidJson(String),
    InvalidField {
        field: &'static str,
        message: String,
    },
    Model(ResultModelError),
    Serialization(String),
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => {
                write!(formatter, "invalid canonical test JSON: {message}")
            }
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Model(error) => error.fmt(formatter),
            Self::Serialization(message) => {
                write!(formatter, "cannot encode canonical test JSON: {message}")
            }
        }
    }
}

impl Error for ReportError {}

impl From<ResultModelError> for ReportError {
    fn from(error: ResultModelError) -> Self {
        Self::Model(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_result::{
        AttemptPhase, AttemptStatus, ResultNodeKind, RetryUnitKind, SnapshotRecord, SourceSpan,
        TestAttempt,
    };
    use serde_json::{Value, json};

    fn hash(ch: char) -> String {
        std::iter::repeat_n(ch, 64).collect()
    }

    fn node(id: &str, kind: ResultNodeKind) -> TestNode {
        let parent = if kind == ResultNodeKind::Test {
            Some("application::unit::math".into())
        } else {
            None
        };
        TestNode::new(
            id,
            parent,
            "application",
            kind,
            "math",
            id.rsplit("::").next().unwrap_or(id),
            vec![TestAttempt::new(1, 1, 0, None, AttemptStatus::Passed)],
        )
    }

    fn metadata() -> ReportMetadata {
        let mut value = ReportMetadata::default();
        value.inputs.public_sha256 = hash('a');
        value.snapshot_policy.before_sha256 = hash('b');
        value.snapshot_policy.after_sha256 = hash('b');
        value.limits.resource_profile_sha256 = hash('c');
        value
    }

    fn report() -> TestReport {
        TestReport::assemble(
            metadata(),
            vec!["application::unit::math::adds".into()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![node("application::unit::math::adds", ResultNodeKind::Test)],
        )
        .unwrap()
    }

    #[test]
    fn report_is_compact_lossless_and_round_trips() {
        let report = report();
        let bytes = report.canonical_bytes().unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes[..bytes.len() - 1].contains(&b'\n'));
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["format"], TEST_REPORT_FORMAT);
        assert_eq!(value["tests"][0]["kind"], "unit");
        assert_eq!(value["summary"]["artifact_bytes"], "0");
        assert_eq!(
            report.canonical_bytes().unwrap(),
            TestReport::parse(&bytes)
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
    }

    #[test]
    fn report_preserves_metadata_and_rejects_unknown_or_noncanonical_output() {
        let mut value: Value =
            serde_json::from_slice(&report().canonical_bytes().unwrap()).unwrap();
        value["unexpected"] = json!(true);
        let bytes = format!("{}\n", serde_json::to_string(&value).unwrap());
        assert!(matches!(
            TestReport::parse(bytes.as_bytes()),
            Err(ReportError::InvalidJson(_))
        ));
        let valid = report().canonical_bytes().unwrap();
        assert!(TestReport::parse(&valid[..valid.len() - 1]).is_err());
        assert!(TestReport::parse(&[valid.as_slice(), b"\n"].concat()).is_err());
        let pretty = format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::from_slice::<Value>(&valid).unwrap())
                .unwrap()
        );
        assert!(TestReport::parse(pretty.as_bytes()).is_err());
    }

    #[test]
    fn metadata_validation_closes_selection_ownership_inputs_and_order() {
        let mut value = metadata();
        value.selection.value = Some("bad".into());
        assert!(value.validate().is_err());
        value = metadata();
        value.ownership.mode = OwnershipMode::Auto;
        assert!(value.validate().is_err());
        value = metadata();
        value.inputs.secret_count = 1;
        assert!(value.validate().is_err());
        value = metadata();
        value.order.mode = OrderMode::Random;
        assert!(value.validate().is_err());
        value = metadata();
        value.shard = Some(ReportShard {
            index: 2,
            count: 1,
            algorithm: SHARD_ALGORITHM.into(),
        });
        assert!(value.validate().is_err());
    }

    #[test]
    fn retry_repeat_snapshot_and_artifact_policies_are_closed() {
        let mut value = metadata();
        value.retry.max_additional_rounds = 1;
        value.retry.rounds.push(ReportRetryRound {
            round: 1,
            units: vec![RetryUnit {
                kind: RetryUnitKind::Test,
                id: "application::unit::math::adds".into(),
                execution_plan: vec!["application::unit::math::adds".into()],
            }],
        });
        assert!(value.validate().is_ok());
        value.repeat.count = 2;
        assert!(value.validate().is_err());
        value = metadata();
        value.snapshot_policy.mode = SnapshotMode::Update;
        value.snapshot_policy.published = Some(false);
        assert!(value.validate().is_ok());
        value.artifact_store.algorithm = "other".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn list_contains_only_descriptors_and_round_trips() {
        let list = TestList::new(
            metadata(),
            SnapshotStoreIdentity {
                format: TEST_SNAPSHOT_FORMAT.into(),
                sha256: hash('d'),
            },
            vec!["application::unit::math::adds".into()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![node("application::unit::math::adds", ResultNodeKind::Test)],
        )
        .unwrap();
        let bytes = list.canonical_bytes().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["format"], TEST_LIST_FORMAT);
        assert!(value["tests"][0].get("attempts").is_none());
        assert_eq!(
            TestList::parse(&bytes).unwrap().canonical_bytes().unwrap(),
            bytes
        );
    }

    #[test]
    fn list_rejects_bad_plan_parent_and_snapshot_identity() {
        let base = metadata();
        assert!(
            TestList::new(
                base.clone(),
                SnapshotStoreIdentity {
                    format: "bad".into(),
                    sha256: hash('d')
                },
                vec![],
                vec![node("application::unit::math", ResultNodeKind::Suite)],
                vec![node("application::unit::math::adds", ResultNodeKind::Test)],
            )
            .is_err()
        );
        assert!(
            TestList::new(
                base.clone(),
                SnapshotStoreIdentity {
                    format: TEST_SNAPSHOT_FORMAT.into(),
                    sha256: hash('d')
                },
                vec!["missing".into()],
                vec![],
                vec![],
            )
            .is_err()
        );
        let mut child = node("application::unit::math::adds", ResultNodeKind::Test);
        child.parent = Some("missing".into());
        assert!(
            TestList::new(
                base,
                SnapshotStoreIdentity {
                    format: TEST_SNAPSHOT_FORMAT.into(),
                    sha256: hash('d')
                },
                vec![child.id.clone()],
                vec![node("application::unit::math", ResultNodeKind::Suite)],
                vec![child],
            )
            .is_err()
        );
    }

    #[test]
    fn report_rejects_mismatched_tree_policy_and_node_source_kind() {
        let mut value = metadata();
        value.policy.allow_flaky = true;
        let tree = TestResultTree::assemble(
            vec!["application::unit::math::adds".into()],
            ResultPolicy::default(),
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![node("application::unit::math::adds", ResultNodeKind::Test)],
        )
        .unwrap();
        assert!(TestReport::from_tree(value, tree).is_err());
        let mut malformed = report().canonical_bytes().unwrap();
        let mut json: Value = serde_json::from_slice(&malformed).unwrap();
        json["tests"][0]["kind"] = json!("integration");
        malformed = format!("{}\n", serde_json::to_string(&json).unwrap()).into_bytes();
        assert!(TestReport::parse(&malformed).is_err());
    }

    #[test]
    fn report_keeps_source_and_attempt_payloads() {
        let mut test = node("application::unit::math::adds", ResultNodeKind::Test);
        test.source = Some(SourceSpan {
            file: "tests/math.to".into(),
            start: 2,
            end: 9,
        });
        test.attempts[0].logs.push("hello".into());
        let mut meta = metadata();
        meta.target.capabilities = vec!["z".into(), "a".into()];
        let report = TestReport::assemble(
            meta,
            vec![test.id.clone()],
            vec![node("application::unit::math", ResultNodeKind::Suite)],
            vec![test],
        )
        .unwrap();
        let text = String::from_utf8(report.canonical_bytes().unwrap()).unwrap();
        assert!(text.find("\"a\"").unwrap() < text.find("\"z\"").unwrap());
        assert!(text.contains("tests/math.to"));
        assert!(text.contains("hello"));
        let _ = AttemptPhase::Setup;
        let _ = SnapshotRecord {
            name: "s".into(),
            status: crate::test_result::SnapshotStatus::Matched,
            expected_sha256: Some(hash('a')),
            actual_sha256: hash('a'),
        };
    }
}
