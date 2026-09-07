//! Public retry units over one immutable compiled test target.

use super::*;
use tondo_compiler::test_retry::{RetryNode, RetryPlanner};

fn status(value: &str) -> Result<RuntimeStatus, TestCommandError> {
    Ok(match value {
        "passed" => RuntimeStatus::Passed,
        "skipped" => RuntimeStatus::Skipped,
        "failed-error" => RuntimeStatus::FailedError,
        "failed-panic" => RuntimeStatus::FailedPanic,
        "resource-limit" => RuntimeStatus::ResourceLimit,
        "timeout" => RuntimeStatus::Timeout,
        "infrastructure" => RuntimeStatus::Infrastructure,
        "blocked-setup" => RuntimeStatus::BlockedSetup,
        "blocked-skip" => RuntimeStatus::BlockedSkip,
        _ => {
            return Err(TestCommandError::Internal(format!(
                "unknown worker status `{value}`"
            )));
        }
    })
}

fn leaf_attempt(
    id: String,
    response: WorkerResponse,
    round: u32,
    unit: u32,
    invocation: u64,
) -> Result<CliAttempt, TestCommandError> {
    Ok(CliAttempt {
        id,
        iteration: 1,
        round,
        unit: Some(unit),
        invocation,
        status: status(&response.status)?,
        report: EnvelopeReport::decode_process(&response.report)
            .map_err(TestCommandError::Internal)?,
        error: response
            .error
            .map(|error| error.into_run_error().unwrap_err()),
        snapshot_updates: response
            .updates
            .into_iter()
            .map(|update| (update.name, update.value))
            .collect(),
        diagnostic_artifacts: response
            .diagnostics
            .iter()
            .flat_map(|report| report.artifacts.clone())
            .collect(),
        diagnostics: response.diagnostics,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn retry_units(
    plan: &test_cli::TestCliPlan,
    entries: &[tondo_compiler::test_backend::TestEntry],
    groups: &BTreeMap<(u32, String), Arc<SharedWorkerGroup>>,
    attempts: &mut Vec<CliAttempt>,
    suites: &mut Vec<CliSuiteAttempt>,
    request: &CompilationRequest,
    identity: &TestInvocationIdentity,
    snapshots: &SnapshotInputs,
) -> Result<Vec<tondo_compiler::test_report::ReportRetryRound>, TestCommandError> {
    if plan.retry == 0 || entries.is_empty() {
        return Ok(Vec::new());
    }
    let context = RetryContext::new(
        shard_identity(plan),
        request.target().name(),
        identity.inputs.public_sha256(),
        order_seed(plan),
        tondo_compiler::test_report::CANONICAL_ORDER_ALGORITHM,
        request
            .capabilities()
            .iter()
            .map(|capability| capability.as_str().to_owned()),
        campaign_limits(plan),
        &identity.artifact_store_sha256,
        &snapshots.before_sha256,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let mut nodes = entries
        .iter()
        .map(|entry| (entry.id().to_owned(), RetryNode::test(entry.id())))
        .collect::<BTreeMap<_, _>>();
    for entry in entries {
        let ancestors = suite_ids(entry);
        for (index, id) in ancestors.iter().enumerate() {
            nodes.entry(id.clone()).or_insert_with(|| {
                RetryNode::suite(
                    id.clone(),
                    index.checked_sub(1).map(|parent| ancestors[parent].clone()),
                    entries
                        .iter()
                        .filter(|entry| {
                            entry
                                .id()
                                .strip_prefix(id)
                                .is_some_and(|suffix| suffix.starts_with("::"))
                        })
                        .map(|entry| entry.id().to_owned()),
                )
            });
        }
    }
    let planner = RetryPlanner::new(
        entries.iter().map(|entry| entry.id().to_owned()),
        nodes.into_values(),
        context,
    )
    .map_err(|error| TestCommandError::Internal(error.to_string()))?;
    let mut invocation = attempts
        .iter()
        .map(|attempt| attempt.invocation)
        .max()
        .unwrap_or(1);
    let mut rounds = Vec::new();
    for round in 1..=plan.retry {
        if test_interrupt::requests() > 0 {
            break;
        }
        // The latest participation controls eligibility. An older aggregate
        // failure must not turn a later skip or resource limit into a retry.
        let statuses = attempts
            .iter()
            .map(|attempt| (attempt.id.clone(), attempt.status))
            .chain(suites.iter().map(|suite| (suite.id.clone(), suite.status)))
            .collect();
        let retry = planner
            .plan(&statuses, round)
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
        if retry.units().is_empty() {
            break;
        }
        for (index, unit) in retry.units().iter().enumerate() {
            let group = groups
                .values()
                .find(|group| {
                    unit.execution_plan
                        .iter()
                        .all(|id| group.entries.contains(id))
                })
                .ok_or_else(|| {
                    TestCommandError::Internal("retry unit has no compiled participation".into())
                })?;
            invocation += 1;
            let unit_index = index as u32 + 1;
            let result = spawn_test_worker(
                group.input.clone(),
                &unit.execution_plan,
                group.timeout_ms,
                group.phase_limits,
                false,
                &DiagnosticWorkerContext {
                    profiles: &group.diagnostics,
                    run_id: &group.run_id,
                    source_revision: &group.source_revision,
                    shard: &group.shard,
                    invocation,
                },
            )
            .map_err(|error| TestCommandError::Internal(error.to_string()))?;
            for id in &unit.execution_plan {
                let response = result.leaves.get(id).cloned().ok_or_else(|| {
                    TestCommandError::Internal("retry worker omitted selected leaf".into())
                })?;
                attempts.push(leaf_attempt(
                    id.clone(),
                    response,
                    round,
                    unit_index,
                    invocation,
                )?);
            }
            for suite in result.suites {
                let phase = match suite.phase.as_deref() {
                    None => None,
                    Some("setup") => Some(AttemptPhase::Setup),
                    Some("teardown") => Some(AttemptPhase::Teardown),
                    _ => {
                        return Err(TestCommandError::Internal(
                            "retry suite has an invalid phase".into(),
                        ));
                    }
                };
                let leaf = leaf_attempt(
                    suite.id,
                    WorkerResponse {
                        format: WORKER_RESPONSE_FORMAT.into(),
                        status: suite.status,
                        report: suite.report,
                        updates: suite.updates,
                        error: suite.error,
                        diagnostics: suite.diagnostics,
                    },
                    round,
                    unit_index,
                    invocation,
                )?;
                suites.push(CliSuiteAttempt {
                    id: leaf.id,
                    iteration: 1,
                    round,
                    unit: Some(unit_index),
                    invocation,
                    status: leaf.status,
                    phase,
                    report: leaf.report,
                    error: leaf.error,
                    snapshot_updates: leaf.snapshot_updates,
                    diagnostics: leaf.diagnostics,
                    diagnostic_artifacts: leaf.diagnostic_artifacts,
                });
            }
        }
        rounds.push(tondo_compiler::test_report::ReportRetryRound {
            round,
            units: retry.units().to_vec(),
        });
    }
    Ok(rounds)
}
