//! Deterministic generated-case campaigns for the existing test runtime.
//!
//! Generated cases are a tooling view over ordinary test workers.  They never
//! register test nodes or alter discovery: a campaign materializes a bounded
//! sequence, runs each case through [`RuntimeRunner`], and keeps the resulting
//! evidence in a separate value.  Shrinking reuses the public `Shrink` trait
//! from `std.testing` and executes every candidate in a fresh worker.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use tondo_stdlib::testing::{GenerationError, GenerationId, Generator, Shrink, shrink};

use crate::test_control::EnvelopeReport;
use crate::test_runtime::{
    LeafProgram, RunError, RuntimeError, RuntimeRunner, RuntimeStatus, WorkerContext,
};

pub const TEST_GENERATION_FORMAT: &str = "tondo-test-generation-0.1/1";
pub const MAX_GENERATED_CASES: u64 = 100_000;
pub const MAX_SHRINK_CANDIDATES: usize = tondo_stdlib::testing::MAX_SHRINK_CANDIDATES;
pub const MAX_SHRINK_DEPTH: usize = 64;

type GeneratedBody<T> = dyn Fn(&WorkerContext, &T) -> Result<(), RunError> + Send + Sync;

/// Limits applied before a generated campaign allocates or executes work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationLimits {
    pub cases: u64,
    pub shrink_candidates: usize,
    pub shrink_depth: usize,
}

impl Default for GenerationLimits {
    fn default() -> Self {
        Self {
            cases: 1_000,
            shrink_candidates: 256,
            shrink_depth: 32,
        }
    }
}

impl GenerationLimits {
    pub fn validate(self) -> Result<Self, GenerationError> {
        if self.cases == 0 || self.cases > MAX_GENERATED_CASES {
            return Err(GenerationError::LimitExceeded);
        }
        if self.shrink_candidates > MAX_SHRINK_CANDIDATES || self.shrink_depth > MAX_SHRINK_DEPTH {
            return Err(GenerationError::LimitExceeded);
        }
        Ok(self)
    }
}

/// Immutable campaign identity and resource profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPlan {
    name: String,
    seed: u64,
    limits: GenerationLimits,
}

impl GenerationPlan {
    pub fn new(name: impl Into<String>, seed: u64, cases: u64) -> Result<Self, GenerationError> {
        let name = name.into();
        if name.trim().is_empty() || name.contains(['\n', '\r']) {
            return Err(GenerationError::InvalidBounds);
        }
        let plan = Self {
            name,
            seed,
            limits: GenerationLimits {
                cases,
                ..GenerationLimits::default()
            },
        };
        plan.limits.validate()?;
        Ok(plan)
    }

    pub fn with_limits(mut self, limits: GenerationLimits) -> Result<Self, GenerationError> {
        limits.validate()?;
        self.limits = limits;
        Ok(self)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn seed(&self) -> u64 {
        self.seed
    }

    pub const fn limits(&self) -> GenerationLimits {
        self.limits
    }
}

/// One generated input.  `case_index` is stable and zero-based; it is not a
/// test-tree identity and is never registered as a dynamic test node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCase<T> {
    id: GenerationId,
    input: T,
}

impl<T> GeneratedCase<T> {
    pub const fn id(&self) -> GenerationId {
        self.id
    }

    pub const fn input(&self) -> &T {
        &self.input
    }

    pub fn into_input(self) -> T {
        self.input
    }
}

/// Materialized, ordered generated inputs for one campaign.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCases<T> {
    plan: GenerationPlan,
    cases: Vec<GeneratedCase<T>>,
}

impl<T> GeneratedCases<T> {
    pub fn collect<F>(plan: &GenerationPlan, generate: F) -> Result<Self, GenerationError>
    where
        F: Fn(&mut Generator) -> Result<T, GenerationError>,
    {
        plan.limits.validate()?;
        let capacity =
            usize::try_from(plan.limits.cases).map_err(|_| GenerationError::LimitExceeded)?;
        let mut cases = Vec::new();
        cases
            .try_reserve_exact(capacity)
            .map_err(|_| GenerationError::LimitExceeded)?;
        for case_index in 0..plan.limits.cases {
            let id = GenerationId {
                seed: plan.seed,
                case_index,
            };
            let mut generator = Generator::for_case(id.seed, id.case_index);
            let input = generate(&mut generator)?;
            cases.push(GeneratedCase { id, input });
        }
        Ok(Self {
            plan: plan.clone(),
            cases,
        })
    }

    pub fn replay<F>(
        plan: &GenerationPlan,
        case_index: u64,
        generate: F,
    ) -> Result<GeneratedCase<T>, GenerationError>
    where
        F: Fn(&mut Generator) -> Result<T, GenerationError>,
    {
        plan.limits.validate()?;
        if case_index >= plan.limits.cases {
            return Err(GenerationError::InvalidBounds);
        }
        let id = GenerationId {
            seed: plan.seed,
            case_index,
        };
        let mut generator = Generator::for_case(id.seed, id.case_index);
        Ok(GeneratedCase {
            id,
            input: generate(&mut generator)?,
        })
    }

    pub fn plan(&self) -> &GenerationPlan {
        &self.plan
    }

    pub fn cases(&self) -> &[GeneratedCase<T>] {
        &self.cases
    }

    pub fn case(&self, case_index: u64) -> Option<&GeneratedCase<T>> {
        self.cases
            .get(usize::try_from(case_index).ok()?)
            .filter(|case| case.id.case_index == case_index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCaseResult {
    id: GenerationId,
    status: RuntimeStatus,
    report: EnvelopeReport,
    error: Option<RunError>,
}

impl GeneratedCaseResult {
    pub const fn id(&self) -> GenerationId {
        self.id
    }

    pub const fn status(&self) -> RuntimeStatus {
        self.status
    }

    pub fn report(&self) -> &EnvelopeReport {
        &self.report
    }

    pub fn error(&self) -> Option<&RunError> {
        self.error.as_ref()
    }

    pub const fn failed(&self) -> bool {
        !matches!(self.status, RuntimeStatus::Passed)
    }
}

/// The candidate selected by deterministic shrinking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShrinkResult<T> {
    original: GeneratedCase<T>,
    minimized: T,
    evaluations: usize,
    depth: usize,
}

impl<T> ShrinkResult<T> {
    pub fn original(&self) -> &GeneratedCase<T> {
        &self.original
    }

    pub const fn minimized(&self) -> &T {
        &self.minimized
    }

    pub const fn evaluations(&self) -> usize {
        self.evaluations
    }

    pub const fn depth(&self) -> usize {
        self.depth
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRun<T> {
    plan: GenerationPlan,
    cases: Vec<GeneratedCaseResult>,
    first_failure: Option<GeneratedCase<T>>,
    shrink: Option<ShrinkResult<T>>,
}

impl<T> GeneratedRun<T> {
    pub fn plan(&self) -> &GenerationPlan {
        &self.plan
    }

    pub fn cases(&self) -> &[GeneratedCaseResult] {
        &self.cases
    }

    pub fn failed_cases(&self) -> impl Iterator<Item = &GeneratedCaseResult> {
        self.cases.iter().filter(|case| case.failed())
    }

    pub fn first_failure(&self) -> Option<&GeneratedCase<T>> {
        self.first_failure.as_ref()
    }

    pub fn shrink(&self) -> Option<&ShrinkResult<T>> {
        self.shrink.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeneratedRunError {
    Generation(GenerationError),
    Runtime(RuntimeError),
    MissingResult(GenerationId),
}

impl fmt::Display for GeneratedRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generation(error) => write!(formatter, "generation failed: {error:?}"),
            Self::Runtime(error) => write!(formatter, "generated campaign runtime failed: {error}"),
            Self::MissingResult(id) => write!(
                formatter,
                "generated campaign omitted case {}:{}",
                id.seed, id.case_index
            ),
        }
    }
}

impl Error for GeneratedRunError {}

impl From<GenerationError> for GeneratedRunError {
    fn from(error: GenerationError) -> Self {
        Self::Generation(error)
    }
}

impl From<RuntimeError> for GeneratedRunError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl<T> GeneratedCases<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// Execute all generated cases through the existing isolated runtime.
    pub fn run<F>(
        &self,
        runner: &RuntimeRunner,
        body: F,
    ) -> Result<GeneratedRun<T>, GeneratedRunError>
    where
        F: Fn(&WorkerContext, &T) -> Result<(), RunError> + Send + Sync + 'static,
    {
        let cases = run_cases(runner, &self.plan, &self.cases, Arc::new(body))?;
        let first_failure = self
            .cases
            .iter()
            .zip(cases.iter())
            .find(|(_, result)| result.failed())
            .map(|(case, _)| (*case).clone());
        Ok(GeneratedRun {
            plan: self.plan.clone(),
            cases,
            first_failure,
            shrink: None,
        })
    }
}

impl<T> GeneratedCases<T>
where
    T: Clone + PartialEq + Shrink + Send + Sync + 'static,
{
    /// Execute the campaign and shrink the first failing input in deterministic
    /// candidate order.  Every candidate is run in a fresh worker invocation.
    pub fn run_with_shrink<F>(
        &self,
        runner: &RuntimeRunner,
        body: F,
    ) -> Result<GeneratedRun<T>, GeneratedRunError>
    where
        F: Fn(&WorkerContext, &T) -> Result<(), RunError> + Send + Sync + 'static,
    {
        let body = Arc::new(body);
        let cases = run_cases(runner, &self.plan, &self.cases, body.clone())?;
        let Some(original) = self
            .cases
            .iter()
            .zip(cases.iter())
            .find(|(_, result)| result.failed())
            .map(|(case, _)| case.clone())
        else {
            return Ok(GeneratedRun {
                plan: self.plan.clone(),
                cases,
                first_failure: None,
                shrink: None,
            });
        };

        let mut current = original.input.clone();
        let mut evaluations = 0_usize;
        let mut depth = 0_usize;
        while depth < self.plan.limits.shrink_depth && self.plan.limits.shrink_candidates > 0 {
            let candidates = shrink(&current, self.plan.limits.shrink_candidates)
                .map_err(GeneratedRunError::Generation)?;
            let mut improved = None;
            for candidate in candidates {
                evaluations = evaluations.saturating_add(1);
                let candidate_case = GeneratedCase {
                    id: original.id,
                    input: candidate.clone(),
                };
                let result = run_cases(
                    runner,
                    &self.plan,
                    std::slice::from_ref(&candidate_case),
                    body.clone(),
                )?
                .into_iter()
                .next()
                .ok_or(GeneratedRunError::MissingResult(original.id))?;
                if result.failed() {
                    improved = Some(candidate);
                    break;
                }
            }
            let Some(candidate) = improved else {
                break;
            };
            current = candidate;
            depth = depth.saturating_add(1);
        }

        Ok(GeneratedRun {
            plan: self.plan.clone(),
            cases,
            first_failure: Some(original.clone()),
            shrink: Some(ShrinkResult {
                original,
                minimized: current,
                evaluations,
                depth,
            }),
        })
    }
}

fn run_cases<T>(
    runner: &RuntimeRunner,
    plan: &GenerationPlan,
    cases: &[GeneratedCase<T>],
    body: Arc<GeneratedBody<T>>,
) -> Result<Vec<GeneratedCaseResult>, GeneratedRunError>
where
    T: Clone + Send + Sync + 'static,
{
    let programs = cases
        .iter()
        .map(|case| {
            let input = case.input.clone();
            let id = case_id(plan.name(), case.id.case_index);
            let body = body.clone();
            LeafProgram::new(id, move |context| body(context, &input))
        })
        .collect::<Vec<_>>();
    let report = runner.run(programs)?;
    let mut results = Vec::with_capacity(cases.len());
    for case in cases {
        let id = case_id(plan.name(), case.id.case_index);
        let leaf = report
            .leaves()
            .iter()
            .find(|leaf| leaf.id() == id)
            .ok_or(GeneratedRunError::MissingResult(case.id))?;
        results.push(GeneratedCaseResult {
            id: case.id,
            status: leaf.status(),
            report: leaf.report().clone(),
            error: leaf.error().cloned(),
        });
    }
    Ok(results)
}

fn case_id(name: &str, case_index: u64) -> String {
    format!("{name}::generated:{case_index:020}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_control::EnvelopeLimits;

    fn runner() -> RuntimeRunner {
        RuntimeRunner::new(
            crate::test_runtime::RuntimeConfig::new(2, EnvelopeLimits::new(1_000, 1_000, 1_000))
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn collection_is_replayable_and_keeps_case_order() {
        let plan = GenerationPlan::new("property", 7, 4).unwrap();
        let cases =
            GeneratedCases::collect(&plan, |generator| generator.next_int(-10, 10)).unwrap();
        let replay =
            GeneratedCases::replay(&plan, 2, |generator| generator.next_int(-10, 10)).unwrap();
        assert_eq!(cases.case(2), Some(&replay));
        assert_eq!(
            cases
                .cases()
                .iter()
                .map(|case| case.id.case_index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn runner_reports_generated_cases_in_case_order_even_with_parallel_workers() {
        let plan = GenerationPlan::new("property", 11, 12)
            .unwrap()
            .with_limits(GenerationLimits {
                cases: 12,
                shrink_candidates: 4,
                shrink_depth: 4,
            })
            .unwrap();
        let cases = GeneratedCases::collect(&plan, |generator| generator.next_u64()).unwrap();
        let run = cases
            .run(&runner(), |context, value| context.log(value.to_string()))
            .unwrap();
        assert_eq!(
            run.cases()
                .iter()
                .map(|case| case.id.case_index)
                .collect::<Vec<_>>(),
            (0..12).collect::<Vec<_>>()
        );
        assert!(run.failed_cases().next().is_none());
    }

    #[test]
    fn shrinking_reuses_the_existing_runtime_and_stops_at_first_failing_candidate() {
        let plan = GenerationPlan::new("property", 1, 1)
            .unwrap()
            .with_limits(GenerationLimits {
                cases: 1,
                shrink_candidates: 8,
                shrink_depth: 8,
            })
            .unwrap();
        let cases = GeneratedCases::collect(&plan, |_| Ok::<_, GenerationError>(10_i128)).unwrap();
        let run = cases
            .run_with_shrink(&runner(), |_, value| {
                if *value >= 2 {
                    Err(RunError::Error {
                        code: "P".into(),
                        message: "property failed".into(),
                    })
                } else {
                    Ok(())
                }
            })
            .unwrap();
        let shrink = run.shrink().unwrap();
        assert_eq!(*shrink.minimized(), 2);
        assert_eq!(shrink.depth(), 2);
        assert_eq!(shrink.evaluations(), 4);
        assert_eq!(run.first_failure().unwrap().id.case_index, 0);
    }

    #[test]
    fn limits_reject_unbounded_campaigns_before_materialization() {
        assert_eq!(
            GenerationPlan::new("property", 0, MAX_GENERATED_CASES + 1),
            Err(GenerationError::LimitExceeded)
        );
        let plan = GenerationPlan::new("property", 0, 1).unwrap();
        assert_eq!(
            plan.with_limits(GenerationLimits {
                cases: 1,
                shrink_candidates: MAX_SHRINK_CANDIDATES + 1,
                shrink_depth: 1,
            }),
            Err(GenerationError::LimitExceeded)
        );
    }

    #[test]
    fn public_views_and_error_boundaries_are_closed() {
        assert_eq!(
            GenerationLimits::default().validate(),
            Ok(GenerationLimits::default())
        );
        assert_eq!(
            GenerationLimits {
                cases: 0,
                ..GenerationLimits::default()
            }
            .validate(),
            Err(GenerationError::LimitExceeded)
        );
        assert_eq!(
            GenerationLimits {
                shrink_candidates: MAX_SHRINK_CANDIDATES + 1,
                ..GenerationLimits::default()
            }
            .validate(),
            Err(GenerationError::LimitExceeded)
        );
        assert_eq!(
            GenerationLimits {
                shrink_depth: MAX_SHRINK_DEPTH + 1,
                ..GenerationLimits::default()
            }
            .validate(),
            Err(GenerationError::LimitExceeded)
        );
        assert_eq!(
            GenerationPlan::new("", 1, 1),
            Err(GenerationError::InvalidBounds)
        );
        assert_eq!(
            GenerationPlan::new("bad\nname", 1, 1),
            Err(GenerationError::InvalidBounds)
        );

        let plan = GenerationPlan::new("views", 9, 1)
            .unwrap()
            .with_limits(GenerationLimits {
                cases: 1,
                shrink_candidates: 0,
                shrink_depth: 0,
            })
            .unwrap();
        assert_eq!(plan.name(), "views");
        assert_eq!(plan.seed(), 9);
        assert_eq!(plan.limits().shrink_candidates, 0);
        assert_eq!(
            GeneratedCases::<i128>::replay(&plan, 1, |_| Ok(0)),
            Err(GenerationError::InvalidBounds)
        );

        let cases = GeneratedCases::collect(&plan, |_| Ok::<_, GenerationError>(4_i128)).unwrap();
        assert_eq!(cases.plan(), &plan);
        assert_eq!(cases.cases().len(), 1);
        assert_eq!(cases.case(1), None);
        let generated = cases.case(0).unwrap().clone();
        assert_eq!(
            generated.id(),
            GenerationId {
                seed: 9,
                case_index: 0
            }
        );
        assert_eq!(generated.input(), &4);
        assert_eq!(generated.clone().into_input(), 4);

        let failed = cases
            .run(&runner(), |_, _| {
                Err(RunError::Error {
                    code: "E".into(),
                    message: "generated failure".into(),
                })
            })
            .unwrap();
        assert_eq!(failed.plan(), &plan);
        assert_eq!(failed.cases().len(), 1);
        assert_eq!(failed.failed_cases().count(), 1);
        let result = &failed.cases()[0];
        assert_eq!(result.id(), generated.id());
        assert_eq!(result.status(), RuntimeStatus::FailedError);
        assert!(result.report().logs().is_empty());
        assert_eq!(result.error().unwrap().code(), Some("E"));
        assert!(result.failed());
        assert_eq!(failed.first_failure(), Some(&generated));
        assert!(failed.shrink().is_none());

        let passed = cases.run_with_shrink(&runner(), |_, _| Ok(())).unwrap();
        assert!(passed.first_failure().is_none());
        assert!(passed.shrink().is_none());
        let no_shrink = passed.plan().limits();
        assert_eq!(no_shrink.shrink_depth, 0);

        let errors = [
            GeneratedRunError::from(GenerationError::Exhausted),
            GeneratedRunError::from(RuntimeError::EmptyLeafId),
            GeneratedRunError::MissingResult(GenerationId {
                seed: 9,
                case_index: 3,
            }),
        ];
        assert!(errors.iter().all(|error| !error.to_string().is_empty()));
        assert!(errors.iter().all(|error| error.source().is_none()));
    }
}
