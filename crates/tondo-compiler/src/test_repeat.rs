//! Full, sequential and isolated repeat iterations for the test runner.
//!
//! Repeat is intentionally separate from retry.  Every iteration executes the
//! complete selected program set and receives fresh workers; no iteration is
//! scheduled while the previous one still owns resources or envelopes.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::test_control::EnvelopeReport;
use crate::test_runtime::{LeafProgram, RuntimeError, RuntimeRunner, RuntimeStatus, WorkerInfo};

pub const TEST_REPEAT_FORMAT: &str = "tondo-test-repeat-0.1/1";
pub const MAX_REPEAT_COUNT: u32 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatPolicy {
    count: u32,
    retry_count: u32,
    allow_flaky: bool,
    list_mode: bool,
    update_snapshots: bool,
}

impl RepeatPolicy {
    pub fn new(count: u32) -> Result<Self, RepeatError> {
        if count == 0 || count > MAX_REPEAT_COUNT {
            return Err(RepeatError::CountOutOfRange { value: count });
        }
        Ok(Self {
            count,
            retry_count: 0,
            allow_flaky: false,
            list_mode: false,
            update_snapshots: false,
        })
    }

    pub const fn count(&self) -> u32 {
        self.count
    }

    pub const fn retry_count(&self) -> u32 {
        self.retry_count
    }

    pub const fn allow_flaky(&self) -> bool {
        self.allow_flaky
    }

    pub const fn list_mode(&self) -> bool {
        self.list_mode
    }

    pub const fn update_snapshots(&self) -> bool {
        self.update_snapshots
    }

    pub const fn with_retry_count(mut self, count: u32) -> Self {
        self.retry_count = count;
        self
    }

    pub const fn with_allow_flaky(mut self, value: bool) -> Self {
        self.allow_flaky = value;
        self
    }

    pub const fn with_list_mode(mut self, value: bool) -> Self {
        self.list_mode = value;
        self
    }

    pub const fn with_update_snapshots(mut self, value: bool) -> Self {
        self.update_snapshots = value;
        self
    }

    pub fn validate(&self) -> Result<(), RepeatError> {
        if self.count == 0 || self.count > MAX_REPEAT_COUNT {
            return Err(RepeatError::CountOutOfRange { value: self.count });
        }
        if self.retry_count != 0 {
            return Err(RepeatError::Incompatible(
                "repeat and retry cannot be combined",
            ));
        }
        if self.allow_flaky {
            return Err(RepeatError::Incompatible(
                "repeat and allow-flaky cannot be combined",
            ));
        }
        if self.list_mode {
            return Err(RepeatError::Incompatible(
                "repeat and list cannot be combined",
            ));
        }
        if self.update_snapshots {
            return Err(RepeatError::Incompatible(
                "repeat and snapshot update cannot be combined",
            ));
        }
        Ok(())
    }
}

/// Immutable invocation identity copied into every iteration report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatContext {
    selection: Vec<String>,
    execution_plan: Vec<String>,
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

impl RepeatContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        selection: impl IntoIterator<Item = String>,
        execution_plan: impl IntoIterator<Item = String>,
        shard: impl Into<String>,
        target: impl Into<String>,
        inputs_hash: impl Into<String>,
        seed: u64,
        order: impl Into<String>,
        capabilities: impl IntoIterator<Item = String>,
        limits: BTreeMap<String, u64>,
        artifact_store: impl Into<String>,
        snapshot_store: impl Into<String>,
    ) -> Result<Self, RepeatError> {
        let mut capabilities = capabilities.into_iter().collect::<Vec<_>>();
        capabilities.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        capabilities.dedup();
        let context = Self {
            selection: selection.into_iter().collect(),
            execution_plan: execution_plan.into_iter().collect(),
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

    pub fn selection(&self) -> &[String] {
        &self.selection
    }

    pub fn execution_plan(&self) -> &[String] {
        &self.execution_plan
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

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, RepeatError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| RepeatError::Serialization(error.to_string()))
    }

    fn validate(&self) -> Result<(), RepeatError> {
        if self.selection.is_empty() || self.execution_plan.is_empty() {
            return Err(RepeatError::InvalidContext);
        }
        for value in [
            &self.shard,
            &self.target,
            &self.inputs_hash,
            &self.order,
            &self.artifact_store,
            &self.snapshot_store,
        ] {
            if value.trim().is_empty() || value.contains(['\n', '\r']) {
                return Err(RepeatError::InvalidContext);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatAttempt {
    id: String,
    iteration: u32,
    round: u32,
    unit: Option<u32>,
    status: RuntimeStatus,
    worker: WorkerInfo,
    report: EnvelopeReport,
}

impl RepeatAttempt {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn iteration(&self) -> u32 {
        self.iteration
    }

    pub const fn round(&self) -> u32 {
        self.round
    }

    pub const fn unit(&self) -> Option<u32> {
        self.unit
    }

    pub const fn status(&self) -> RuntimeStatus {
        self.status
    }

    pub const fn worker(&self) -> WorkerInfo {
        self.worker
    }

    pub fn report(&self) -> &EnvelopeReport {
        &self.report
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatOutcome {
    id: String,
    attempts: Vec<RepeatAttempt>,
    all_passed: bool,
    decisive: RuntimeStatus,
}

impl RepeatOutcome {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn attempts(&self) -> &[RepeatAttempt] {
        &self.attempts
    }

    pub const fn all_passed(&self) -> bool {
        self.all_passed
    }

    pub const fn decisive(&self) -> RuntimeStatus {
        self.decisive
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeatReport {
    iterations: u32,
    attempts: Vec<RepeatAttempt>,
    outcomes: Vec<RepeatOutcome>,
    exit_pass: bool,
    context: RepeatContext,
}

impl RepeatReport {
    pub const fn iterations(&self) -> u32 {
        self.iterations
    }

    pub fn attempts(&self) -> &[RepeatAttempt] {
        &self.attempts
    }

    pub fn outcomes(&self) -> &[RepeatOutcome] {
        &self.outcomes
    }

    pub const fn exit_pass(&self) -> bool {
        self.exit_pass
    }

    pub fn context(&self) -> &RepeatContext {
        &self.context
    }
}

pub struct RepeatCampaign {
    runner: RuntimeRunner,
    policy: RepeatPolicy,
    context: RepeatContext,
}

impl fmt::Debug for RepeatCampaign {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RepeatCampaign")
            .field("policy", &self.policy)
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}

impl RepeatCampaign {
    pub fn new(
        runner: RuntimeRunner,
        policy: RepeatPolicy,
        context: RepeatContext,
    ) -> Result<Self, RepeatError> {
        policy.validate()?;
        Ok(Self {
            runner,
            policy,
            context,
        })
    }

    pub fn run(&self, programs: Vec<LeafProgram>) -> Result<RepeatReport, RepeatError> {
        let ids = programs
            .iter()
            .map(|program| program.id().to_owned())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Err(RepeatError::InvalidContext);
        }
        let mut attempts = Vec::new();
        for iteration in 1..=self.policy.count {
            let report = self
                .runner
                .run(programs.clone())
                .map_err(RepeatError::Runtime)?;
            for leaf in report.leaves() {
                attempts.push(RepeatAttempt {
                    id: leaf.id().into(),
                    iteration,
                    round: 0,
                    unit: None,
                    status: leaf.status(),
                    worker: leaf.worker(),
                    report: leaf.report().clone(),
                });
            }
            if report.active_resources() != 0 {
                return Err(RepeatError::Runtime(RuntimeError::WorkerJoin));
            }
        }
        let mut outcomes = Vec::new();
        for id in ids {
            let leaf_attempts = attempts
                .iter()
                .filter(|attempt| attempt.id == id)
                .cloned()
                .collect::<Vec<_>>();
            let all_passed = leaf_attempts
                .iter()
                .all(|attempt| attempt.status == RuntimeStatus::Passed);
            let decisive = leaf_attempts
                .iter()
                .find(|attempt| attempt.status != RuntimeStatus::Passed)
                .map_or(RuntimeStatus::Passed, |attempt| attempt.status);
            outcomes.push(RepeatOutcome {
                id,
                attempts: leaf_attempts,
                all_passed,
                decisive,
            });
        }
        let exit_pass = outcomes.iter().all(|outcome| outcome.all_passed);
        Ok(RepeatReport {
            iterations: self.policy.count,
            attempts,
            outcomes,
            exit_pass,
            context: self.context.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepeatError {
    CountOutOfRange { value: u32 },
    Incompatible(&'static str),
    InvalidContext,
    Serialization(String),
    Runtime(RuntimeError),
}

impl fmt::Display for RepeatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CountOutOfRange { value } => {
                write!(
                    formatter,
                    "repeat count {value} must be 1..={MAX_REPEAT_COUNT}"
                )
            }
            Self::Incompatible(message) => formatter.write_str(message),
            Self::InvalidContext => formatter.write_str("repeat context is empty or invalid"),
            Self::Serialization(message) => {
                write!(formatter, "repeat context serialization failed: {message}")
            }
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl Error for RepeatError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_control::EnvelopeLimits;
    use crate::test_runtime::{RunError, RuntimeConfig};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    fn context() -> RepeatContext {
        RepeatContext::new(
            ["suite::test".into()],
            ["suite::test".into()],
            "0/2",
            "x86_64",
            "inputs-hash",
            17,
            "id-byte-order-v1",
            ["clock".into(), "network".into()],
            BTreeMap::from([("timeout_ns".into(), 100)]),
            "artifacts",
            "snapshots",
        )
        .unwrap()
    }

    fn runner(jobs: usize) -> RuntimeRunner {
        RuntimeRunner::new(
            RuntimeConfig::new(jobs, EnvelopeLimits::new(1_000, 1_000, 1_000)).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn policy_requires_positive_finite_count_and_rejects_incompatible_modes() {
        assert!(matches!(
            RepeatPolicy::new(0),
            Err(RepeatError::CountOutOfRange { .. })
        ));
        assert!(matches!(
            RepeatPolicy::new(MAX_REPEAT_COUNT + 1),
            Err(RepeatError::CountOutOfRange { .. })
        ));
        for policy in [
            RepeatPolicy::new(2).unwrap().with_retry_count(1),
            RepeatPolicy::new(2).unwrap().with_allow_flaky(true),
            RepeatPolicy::new(2).unwrap().with_list_mode(true),
            RepeatPolicy::new(2).unwrap().with_update_snapshots(true),
        ] {
            assert!(matches!(
                policy.validate(),
                Err(RepeatError::Incompatible(_))
            ));
        }
        let policy = RepeatPolicy::new(2).unwrap();
        assert_eq!(policy.count(), 2);
        assert_eq!(policy.retry_count(), 0);
        assert!(!policy.allow_flaky());
        assert!(!policy.list_mode());
        assert!(!policy.update_snapshots());
    }

    #[test]
    fn context_is_canonical_and_exposes_all_preserved_identity() {
        let first = context();
        let second = RepeatContext::new(
            ["suite::test".into()],
            ["suite::test".into()],
            "0/2",
            "x86_64",
            "inputs-hash",
            17,
            "id-byte-order-v1",
            ["network".into(), "clock".into()],
            BTreeMap::from([("timeout_ns".into(), 100)]),
            "artifacts",
            "snapshots",
        )
        .unwrap();
        assert_eq!(
            first.canonical_bytes().unwrap(),
            second.canonical_bytes().unwrap()
        );
        assert_eq!(first.selection(), &["suite::test"]);
        assert_eq!(first.execution_plan(), &["suite::test"]);
        assert_eq!(first.shard(), "0/2");
        assert_eq!(first.target(), "x86_64");
        assert_eq!(first.inputs_hash(), "inputs-hash");
        assert_eq!(first.seed(), 17);
        assert_eq!(first.order(), "id-byte-order-v1");
        assert_eq!(first.capabilities(), &["clock", "network"]);
        assert_eq!(first.limits().get("timeout_ns"), Some(&100));
        assert_eq!(first.artifact_store(), "artifacts");
        assert_eq!(first.snapshot_store(), "snapshots");
        assert!(
            RepeatContext::new(
                [],
                ["test".into()],
                "shard",
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
            RepeatContext::new(
                ["test".into()],
                ["test".into()],
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
    }

    #[test]
    fn repeat_runs_iterations_sequentially_with_new_workers_and_fresh_virtual_time() {
        let active = Arc::new(AtomicBool::new(false));
        let max_active = Arc::new(AtomicU32::new(0));
        let calls = Arc::new(AtomicU32::new(0));
        let active_body = active.clone();
        let max_body = max_active.clone();
        let call_body = calls.clone();
        let program = LeafProgram::new("suite::test", move |context| {
            assert!(!active_body.swap(true, Ordering::SeqCst));
            max_body.fetch_max(1, Ordering::SeqCst);
            let call = call_body.fetch_add(1, Ordering::SeqCst);
            context.with_virtual_time(|time| {
                time.advance(10)?;
                assert_eq!(time.now()?, 10);
                Ok(())
            })?;
            active_body.store(false, Ordering::SeqCst);
            if call == 1 {
                Err(RunError::Error {
                    code: "E".into(),
                    message: "iteration".into(),
                })
            } else {
                Ok(())
            }
        });
        let campaign =
            RepeatCampaign::new(runner(2), RepeatPolicy::new(3).unwrap(), context()).unwrap();
        let report = campaign.run(vec![program]).unwrap();
        assert_eq!(report.iterations(), 3);
        assert_eq!(report.attempts().len(), 3);
        assert!(
            report
                .attempts()
                .iter()
                .all(|attempt| attempt.round() == 0 && attempt.unit().is_none())
        );
        assert_eq!(
            report
                .attempts()
                .iter()
                .map(RepeatAttempt::iteration)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(
            report.attempts()[0].report().virtual_time()[0].elapsed_ns(),
            10
        );
        assert_ne!(report.attempts()[0].worker(), report.attempts()[1].worker());
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert!(!report.exit_pass());
        assert!(!report.outcomes()[0].all_passed());
        assert_eq!(report.outcomes()[0].decisive(), RuntimeStatus::FailedError);
        assert_eq!(report.context().seed(), 17);
    }

    #[test]
    fn single_iteration_is_ordinary_policy_and_cleanup_resources_are_revoked() {
        let campaign =
            RepeatCampaign::new(runner(1), RepeatPolicy::new(1).unwrap(), context()).unwrap();
        let report = campaign
            .run(vec![LeafProgram::new("suite::test", |context| {
                let handle = context.allocate_resource()?;
                context.defer(move |_| {
                    drop(handle);
                    Ok(())
                })?;
                Ok(())
            })])
            .unwrap();
        assert!(report.exit_pass());
        assert_eq!(report.outcomes()[0].attempts().len(), 1);
        assert!(report.outcomes()[0].all_passed());
        assert_eq!(report.outcomes()[0].decisive(), RuntimeStatus::Passed);
        assert!(format!("{:?}", campaign).contains("RepeatCampaign"));
        assert_eq!(TEST_REPEAT_FORMAT, "tondo-test-repeat-0.1/1");
    }

    #[test]
    fn skip_and_empty_programs_are_reported_without_implicit_repeat_flaky_mode() {
        let campaign =
            RepeatCampaign::new(runner(1), RepeatPolicy::new(2).unwrap(), context()).unwrap();
        assert_eq!(campaign.run(Vec::new()), Err(RepeatError::InvalidContext));
        let report = campaign
            .run(vec![LeafProgram::new("suite::test", |context| {
                context.skip("n/a")
            })])
            .unwrap();
        assert_eq!(report.attempts().len(), 2);
        assert!(!report.exit_pass());
        assert_eq!(report.outcomes()[0].decisive(), RuntimeStatus::Skipped);
    }
}
