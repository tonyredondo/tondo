//! Explicit, isolated retry rounds for the test runner.
//!
//! Retry planning is pure and deterministic.  The runtime campaign performs
//! the initial full round and then asks the planner for only eligible error,
//! panic, and timeout units.  Every round calls `RuntimeRunner::run`, which
//! creates fresh workers, envelopes and resource registries.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::test_result::{RetryUnit, RetryUnitKind};
use crate::test_runtime::{LeafProgram, RuntimeError, RuntimeRunner, RuntimeStatus, WorkerInfo};

pub const TEST_RETRY_FORMAT: &str = "tondo-test-retry-0.1/1";
pub const MAX_RETRY_ROUNDS: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetryCause {
    FailedError,
    FailedPanic,
    Timeout,
}

impl RetryCause {
    pub const fn from_status(status: RuntimeStatus) -> Option<Self> {
        match status {
            RuntimeStatus::FailedError => Some(Self::FailedError),
            RuntimeStatus::FailedPanic => Some(Self::FailedPanic),
            RuntimeStatus::Timeout => Some(Self::Timeout),
            RuntimeStatus::Passed
            | RuntimeStatus::Skipped
            | RuntimeStatus::ResourceLimit
            | RuntimeStatus::Infrastructure
            | RuntimeStatus::BlockedSetup => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    max_additional_rounds: u32,
    allow_flaky: bool,
    repeat_count: u32,
    update_snapshots: bool,
}

impl RetryPolicy {
    pub fn new(max_additional_rounds: u32) -> Result<Self, RetryError> {
        if max_additional_rounds > MAX_RETRY_ROUNDS {
            return Err(RetryError::RoundsOutOfRange {
                value: max_additional_rounds,
            });
        }
        Ok(Self {
            max_additional_rounds,
            allow_flaky: false,
            repeat_count: 1,
            update_snapshots: false,
        })
    }

    pub const fn max_additional_rounds(&self) -> u32 {
        self.max_additional_rounds
    }

    pub const fn allow_flaky(&self) -> bool {
        self.allow_flaky
    }

    pub const fn repeat_count(&self) -> u32 {
        self.repeat_count
    }

    pub const fn update_snapshots(&self) -> bool {
        self.update_snapshots
    }

    pub const fn with_allow_flaky(mut self, allow: bool) -> Self {
        self.allow_flaky = allow;
        self
    }

    pub const fn with_repeat_count(mut self, count: u32) -> Self {
        self.repeat_count = count;
        self
    }

    pub const fn with_update_snapshots(mut self, update: bool) -> Self {
        self.update_snapshots = update;
        self
    }

    pub fn validate(&self) -> Result<(), RetryError> {
        if self.max_additional_rounds > MAX_RETRY_ROUNDS {
            return Err(RetryError::RoundsOutOfRange {
                value: self.max_additional_rounds,
            });
        }
        if self.repeat_count != 1 && self.max_additional_rounds != 0 {
            return Err(RetryError::Incompatible(
                "retry and repeat cannot be combined",
            ));
        }
        if self.update_snapshots && self.max_additional_rounds != 0 {
            return Err(RetryError::Incompatible(
                "retry and snapshot update cannot be combined",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryContext {
    shard: String,
    target: String,
    inputs_hash: String,
    seed: u64,
    order: String,
    capabilities: Vec<String>,
    limits: BTreeMap<String, u64>,
    artifact_store: String,
    snapshot_store: String,
}

impl RetryContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        shard: impl Into<String>,
        target: impl Into<String>,
        inputs_hash: impl Into<String>,
        seed: u64,
        order: impl Into<String>,
        capabilities: impl IntoIterator<Item = String>,
        limits: BTreeMap<String, u64>,
        artifact_store: impl Into<String>,
        snapshot_store: impl Into<String>,
    ) -> Result<Self, RetryError> {
        let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
        capabilities.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        capabilities.dedup();
        let context = Self {
            shard: shard.into(),
            target: target.into(),
            inputs_hash: inputs_hash.into(),
            seed,
            order: order.into(),
            capabilities,
            limits,
            artifact_store: artifact_store.into(),
            snapshot_store: snapshot_store.into(),
        };
        context.validate()?;
        Ok(context)
    }

    pub fn shard(&self) -> &str {
        &self.shard
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn inputs_hash(&self) -> &str {
        &self.inputs_hash
    }

    pub const fn seed(&self) -> u64 {
        self.seed
    }

    pub fn order(&self) -> &str {
        &self.order
    }

    pub fn capabilities(&self) -> &[String] {
        &self.capabilities
    }

    pub fn limits(&self) -> &BTreeMap<String, u64> {
        &self.limits
    }

    pub fn artifact_store(&self) -> &str {
        &self.artifact_store
    }

    pub fn snapshot_store(&self) -> &str {
        &self.snapshot_store
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RetryError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| RetryError::Serialization(error.to_string()))
    }

    fn validate(&self) -> Result<(), RetryError> {
        for value in [
            &self.shard,
            &self.target,
            &self.inputs_hash,
            &self.order,
            &self.artifact_store,
            &self.snapshot_store,
        ] {
            if value.trim().is_empty() || value.contains(['\n', '\r']) {
                return Err(RetryError::InvalidContext);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryNodeKind {
    Test,
    Suite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryNode {
    id: String,
    kind: RetryNodeKind,
    parent: Option<String>,
    leaves: Vec<String>,
}

impl RetryNode {
    pub fn test(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: RetryNodeKind::Test,
            parent: None,
            leaves: Vec::new(),
        }
    }

    pub fn suite(
        id: impl Into<String>,
        parent: Option<impl Into<String>>,
        leaves: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: RetryNodeKind::Suite,
            parent: parent.map(Into::into),
            leaves: leaves.into_iter().collect(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn kind(&self) -> RetryNodeKind {
        self.kind
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub fn leaves(&self) -> &[String] {
        &self.leaves
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPlan {
    round: u32,
    units: Vec<RetryUnit>,
    context: RetryContext,
}

impl RetryPlan {
    pub const fn round(&self) -> u32 {
        self.round
    }

    pub fn units(&self) -> &[RetryUnit] {
        &self.units
    }

    pub fn context(&self) -> &RetryContext {
        &self.context
    }
}

#[derive(Debug, Clone)]
pub struct RetryPlanner {
    execution_plan: Vec<String>,
    nodes: BTreeMap<String, RetryNode>,
    context: RetryContext,
}

impl RetryPlanner {
    pub fn new(
        execution_plan: impl IntoIterator<Item = String>,
        nodes: impl IntoIterator<Item = RetryNode>,
        context: RetryContext,
    ) -> Result<Self, RetryError> {
        let execution_plan = execution_plan.into_iter().collect::<Vec<_>>();
        if execution_plan.is_empty() || execution_plan.iter().any(|id| id.trim().is_empty()) {
            return Err(RetryError::InvalidPlan(
                "execution plan must contain non-empty leaves",
            ));
        }
        let mut seen_leaves = BTreeSet::new();
        if execution_plan
            .iter()
            .any(|id| !seen_leaves.insert(id.clone()))
        {
            return Err(RetryError::InvalidPlan(
                "execution plan contains duplicates",
            ));
        }
        let mut map = BTreeMap::new();
        for node in nodes {
            if node.id.trim().is_empty() || map.insert(node.id.clone(), node).is_some() {
                return Err(RetryError::InvalidPlan(
                    "retry node identity is duplicated or empty",
                ));
            }
        }
        for (id, node) in &map {
            if let Some(parent) = node.parent.as_deref() {
                let parent_node = map
                    .get(parent)
                    .ok_or(RetryError::InvalidPlan("retry parent is unknown"))?;
                if parent_node.kind != RetryNodeKind::Suite {
                    return Err(RetryError::InvalidPlan(
                        "a test cannot contain a retry child",
                    ));
                }
            }
            match node.kind {
                RetryNodeKind::Test => {
                    if !execution_plan.contains(id) {
                        return Err(RetryError::InvalidPlan(
                            "retry test is outside execution plan",
                        ));
                    }
                }
                RetryNodeKind::Suite => {
                    if node.leaves.is_empty()
                        || node
                            .leaves
                            .iter()
                            .any(|leaf| !execution_plan.contains(leaf))
                    {
                        return Err(RetryError::InvalidPlan("retry suite leaves are invalid"));
                    }
                }
            }
        }
        Ok(Self {
            execution_plan,
            nodes: map,
            context,
        })
    }

    pub fn plan(
        &self,
        statuses: &BTreeMap<String, RuntimeStatus>,
        round: u32,
    ) -> Result<RetryPlan, RetryError> {
        let mut chosen_suites = BTreeSet::new();
        for node in self.nodes.values() {
            if node.kind == RetryNodeKind::Suite
                && node
                    .id
                    .as_str()
                    .pipe(|id| statuses.get(id).copied())
                    .and_then(RetryCause::from_status)
                    .is_some()
                && !self.has_eligible_ancestor(node, statuses)
            {
                chosen_suites.insert(node.id.clone());
            }
        }
        let mut units = Vec::new();
        for (index, leaf) in self.execution_plan.iter().enumerate() {
            let Some(cause) = statuses
                .get(leaf)
                .copied()
                .and_then(RetryCause::from_status)
            else {
                continue;
            };
            if let Some(suite) = self.outer_suite_for_leaf(leaf, &chosen_suites) {
                let _ = suite;
                continue;
            }
            units.push((
                index,
                RetryUnit {
                    kind: RetryUnitKind::Test,
                    id: leaf.clone(),
                    execution_plan: vec![leaf.clone()],
                },
                cause,
            ));
        }
        for suite_id in chosen_suites {
            let node = &self.nodes[&suite_id];
            let first = node
                .leaves
                .iter()
                .filter_map(|leaf| self.execution_plan.iter().position(|id| id == leaf))
                .min()
                .unwrap_or(usize::MAX);
            units.push((
                first,
                RetryUnit {
                    kind: RetryUnitKind::Suite,
                    id: suite_id,
                    execution_plan: self
                        .execution_plan
                        .iter()
                        .filter(|leaf| node.leaves.contains(leaf))
                        .cloned()
                        .collect(),
                },
                RetryCause::FailedError,
            ));
        }
        units.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.id.as_bytes().cmp(right.1.id.as_bytes()))
        });
        units.dedup_by(|left, right| left.1.id == right.1.id);
        let _causes = units.iter().map(|(_, _, cause)| *cause).collect::<Vec<_>>();
        Ok(RetryPlan {
            round,
            units: units.into_iter().map(|(_, unit, _)| unit).collect(),
            context: self.context.clone(),
        })
    }

    fn has_eligible_ancestor(
        &self,
        node: &RetryNode,
        statuses: &BTreeMap<String, RuntimeStatus>,
    ) -> bool {
        let mut parent = node.parent.as_deref();
        while let Some(id) = parent {
            if statuses
                .get(id)
                .copied()
                .and_then(RetryCause::from_status)
                .is_some()
            {
                return true;
            }
            parent = self.nodes.get(id).and_then(RetryNode::parent);
        }
        false
    }

    fn outer_suite_for_leaf<'a>(
        &'a self,
        leaf: &str,
        chosen: &BTreeSet<String>,
    ) -> Option<&'a str> {
        self.nodes
            .values()
            .filter(|node| {
                node.kind == RetryNodeKind::Suite
                    && chosen.contains(&node.id)
                    && node.leaves.iter().any(|candidate| candidate == leaf)
            })
            .min_by_key(|node| node.leaves.len())
            .map(|node| node.id.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryAttempt {
    id: String,
    round: u32,
    unit: Option<u32>,
    cause: Option<RetryCause>,
    status: RuntimeStatus,
    worker: WorkerInfo,
}

impl RetryAttempt {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub const fn round(&self) -> u32 {
        self.round
    }
    pub const fn unit(&self) -> Option<u32> {
        self.unit
    }
    pub const fn cause(&self) -> Option<RetryCause> {
        self.cause
    }
    pub const fn status(&self) -> RuntimeStatus {
        self.status
    }
    pub const fn worker(&self) -> WorkerInfo {
        self.worker
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryOutcome {
    id: String,
    attempts: Vec<RetryAttempt>,
    decisive: RuntimeStatus,
    flaky_pass: bool,
}

impl RetryOutcome {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn attempts(&self) -> &[RetryAttempt] {
        &self.attempts
    }
    pub const fn decisive(&self) -> RuntimeStatus {
        self.decisive
    }
    pub const fn flaky_pass(&self) -> bool {
        self.flaky_pass
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryReport {
    rounds: u32,
    outcomes: Vec<RetryOutcome>,
    attempts: Vec<RetryAttempt>,
    exit_pass: bool,
    context: RetryContext,
}

impl RetryReport {
    pub const fn rounds(&self) -> u32 {
        self.rounds
    }
    pub fn outcomes(&self) -> &[RetryOutcome] {
        &self.outcomes
    }
    pub fn attempts(&self) -> &[RetryAttempt] {
        &self.attempts
    }
    pub const fn exit_pass(&self) -> bool {
        self.exit_pass
    }
    pub fn context(&self) -> &RetryContext {
        &self.context
    }
}

pub struct RetryCampaign {
    runner: RuntimeRunner,
    policy: RetryPolicy,
    context: RetryContext,
}

impl fmt::Debug for RetryCampaign {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RetryCampaign")
            .field("policy", &self.policy)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl RetryCampaign {
    pub fn new(
        runner: RuntimeRunner,
        policy: RetryPolicy,
        context: RetryContext,
    ) -> Result<Self, RetryError> {
        policy.validate()?;
        Ok(Self {
            runner,
            policy,
            context,
        })
    }

    pub fn run(&self, programs: Vec<LeafProgram>) -> Result<RetryReport, RetryError> {
        let by_id = programs
            .iter()
            .map(|program| (program.id().to_owned(), program.clone()))
            .collect::<BTreeMap<_, _>>();
        if by_id.len() != programs.len() {
            return Err(RetryError::Runtime(RuntimeError::DuplicateLeaf(
                "duplicate".into(),
            )));
        }
        let initial = self.runner.run(programs).map_err(RetryError::Runtime)?;
        let mut statuses = initial
            .leaves()
            .iter()
            .map(|leaf| (leaf.id().to_owned(), leaf.status()))
            .collect::<BTreeMap<_, _>>();
        let mut attempts = initial
            .leaves()
            .iter()
            .map(|leaf| RetryAttempt {
                id: leaf.id().into(),
                round: 0,
                unit: None,
                cause: RetryCause::from_status(leaf.status()),
                status: leaf.status(),
                worker: leaf.worker(),
            })
            .collect::<Vec<_>>();
        let mut rounds = 0;
        for round in 1..=self.policy.max_additional_rounds {
            let ids = statuses
                .iter()
                .filter_map(|(id, status)| RetryCause::from_status(*status).map(|_| id.clone()))
                .collect::<Vec<_>>();
            if ids.is_empty() {
                break;
            }
            let retry_programs = ids
                .iter()
                .filter_map(|id| by_id.get(id).cloned())
                .collect::<Vec<_>>();
            let report = self
                .runner
                .run(retry_programs)
                .map_err(RetryError::Runtime)?;
            rounds = round;
            for leaf in report.leaves() {
                let cause = RetryCause::from_status(leaf.status());
                statuses.insert(leaf.id().into(), leaf.status());
                attempts.push(RetryAttempt {
                    id: leaf.id().into(),
                    round,
                    unit: Some(attempts_for_id(&attempts, leaf.id()) as u32),
                    cause,
                    status: leaf.status(),
                    worker: leaf.worker(),
                });
            }
        }
        let mut outcomes = Vec::new();
        for id in by_id.keys() {
            let mut leaf_attempts = attempts
                .iter()
                .filter(|attempt| attempt.id == *id)
                .cloned()
                .collect::<Vec<_>>();
            leaf_attempts.sort_by_key(|attempt| attempt.round);
            let decisive = leaf_attempts
                .last()
                .map_or(RuntimeStatus::Infrastructure, |a| a.status);
            let flaky_pass = leaf_attempts.len() > 1
                && decisive == RuntimeStatus::Passed
                && leaf_attempts
                    .iter()
                    .any(|attempt| attempt.status != RuntimeStatus::Passed);
            outcomes.push(RetryOutcome {
                id: id.clone(),
                attempts: leaf_attempts,
                decisive,
                flaky_pass,
            });
        }
        let exit_pass = outcomes.iter().all(|outcome| {
            outcome.decisive == RuntimeStatus::Passed
                && (self.policy.allow_flaky || !outcome.flaky_pass)
        });
        Ok(RetryReport {
            rounds,
            outcomes,
            attempts,
            exit_pass,
            context: self.context.clone(),
        })
    }
}

fn attempts_for_id(attempts: &[RetryAttempt], id: &str) -> usize {
    attempts.iter().filter(|attempt| attempt.id == id).count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryError {
    RoundsOutOfRange { value: u32 },
    Incompatible(&'static str),
    InvalidContext,
    InvalidPlan(&'static str),
    Serialization(String),
    Runtime(RuntimeError),
}

impl fmt::Display for RetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RoundsOutOfRange { value } => {
                write!(formatter, "retry rounds {value} exceed {MAX_RETRY_ROUNDS}")
            }
            Self::Incompatible(message) | Self::InvalidPlan(message) => {
                formatter.write_str(message)
            }
            Self::InvalidContext => {
                formatter.write_str("retry context contains an empty or newline value")
            }
            Self::Serialization(message) => {
                write!(formatter, "retry context serialization failed: {message}")
            }
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl Error for RetryError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_control::EnvelopeLimits;
    use crate::test_runtime::RuntimeConfig;

    fn context() -> RetryContext {
        RetryContext::new(
            "shard-0/2",
            "x86_64",
            "inputs-hash",
            7,
            "sha256-tree-v1",
            ["network".into(), "clock".into()],
            BTreeMap::from([("timeout_ns".into(), 100)]),
            "artifact-store",
            "snapshot-store",
        )
        .unwrap()
    }

    #[test]
    fn policy_is_finite_and_rejects_stateful_combinations() {
        assert!(matches!(
            RetryPolicy::new(MAX_RETRY_ROUNDS + 1),
            Err(RetryError::RoundsOutOfRange { .. })
        ));
        let retry_repeat = RetryPolicy::new(1).unwrap().with_repeat_count(2);
        assert_eq!(
            retry_repeat.validate(),
            Err(RetryError::Incompatible(
                "retry and repeat cannot be combined"
            ))
        );
        let update = RetryPolicy::new(1).unwrap().with_update_snapshots(true);
        assert_eq!(
            update.validate(),
            Err(RetryError::Incompatible(
                "retry and snapshot update cannot be combined"
            ))
        );
        assert_eq!(RetryPolicy::new(0).unwrap().max_additional_rounds(), 0);
    }

    #[test]
    fn context_is_canonical_and_rejects_empty_or_newline_fields() {
        let first = context();
        let second = RetryContext::new(
            "shard-0/2",
            "x86_64",
            "inputs-hash",
            7,
            "sha256-tree-v1",
            ["clock".into(), "network".into()],
            BTreeMap::from([("timeout_ns".into(), 100)]),
            "artifact-store",
            "snapshot-store",
        )
        .unwrap();
        assert_eq!(
            first.canonical_bytes().unwrap(),
            second.canonical_bytes().unwrap()
        );
        assert_eq!(first.shard(), "shard-0/2");
        assert_eq!(first.target(), "x86_64");
        assert_eq!(first.seed(), 7);
        assert!(
            RetryContext::new(
                "",
                "target",
                "hash",
                0,
                "order",
                [],
                BTreeMap::new(),
                "a",
                "s"
            )
            .is_err()
        );
        assert!(
            RetryContext::new(
                "s",
                "target\n",
                "hash",
                0,
                "order",
                [],
                BTreeMap::new(),
                "a",
                "s"
            )
            .is_err()
        );
    }

    #[test]
    fn planner_absorbs_descendant_failures_under_the_outer_suite() {
        let planner = RetryPlanner::new(
            vec!["suite::a".into(), "suite::b".into(), "solo".into()],
            vec![
                RetryNode::suite(
                    "suite",
                    None::<String>,
                    vec!["suite::a".into(), "suite::b".into()],
                ),
                RetryNode::test("suite::a"),
                RetryNode::test("suite::b"),
                RetryNode::test("solo"),
            ],
            context(),
        )
        .unwrap();
        let statuses = BTreeMap::from([
            ("suite".into(), RuntimeStatus::FailedPanic),
            ("suite::a".into(), RuntimeStatus::FailedError),
            ("suite::b".into(), RuntimeStatus::Passed),
            ("solo".into(), RuntimeStatus::Timeout),
        ]);
        let plan = planner.plan(&statuses, 1).unwrap();
        assert_eq!(plan.round(), 1);
        assert_eq!(
            plan.units()
                .iter()
                .map(|unit| unit.id.as_str())
                .collect::<Vec<_>>(),
            ["suite", "solo"]
        );
        assert_eq!(plan.units()[0].execution_plan, ["suite::a", "suite::b"]);
        assert_eq!(plan.context().seed(), 7);
    }

    #[test]
    fn planner_excludes_non_retryable_statuses_and_orders_by_first_leaf() {
        let planner = RetryPlanner::new(
            vec!["z".into(), "a".into()],
            vec![RetryNode::test("z"), RetryNode::test("a")],
            context(),
        )
        .unwrap();
        let statuses = BTreeMap::from([
            ("z".into(), RuntimeStatus::Infrastructure),
            ("a".into(), RuntimeStatus::FailedError),
        ]);
        let plan = planner.plan(&statuses, 2).unwrap();
        assert_eq!(
            plan.units()
                .iter()
                .map(|unit| unit.id.as_str())
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn planner_rejects_malformed_trees_before_retrying() {
        assert!(matches!(
            RetryPlanner::new(
                vec!["a".into()],
                vec![RetryNode::test("missing")],
                context()
            ),
            Err(RetryError::InvalidPlan(_))
        ));
        assert!(matches!(
            RetryPlanner::new(
                vec!["a".into()],
                vec![RetryNode::suite("suite", None::<String>, vec![])],
                context()
            ),
            Err(RetryError::InvalidPlan(_))
        ));
    }

    #[test]
    fn campaign_runs_initial_round_then_only_eligible_failures_in_fresh_workers() {
        let config = RuntimeConfig::new(2, EnvelopeLimits::new(1_000, 1_000, 1_000)).unwrap();
        let runner = RuntimeRunner::new(config).unwrap();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let flaky_calls = calls.clone();
        let flaky = LeafProgram::new("flaky", move |_| {
            let call = flaky_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if call == 0 {
                Err(crate::test_runtime::RunError::Error {
                    code: "E".into(),
                    message: "flaky".into(),
                })
            } else {
                Ok(())
            }
        });
        let stable = LeafProgram::new("stable", |_| Ok(()));
        let campaign = RetryCampaign::new(runner, RetryPolicy::new(2).unwrap(), context()).unwrap();
        let report = campaign.run(vec![flaky, stable]).unwrap();
        assert_eq!(report.rounds(), 1);
        let outcome = report
            .outcomes()
            .iter()
            .find(|outcome| outcome.id() == "flaky")
            .unwrap();
        assert_eq!(outcome.attempts().len(), 2);
        assert!(outcome.flaky_pass());
        assert!(!report.exit_pass());
        assert!(outcome.attempts()[0].worker() != outcome.attempts()[1].worker());
        assert_eq!(report.context().artifact_store(), "artifact-store");
    }

    #[test]
    fn allow_flaky_changes_only_exit_policy_and_non_retryable_failures_stop() {
        let config = RuntimeConfig::new(1, EnvelopeLimits::new(1_000, 1_000, 1_000)).unwrap();
        let runner = RuntimeRunner::new(config).unwrap();
        let allow = RetryCampaign::new(
            runner,
            RetryPolicy::new(1).unwrap().with_allow_flaky(true),
            context(),
        )
        .unwrap();
        let report = allow
            .run(vec![LeafProgram::new("fail", |_| {
                Err(crate::test_runtime::RunError::Error {
                    code: "E".into(),
                    message: "no".into(),
                })
            })])
            .unwrap();
        assert_eq!(report.rounds(), 1);
        assert!(!report.exit_pass());
        assert_eq!(report.outcomes()[0].attempts().len(), 2);
        assert!(!report.outcomes()[0].flaky_pass());

        let runner = RuntimeRunner::new(
            RuntimeConfig::new(1, EnvelopeLimits::new(1_000, 1_000, 1_000)).unwrap(),
        )
        .unwrap();
        let report = RetryCampaign::new(runner, RetryPolicy::new(3).unwrap(), context())
            .unwrap()
            .run(vec![LeafProgram::new("skip", |context| {
                context.skip("skip")
            })])
            .unwrap();
        assert_eq!(report.rounds(), 0);
        assert_eq!(report.outcomes()[0].attempts().len(), 1);
    }

    #[test]
    fn public_accessors_and_status_matrix_are_closed() {
        let policy = RetryPolicy::new(2)
            .unwrap()
            .with_allow_flaky(true)
            .with_repeat_count(1)
            .with_update_snapshots(false);
        assert!(policy.allow_flaky());
        assert_eq!(policy.repeat_count(), 1);
        assert!(!policy.update_snapshots());
        for status in [
            RuntimeStatus::Passed,
            RuntimeStatus::Skipped,
            RuntimeStatus::ResourceLimit,
            RuntimeStatus::Infrastructure,
            RuntimeStatus::BlockedSetup,
        ] {
            assert!(RetryCause::from_status(status).is_none());
        }
        assert_eq!(
            RetryCause::from_status(RuntimeStatus::FailedError),
            Some(RetryCause::FailedError)
        );
        assert_eq!(
            RetryCause::from_status(RuntimeStatus::FailedPanic),
            Some(RetryCause::FailedPanic)
        );
        assert_eq!(
            RetryCause::from_status(RuntimeStatus::Timeout),
            Some(RetryCause::Timeout)
        );

        let test = RetryNode::test("leaf");
        assert_eq!(test.id(), "leaf");
        assert_eq!(test.kind(), RetryNodeKind::Test);
        assert_eq!(test.parent(), None);
        assert!(test.leaves().is_empty());
        let suite = RetryNode::suite("suite", Some("root"), vec!["leaf".into()]);
        assert_eq!(suite.id(), "suite");
        assert_eq!(suite.kind(), RetryNodeKind::Suite);
        assert_eq!(suite.parent(), Some("root"));
        assert_eq!(suite.leaves(), &["leaf".to_owned()]);

        let worker =
            RuntimeRunner::new(RuntimeConfig::new(1, EnvelopeLimits::new(1, 1, 1)).unwrap())
                .unwrap()
                .run(vec![LeafProgram::new("worker", |_| Ok(())).clone()])
                .unwrap()
                .leaves()[0]
                .worker();
        let attempt = RetryAttempt {
            id: "leaf".into(),
            round: 1,
            unit: Some(2),
            cause: Some(RetryCause::Timeout),
            status: RuntimeStatus::Timeout,
            worker,
        };
        assert_eq!(attempt.id(), "leaf");
        assert_eq!(attempt.round(), 1);
        assert_eq!(attempt.unit(), Some(2));
        assert_eq!(attempt.cause(), Some(RetryCause::Timeout));
        assert_eq!(attempt.status(), RuntimeStatus::Timeout);
        assert_eq!(attempt.worker(), worker);
        let outcome = RetryOutcome {
            id: "leaf".into(),
            attempts: vec![attempt.clone()],
            decisive: RuntimeStatus::Timeout,
            flaky_pass: false,
        };
        assert_eq!(outcome.id(), "leaf");
        assert_eq!(outcome.attempts(), &[attempt]);
        assert_eq!(outcome.decisive(), RuntimeStatus::Timeout);
        assert!(!outcome.flaky_pass());

        let report = RetryReport {
            rounds: 1,
            outcomes: vec![outcome],
            attempts: vec![],
            exit_pass: false,
            context: context(),
        };
        assert_eq!(report.rounds(), 1);
        assert_eq!(report.outcomes().len(), 1);
        assert!(report.attempts().is_empty());
        assert!(!report.exit_pass());
        assert_eq!(report.context().order(), "sha256-tree-v1");

        let campaign = RetryCampaign::new(
            RuntimeRunner::new(RuntimeConfig::new(1, EnvelopeLimits::new(1, 1, 1)).unwrap())
                .unwrap(),
            policy,
            context(),
        )
        .unwrap();
        assert!(format!("{campaign:?}").contains("RetryCampaign"));
        assert_eq!(TEST_RETRY_FORMAT, "tondo-test-retry-0.1/1");
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}
