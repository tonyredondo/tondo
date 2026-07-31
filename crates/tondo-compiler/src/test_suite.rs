//! Hierarchical suite lifecycle for the Tondo test runner.
//!
//! The suite coordinator owns setup/cleanup/teardown ordering, while the leaf
//! worker remains responsible for executing an individual test body. A
//! [`SuiteRunner`] creates a fresh context for every participation, so calling
//! it again for a retry never reuses setup state, guards, or snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::test_result::AttemptStatus;

pub const TEST_SUITE_FORMAT: &str = "tondo-test-suite-draft/1";

/// A lifecycle action can represent synchronous or already-lowered async work.
/// The coordinator never assumes that an action is infallible: every terminal
/// outcome is projected into the result model before descendants continue.
pub type SuiteAction = dyn Fn(&SuiteContext) -> Result<(), SuiteActionError> + Send + Sync;

/// Recoverable and terminal outcomes produced by setup, test, cleanup, or
/// teardown. A panic is represented explicitly because it must not stop a
/// sibling suite when the worker boundary is still healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteActionError {
    Error { code: String, message: String },
    Panic { code: String, message: String },
    Skip { reason: String },
    ResourceLimit { kind: String },
    Timeout,
    Infrastructure { message: String },
}

impl SuiteActionError {
    fn status(&self) -> AttemptStatus {
        match self {
            Self::Error { .. } => AttemptStatus::FailedError,
            Self::Panic { .. } => AttemptStatus::FailedPanic,
            Self::Skip { .. } => AttemptStatus::Skipped,
            Self::ResourceLimit { .. } => AttemptStatus::ResourceLimit,
            Self::Timeout => AttemptStatus::Timeout,
            Self::Infrastructure { .. } => AttemptStatus::Infrastructure,
        }
    }
}

impl fmt::Display for SuiteActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error { code, message } | Self::Panic { code, message } => {
                write!(formatter, "{code}: {message}")
            }
            Self::Skip { reason } => write!(formatter, "skipped: {reason}"),
            Self::ResourceLimit { kind } => write!(formatter, "resource limit: {kind}"),
            Self::Timeout => formatter.write_str("suite action timed out"),
            Self::Infrastructure { message } => write!(formatter, "infrastructure: {message}"),
        }
    }
}

impl Error for SuiteActionError {}

/// Per-participation environment shared by a suite setup and its descendants.
/// The map is an orchestration value, not a Tondo-visible identity; it models
/// immutable setup snapshots and cleanup guards at this coordinator boundary.
#[derive(Clone)]
pub struct SuiteContext {
    run_id: u64,
    node_id: String,
    values: Arc<Mutex<BTreeMap<String, String>>>,
}

impl fmt::Debug for SuiteContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SuiteContext")
            .field("run_id", &self.run_id)
            .field("node_id", &self.node_id)
            .finish_non_exhaustive()
    }
}

impl SuiteContext {
    fn root(run_id: u64, node_id: &str) -> Self {
        Self {
            run_id,
            node_id: node_id.into(),
            values: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn child(&self, node_id: &str) -> Self {
        Self {
            run_id: self.run_id,
            node_id: node_id.into(),
            values: self.values.clone(),
        }
    }

    /// Identifier of the fresh coordinator participation. A retry gets a
    /// different value even when it uses the same [`SuitePlan`].
    pub const fn run_id(&self) -> u64 {
        self.run_id
    }

    /// The currently executing suite/test node. This is coordinator metadata
    /// and is never exposed through the Tondo testing API.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Store a setup value for descendants. Real lowered setup snapshots are
    /// immutable; this API models their construction before capture.
    pub fn set(&self, key: impl Into<String>, value: impl Into<String>) -> Result<(), SuiteError> {
        self.values
            .lock()
            .map_err(|_| SuiteError::ContextPoisoned)?
            .insert(key.into(), value.into());
        Ok(())
    }

    /// Read a value captured by an ancestor setup.
    pub fn get(&self, key: &str) -> Result<Option<String>, SuiteError> {
        Ok(self
            .values
            .lock()
            .map_err(|_| SuiteError::ContextPoisoned)?
            .get(key)
            .cloned())
    }

    fn clear(&self) -> Result<(), SuiteError> {
        self.values
            .lock()
            .map_err(|_| SuiteError::ContextPoisoned)?
            .clear();
        Ok(())
    }
}

/// A suite or leaf definition supplied by the static test plan.
pub struct SuiteNode {
    id: String,
    parent: Option<String>,
    selected: bool,
    setup: Option<Arc<SuiteAction>>,
    body: Option<Arc<SuiteAction>>,
    cleanup: Vec<Arc<SuiteAction>>,
    teardown: Option<Arc<SuiteAction>>,
}

impl fmt::Debug for SuiteNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SuiteNode")
            .field("id", &self.id)
            .field("parent", &self.parent)
            .field("selected", &self.selected)
            .field("is_suite", &self.is_suite())
            .field("cleanup_count", &self.cleanup.len())
            .finish_non_exhaustive()
    }
}

impl SuiteNode {
    /// Create a suite. `parent` is the visible ID of its enclosing suite.
    pub fn suite(id: impl Into<String>, parent: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            selected: false,
            setup: None,
            body: None,
            cleanup: Vec::new(),
            teardown: None,
        }
    }

    /// Create a selected or unselected leaf. Selection is applied after
    /// filter/glob/exact and before sharding; suites participate implicitly.
    pub fn test(
        id: impl Into<String>,
        parent: Option<impl Into<String>>,
        body: impl Fn(&SuiteContext) -> Result<(), SuiteActionError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            selected: true,
            setup: None,
            body: Some(Arc::new(body)),
            cleanup: Vec::new(),
            teardown: None,
        }
    }

    pub fn with_selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn with_setup(
        mut self,
        action: impl Fn(&SuiteContext) -> Result<(), SuiteActionError> + Send + Sync + 'static,
    ) -> Self {
        self.setup = Some(Arc::new(action));
        self
    }

    pub fn with_cleanup(
        mut self,
        action: impl Fn(&SuiteContext) -> Result<(), SuiteActionError> + Send + Sync + 'static,
    ) -> Self {
        self.cleanup.push(Arc::new(action));
        self
    }

    pub fn with_teardown(
        mut self,
        action: impl Fn(&SuiteContext) -> Result<(), SuiteActionError> + Send + Sync + 'static,
    ) -> Self {
        self.teardown = Some(Arc::new(action));
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }

    pub const fn selected(&self) -> bool {
        self.selected
    }

    pub const fn is_suite(&self) -> bool {
        self.body.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuiteError {
    EmptyId,
    DuplicateId(String),
    UnknownParent { child: String, parent: String },
    TestParentIsLeaf { child: String, parent: String },
    ContextPoisoned,
}

impl fmt::Display for SuiteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyId => formatter.write_str("suite node identity cannot be empty"),
            Self::DuplicateId(id) => write!(formatter, "suite node `{id}` is duplicated"),
            Self::UnknownParent { child, parent } => {
                write!(
                    formatter,
                    "node `{child}` refers to unknown parent `{parent}`"
                )
            }
            Self::TestParentIsLeaf { child, parent } => {
                write!(formatter, "leaf `{child}` cannot contain child `{parent}`")
            }
            Self::ContextPoisoned => formatter.write_str("suite context is poisoned"),
        }
    }
}

impl Error for SuiteError {}

/// A validated immutable tree. Child order is canonical UTF-8 byte order.
pub struct SuitePlan {
    nodes: BTreeMap<String, SuiteNode>,
    children: BTreeMap<String, Vec<String>>,
    roots: Vec<String>,
}

impl fmt::Debug for SuitePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SuitePlan")
            .field("nodes", &self.nodes.keys().collect::<Vec<_>>())
            .field("roots", &self.roots)
            .finish()
    }
}

impl SuitePlan {
    pub fn new(nodes: impl IntoIterator<Item = SuiteNode>) -> Result<Self, SuiteError> {
        let mut map = BTreeMap::new();
        for node in nodes {
            if node.id.trim().is_empty() {
                return Err(SuiteError::EmptyId);
            }
            let id = node.id.clone();
            if map.insert(id.clone(), node).is_some() {
                return Err(SuiteError::DuplicateId(id));
            }
        }
        let mut children = BTreeMap::<String, Vec<String>>::new();
        let mut roots = Vec::new();
        for (id, node) in &map {
            let Some(parent) = node.parent.as_deref() else {
                roots.push(id.clone());
                continue;
            };
            let Some(parent_node) = map.get(parent) else {
                return Err(SuiteError::UnknownParent {
                    child: id.clone(),
                    parent: parent.into(),
                });
            };
            if !parent_node.is_suite() {
                return Err(SuiteError::TestParentIsLeaf {
                    child: id.clone(),
                    parent: parent.into(),
                });
            }
            children.entry(parent.into()).or_default().push(id.clone());
        }
        roots.sort();
        for values in children.values_mut() {
            values.sort();
        }
        let plan = Self {
            nodes: map,
            children,
            roots,
        };
        for id in plan.nodes.keys() {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(id.as_str());
            while let Some(current) = cursor {
                if !seen.insert(current) {
                    return Err(SuiteError::UnknownParent {
                        child: id.clone(),
                        parent: current.into(),
                    });
                }
                cursor = plan.nodes[current].parent.as_deref();
            }
        }
        Ok(plan)
    }

    pub fn nodes(&self) -> impl Iterator<Item = &SuiteNode> {
        self.nodes.values()
    }

    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    pub fn node(&self, id: &str) -> Option<&SuiteNode> {
        self.nodes.get(id)
    }

    fn children(&self, id: &str) -> &[String] {
        self.children.get(id).map_or(&[], Vec::as_slice)
    }

    fn participates(&self, id: &str) -> bool {
        let node = &self.nodes[id];
        if !node.is_suite() {
            return node.selected;
        }
        self.children(id)
            .iter()
            .any(|child| self.participates(child))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuitePhase {
    Setup,
    Body,
    Cleanup,
    Teardown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteResult {
    id: String,
    status: AttemptStatus,
    phase: Option<SuitePhase>,
    blocked_by: Option<String>,
    cleanup_executed: bool,
}

impl SuiteResult {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub const fn status(&self) -> AttemptStatus {
        self.status
    }

    pub const fn phase(&self) -> Option<SuitePhase> {
        self.phase
    }

    pub fn blocked_by(&self) -> Option<&str> {
        self.blocked_by.as_deref()
    }

    pub const fn cleanup_executed(&self) -> bool {
        self.cleanup_executed
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuiteReport {
    run_id: u64,
    results: Vec<SuiteResult>,
}

impl SuiteReport {
    pub const fn run_id(&self) -> u64 {
        self.run_id
    }

    pub fn results(&self) -> &[SuiteResult] {
        &self.results
    }

    pub fn result(&self, id: &str) -> Option<&SuiteResult> {
        self.results.iter().find(|result| result.id == id)
    }
}

/// Coordinator for one or more independent suite participations.
#[derive(Debug, Default)]
pub struct SuiteRunner {
    next_run: AtomicU64,
}

impl SuiteRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn run(&self, plan: &SuitePlan) -> SuiteReport {
        let run_id = self.next_run.fetch_add(1, Ordering::Relaxed) + 1;
        let mut state = RunState {
            results: BTreeMap::new(),
        };
        for root in plan.roots() {
            if plan.participates(root) {
                let context = SuiteContext::root(run_id, root);
                let _ = execute_suite(plan, root, context.clone(), &mut state);
                let _ = context.clear();
            }
        }
        SuiteReport {
            run_id,
            results: state.results.into_values().collect(),
        }
    }
}

struct RunState {
    results: BTreeMap<String, SuiteResult>,
}

fn execute_suite(
    plan: &SuitePlan,
    id: &str,
    context: SuiteContext,
    state: &mut RunState,
) -> AttemptStatus {
    let node = &plan.nodes[id];
    let setup = invoke(node.setup.as_deref(), &context);
    let mut status = AttemptStatus::Passed;
    let mut phase = None;
    if let Some(outcome) = setup {
        status = outcome.status();
        phase = Some(SuitePhase::Setup);
        for child in plan.children(id) {
            record_blocked(plan, child, &context.child(child), id, status, state);
        }
    } else {
        for child in plan.children(id) {
            if plan.participates(child) {
                let child_context = context.child(child);
                if plan.nodes[child].is_suite() {
                    execute_suite(plan, child, child_context, state);
                } else {
                    execute_leaf(plan, child, child_context, state);
                }
            }
        }
    }

    let cleanup_executed = !node.cleanup.is_empty();
    if cleanup_executed {
        for cleanup in node.cleanup.iter().rev() {
            if let Some(outcome) = invoke(Some(cleanup.as_ref()), &context) {
                status = outcome.status();
                phase = Some(SuitePhase::Cleanup);
                break;
            }
        }
    }
    if let Some(teardown) = node.teardown.as_deref()
        && let Some(outcome) = invoke(Some(teardown), &context)
    {
        status = outcome.status();
        phase = Some(SuitePhase::Teardown);
    }
    state.results.insert(
        id.to_owned(),
        SuiteResult {
            id: id.to_owned(),
            status,
            phase,
            blocked_by: None,
            cleanup_executed,
        },
    );
    status
}

fn execute_leaf(plan: &SuitePlan, id: &str, context: SuiteContext, state: &mut RunState) {
    let node = &plan.nodes[id];
    let (status, phase) = match invoke(node.body.as_deref(), &context) {
        None => (AttemptStatus::Passed, None),
        Some(error) => (error.status(), Some(SuitePhase::Body)),
    };
    state.results.insert(
        id.to_owned(),
        SuiteResult {
            id: id.to_owned(),
            status,
            phase,
            blocked_by: None,
            cleanup_executed: false,
        },
    );
}

fn record_blocked(
    plan: &SuitePlan,
    id: &str,
    context: &SuiteContext,
    blocker: &str,
    setup_status: AttemptStatus,
    state: &mut RunState,
) {
    let status = if setup_status == AttemptStatus::Skipped {
        AttemptStatus::BlockedSkip
    } else {
        AttemptStatus::BlockedSetup
    };
    if plan.nodes[id].is_suite() {
        for child in plan.children(id) {
            record_blocked(
                plan,
                child,
                &context.child(child),
                blocker,
                setup_status,
                state,
            );
        }
    }
    state.results.insert(
        id.to_owned(),
        SuiteResult {
            id: id.to_owned(),
            status,
            phase: None,
            blocked_by: Some(blocker.to_owned()),
            cleanup_executed: false,
        },
    );
}

fn invoke(action: Option<&SuiteAction>, context: &SuiteContext) -> Option<SuiteActionError> {
    let action = action?;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| action(context))) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(payload) => Some(SuiteActionError::Panic {
            code: "P0007".into(),
            message: panic_message(payload),
        }),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload is not displayable".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passed_test(id: &str, parent: &str, events: Arc<Mutex<Vec<String>>>) -> SuiteNode {
        let id = id.to_owned();
        SuiteNode::test(id.clone(), Some(parent.to_owned()), move |context| {
            events
                .lock()
                .unwrap()
                .push(format!("body:{id}:{}", context.run_id()));
            Ok(())
        })
    }

    #[test]
    fn setup_runs_once_and_teardown_is_inside_out() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let outer_events = events.clone();
        let inner_setup_events = events.clone();
        let root_teardown_events = events.clone();
        let inner_teardown_events = events.clone();
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>)
                .with_setup(move |_| {
                    outer_events.lock().unwrap().push("setup:root".into());
                    Ok(())
                })
                .with_teardown(move |_| {
                    root_teardown_events
                        .lock()
                        .unwrap()
                        .push("teardown:root".into());
                    Ok(())
                }),
            SuiteNode::suite("root::inner", Some("root"))
                .with_setup(move |_| {
                    inner_setup_events
                        .lock()
                        .unwrap()
                        .push("setup:inner".into());
                    Ok(())
                })
                .with_teardown(move |_| {
                    inner_teardown_events
                        .lock()
                        .unwrap()
                        .push("teardown:inner".into());
                    Ok(())
                }),
            passed_test("root::inner::a", "root::inner", events.clone()),
            passed_test("root::inner::b", "root::inner", events.clone()),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert_eq!(
            *events.lock().unwrap(),
            [
                "setup:root",
                "setup:inner",
                "body:root::inner::a:1",
                "body:root::inner::b:1",
                "teardown:inner",
                "teardown:root"
            ]
        );
        assert_eq!(
            report.result("root").unwrap().status(),
            AttemptStatus::Passed
        );
    }

    #[test]
    fn unselected_subtree_has_no_lifecycle() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let setup_events = events.clone();
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>).with_setup(move |_| {
                setup_events.lock().unwrap().push("setup".into());
                Ok(())
            }),
            SuiteNode::test("root::leaf", Some("root"), |_| Ok(())).with_selected(false),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert!(report.results().is_empty());
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn setup_failure_blocks_only_its_subtree_and_sibling_continues() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let sibling_events = events.clone();
        let cleanup_events = events.clone();
        let plan = SuitePlan::new([
            SuiteNode::suite("bad", None::<String>)
                .with_setup(|_| {
                    Err(SuiteActionError::Error {
                        code: "ESETUP".into(),
                        message: "boom".into(),
                    })
                })
                .with_cleanup(move |_| {
                    cleanup_events.lock().unwrap().push("bad-cleanup".into());
                    Ok(())
                }),
            SuiteNode::test("bad::leaf", Some("bad"), |_| panic!("must not run")),
            SuiteNode::suite("good", None::<String>),
            SuiteNode::test("good::leaf", Some("good"), move |_| {
                sibling_events.lock().unwrap().push("good".into());
                Ok(())
            }),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert_eq!(
            report.result("bad").unwrap().status(),
            AttemptStatus::FailedError
        );
        assert_eq!(
            report.result("bad::leaf").unwrap().status(),
            AttemptStatus::BlockedSetup
        );
        assert_eq!(
            report.result("good::leaf").unwrap().status(),
            AttemptStatus::Passed
        );
        assert_eq!(*events.lock().unwrap(), ["bad-cleanup", "good"]);
    }

    #[test]
    fn skipped_setup_marks_descendants_blocked_skip() {
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>).with_setup(|_| {
                Err(SuiteActionError::Skip {
                    reason: "platform".into(),
                })
            }),
            SuiteNode::test("root::leaf", Some("root"), |_| panic!("must not run")),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert_eq!(
            report.result("root").unwrap().status(),
            AttemptStatus::Skipped
        );
        assert_eq!(
            report.result("root::leaf").unwrap().status(),
            AttemptStatus::BlockedSkip
        );
    }

    #[test]
    fn teardown_failure_does_not_rewrite_leaf_result() {
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>).with_teardown(|_| {
                Err(SuiteActionError::Panic {
                    code: "PTEARDOWN".into(),
                    message: "cleanup failed".into(),
                })
            }),
            SuiteNode::test("root::leaf", Some("root"), |_| Ok(())),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert_eq!(
            report.result("root::leaf").unwrap().status(),
            AttemptStatus::Passed
        );
        assert_eq!(
            report.result("root").unwrap().status(),
            AttemptStatus::FailedPanic
        );
        assert_eq!(
            report.result("root").unwrap().phase(),
            Some(SuitePhase::Teardown)
        );
    }

    #[test]
    fn cleanup_is_lifo_and_cleanup_failure_precedes_setup_failure() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let first = events.clone();
        let second = events.clone();
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>)
                .with_setup(|_| {
                    Err(SuiteActionError::Error {
                        code: "ESETUP".into(),
                        message: "setup".into(),
                    })
                })
                .with_cleanup(move |_| {
                    first.lock().unwrap().push("first".into());
                    Ok(())
                })
                .with_cleanup(move |_| {
                    second.lock().unwrap().push("second".into());
                    Err(SuiteActionError::Panic {
                        code: "ECLEANUP".into(),
                        message: "cleanup".into(),
                    })
                }),
            SuiteNode::test("root::leaf", Some("root"), |_| Ok(())),
        ])
        .unwrap();
        let report = SuiteRunner::new().run(&plan);
        assert_eq!(*events.lock().unwrap(), ["second"]);
        assert_eq!(
            report.result("root").unwrap().status(),
            AttemptStatus::FailedPanic
        );
        assert_eq!(
            report.result("root").unwrap().phase(),
            Some(SuitePhase::Cleanup)
        );
        assert_eq!(
            report.result("root::leaf").unwrap().status(),
            AttemptStatus::BlockedSetup
        );
    }

    #[test]
    fn setup_environment_is_available_to_descendants_and_fresh_on_retry() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let setup_observed = observed.clone();
        let body_observed = observed.clone();
        let plan = SuitePlan::new([
            SuiteNode::suite("root", None::<String>).with_setup(move |context| {
                context.set("token", "fresh").map_err(|error| {
                    SuiteActionError::Infrastructure {
                        message: error.to_string(),
                    }
                })?;
                setup_observed.lock().unwrap().push(context.run_id());
                Ok(())
            }),
            SuiteNode::test("root::leaf", Some("root"), move |context| {
                assert_eq!(context.get("token").unwrap().as_deref(), Some("fresh"));
                body_observed.lock().unwrap().push(context.run_id());
                Ok(())
            }),
        ])
        .unwrap();
        let runner = SuiteRunner::new();
        let first = runner.run(&plan);
        let second = runner.run(&plan);
        assert_ne!(first.run_id(), second.run_id());
        assert_eq!(*observed.lock().unwrap(), [1, 1, 2, 2]);
    }

    #[test]
    fn invalid_trees_are_rejected() {
        assert_eq!(
            SuitePlan::new([SuiteNode::test(" ", None::<String>, |_| Ok(()))]).unwrap_err(),
            SuiteError::EmptyId
        );
        assert!(matches!(
            SuitePlan::new([SuiteNode::test("leaf", Some("missing"), |_| Ok(()))]),
            Err(SuiteError::UnknownParent { .. })
        ));
        assert!(matches!(
            SuitePlan::new([
                SuiteNode::test("parent", None::<String>, |_| Ok(())),
                SuiteNode::test("child", Some("parent"), |_| Ok(())),
            ]),
            Err(SuiteError::TestParentIsLeaf { .. })
        ));
        assert!(matches!(
            SuitePlan::new([
                SuiteNode::suite("root", None::<String>),
                SuiteNode::suite("root", None::<String>),
            ]),
            Err(SuiteError::DuplicateId(_))
        ));
    }
}
