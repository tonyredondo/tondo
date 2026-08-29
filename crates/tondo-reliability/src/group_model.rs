//! Independent, bounded reference model for `std.async.Group`.
//!
//! The VM owns the production implementation.  This model deliberately uses
//! only ordinary Rust collections and explicit transitions so that tests and
//! fuzzing can compare observable group behaviour without reusing VM state.

use std::collections::BTreeSet;

/// Maximum number of children accepted by a model run.
pub const MAX_CHILDREN: usize = 64;
/// Maximum bytes consumed by one fuzz input.
pub const MAX_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum state transitions in one fuzz input.
pub const MAX_FUZZ_STEPS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupState {
    Open,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChildState<T, E> {
    Pending,
    CancellationRequested,
    Terminal {
        order: u64,
        outcome: GroupOutcome<T, E>,
    },
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Child<T, E> {
    index: usize,
    state: ChildState<T, E>,
    cleanup_runs: u8,
}

/// A result produced by a modeled child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupOutcome<T, E> {
    /// The child returned its value.
    Value(T),
    /// The child returned its declared error.
    Error(E),
    /// The child panicked outside its declared error channel.
    Panic(String),
    /// The child was cancelled during group cleanup.
    Cancelled,
}

/// Errors for invalid model operations.  These are expected negative cases,
/// not failures of the model itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    Consumed,
    Limit,
    UnknownChild,
    ChildNotPending,
    InvalidProbe,
    NotOwner,
    OrderOverflow,
}

/// A stable selection witness.  Preparing a witness never mutates the group;
/// only committing it removes one completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupProbe {
    index: usize,
    order: u64,
}

/// A summary of a panic and its suppressed sibling panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicSummary {
    /// The first panic in insertion order.
    pub primary: String,
    /// Later panics observed during the same terminal drain.
    pub suppressed: Vec<String>,
}

/// Observation returned by `next`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextPoll<T, E> {
    Pending,
    None,
    Completed { index: usize, outcome: Result<T, E> },
    Panicked(PanicSummary),
}

/// Observation returned by `all`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllPoll<T, E> {
    Pending,
    Ready(Result<Vec<T>, E>),
    Panicked(PanicSummary),
}

/// Observation returned by `settle`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlePoll<T, E> {
    Pending,
    Ready(Vec<Result<T, E>>),
    Panicked(PanicSummary),
}

/// A bounded, affine reference model for `Group[T, E]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupModel<T, E> {
    owner_scope: u32,
    state: GroupState,
    children: Vec<Child<T, E>>,
    next_order: u64,
}

/// Cheap state observation used to prove that select rollback is atomic and
/// that a fuzz run leaves no pending child or duplicate cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSnapshot {
    pub owner_scope: u32,
    pub consumed: bool,
    pub child_count: usize,
    pub pending_children: usize,
    pub terminal_children: usize,
    pub consumed_children: usize,
    pub cleanup_runs: usize,
}

impl<T, E> GroupModel<T, E> {
    /// Create an open group owned by one lexical task scope.
    pub fn new(owner_scope: u32) -> Self {
        Self {
            owner_scope,
            state: GroupState::Open,
            children: Vec::new(),
            next_order: 0,
        }
    }

    /// Return the current lexical owner.
    pub fn owner_scope(&self) -> u32 {
        self.owner_scope
    }

    /// Move the affine group to another scope.
    pub fn transfer(&mut self, from_scope: u32, to_scope: u32) -> Result<(), ModelError> {
        self.ensure_open()?;
        if self.owner_scope != from_scope {
            return Err(ModelError::NotOwner);
        }
        self.owner_scope = to_scope;
        Ok(())
    }

    /// Add one not-yet-completed child and return its stable insertion index.
    pub fn add(&mut self) -> Result<usize, ModelError> {
        self.ensure_open()?;
        if self.children.len() >= MAX_CHILDREN {
            return Err(ModelError::Limit);
        }
        let index = self.children.len();
        self.children.push(Child {
            index,
            state: ChildState::Pending,
            cleanup_runs: 0,
        });
        Ok(index)
    }

    /// Complete a child at an explicit scheduler order.  Equal orders model
    /// simultaneous completion and are resolved by insertion index.
    pub fn complete(
        &mut self,
        index: usize,
        order: u64,
        outcome: GroupOutcome<T, E>,
    ) -> Result<(), ModelError> {
        self.ensure_open()?;
        let next_order = order.checked_add(1).ok_or(ModelError::OrderOverflow)?;
        let child = self
            .children
            .get_mut(index)
            .ok_or(ModelError::UnknownChild)?;
        if !matches!(
            child.state,
            ChildState::Pending | ChildState::CancellationRequested
        ) {
            return Err(ModelError::ChildNotPending);
        }
        child.state = ChildState::Terminal { order, outcome };
        self.next_order = self.next_order.max(next_order);
        Ok(())
    }

    /// Request cancellation without publishing a user-visible `E`.
    pub fn request_cancel(&mut self) -> Result<usize, ModelError> {
        self.ensure_open()?;
        let mut requested = 0;
        for child in &mut self.children {
            if matches!(child.state, ChildState::Pending) {
                child.state = ChildState::CancellationRequested;
                requested += 1;
            }
        }
        Ok(requested)
    }

    /// Deterministically drain cancellation requests.  Cleanup is performed
    /// later, when a terminal consumer takes the child completion.
    pub fn drain_cancel(&mut self) -> Result<usize, ModelError> {
        self.ensure_open()?;
        let mut drained = 0;
        for child in &mut self.children {
            if matches!(child.state, ChildState::CancellationRequested) {
                let order = self.next_order;
                self.next_order = self
                    .next_order
                    .checked_add(1)
                    .ok_or(ModelError::OrderOverflow)?;
                child.state = ChildState::Terminal {
                    order,
                    outcome: GroupOutcome::Cancelled,
                };
                drained += 1;
            }
        }
        Ok(drained)
    }

    /// Probe the earliest terminal child without consuming it.
    pub fn probe_next(&self) -> Result<Option<GroupProbe>, ModelError> {
        self.ensure_open()?;
        Ok(self
            .children
            .iter()
            .filter_map(|child| match &child.state {
                ChildState::Terminal { order, .. } => Some(GroupProbe {
                    index: child.index,
                    order: *order,
                }),
                ChildState::Pending | ChildState::CancellationRequested | ChildState::Consumed => {
                    None
                }
            })
            .min_by_key(|probe| (probe.order, probe.index)))
    }

    /// Roll back a losing `select` arm.  The witness remains valid and no
    /// group state changes.
    pub fn rollback_probe(&self, probe: Option<GroupProbe>) -> Result<(), ModelError> {
        self.ensure_open()?;
        let Some(probe) = probe else {
            return Ok(());
        };
        let child = self
            .children
            .get(probe.index)
            .ok_or(ModelError::InvalidProbe)?;
        if matches!(
            child.state,
            ChildState::Terminal { order, .. } if order == probe.order
        ) {
            Ok(())
        } else {
            Err(ModelError::InvalidProbe)
        }
    }

    /// Commit a selected completion.  Cancelled children are skipped exactly
    /// as the public `Group.next` operation skips them.
    pub fn commit_probe(&mut self, probe: GroupProbe) -> Result<NextPoll<T, E>, ModelError> {
        self.ensure_open()?;
        let child = self
            .children
            .get_mut(probe.index)
            .ok_or(ModelError::InvalidProbe)?;
        let state = std::mem::replace(&mut child.state, ChildState::Consumed);
        let ChildState::Terminal { order, outcome } = state else {
            child.state = state;
            return Err(ModelError::InvalidProbe);
        };
        if order != probe.order {
            child.state = ChildState::Terminal { order, outcome };
            return Err(ModelError::InvalidProbe);
        }
        child.cleanup_runs = child.cleanup_runs.saturating_add(1);
        match outcome {
            GroupOutcome::Value(value) => Ok(NextPoll::Completed {
                index: probe.index,
                outcome: Ok(value),
            }),
            GroupOutcome::Error(error) => Ok(NextPoll::Completed {
                index: probe.index,
                outcome: Err(error),
            }),
            GroupOutcome::Panic(message) => Ok(NextPoll::Panicked(PanicSummary {
                primary: message,
                suppressed: Vec::new(),
            })),
            GroupOutcome::Cancelled => self.poll_next(),
        }
    }

    /// Consume one completion in scheduler order.
    pub fn poll_next(&mut self) -> Result<NextPoll<T, E>, ModelError> {
        self.ensure_open()?;
        loop {
            let Some(probe) = self.probe_next()? else {
                return Ok(if self.has_pending() {
                    NextPoll::Pending
                } else {
                    NextPoll::None
                });
            };
            match self.commit_probe(probe)? {
                NextPoll::Completed { index, outcome } => {
                    return Ok(NextPoll::Completed { index, outcome });
                }
                NextPoll::Panicked(panic) => return Ok(NextPoll::Panicked(panic)),
                NextPoll::Pending | NextPoll::None => continue,
            }
        }
    }

    /// Wait for all remaining children and return successful values in
    /// insertion order, or the first error in insertion order.
    pub fn all(&mut self) -> Result<AllPoll<T, E>, ModelError>
    where
        T: Clone,
        E: Clone,
    {
        self.ensure_open()?;
        if self.has_terminal_failure() {
            self.request_cancel()?;
        }
        if self.has_pending() {
            return Ok(AllPoll::Pending);
        }
        let mut values = Vec::new();
        let mut error = None;
        let mut panic = None;
        for child in &self.children {
            let ChildState::Terminal { outcome, .. } = &child.state else {
                continue;
            };
            match outcome {
                GroupOutcome::Value(value) => values.push(value.clone()),
                GroupOutcome::Error(child_error) => {
                    if error.is_none() {
                        error = Some(child_error.clone());
                    }
                }
                GroupOutcome::Panic(message) => record_panic(&mut panic, message),
                GroupOutcome::Cancelled => {}
            }
        }
        self.consume_remaining()?;
        self.state = GroupState::Consumed;
        if let Some(panic) = panic {
            Ok(AllPoll::Panicked(panic))
        } else {
            Ok(AllPoll::Ready(error.map_or(Ok(values), Err)))
        }
    }

    /// Wait for all remaining children and preserve one outcome per remaining
    /// child in insertion order.  Cancellation is not fabricated as `E`.
    pub fn settle(&mut self) -> Result<SettlePoll<T, E>, ModelError>
    where
        T: Clone,
        E: Clone,
    {
        self.ensure_open()?;
        if self.has_terminal_panic() {
            self.request_cancel()?;
        }
        if self.has_pending() {
            return Ok(SettlePoll::Pending);
        }
        let mut outcomes = Vec::new();
        let mut panic = None;
        for child in &self.children {
            let ChildState::Terminal { outcome, .. } = &child.state else {
                continue;
            };
            match outcome {
                GroupOutcome::Value(value) => outcomes.push(Ok(value.clone())),
                GroupOutcome::Error(error) => outcomes.push(Err(error.clone())),
                GroupOutcome::Panic(message) => record_panic(&mut panic, message),
                GroupOutcome::Cancelled => {}
            }
        }
        self.consume_remaining()?;
        self.state = GroupState::Consumed;
        if let Some(panic) = panic {
            Ok(SettlePoll::Panicked(panic))
        } else {
            Ok(SettlePoll::Ready(outcomes))
        }
    }

    /// Request, drain and consume every remaining child.  The model treats
    /// this as the post-scheduler observation of the suspending `cancel` API.
    pub fn cancel(&mut self) -> Result<(), ModelError> {
        self.ensure_open()?;
        self.request_cancel()?;
        self.drain_cancel()?;
        self.consume_remaining()?;
        self.state = GroupState::Consumed;
        Ok(())
    }

    /// Return a compact state snapshot.
    pub fn snapshot(&self) -> GroupSnapshot {
        let mut pending_children = 0;
        let mut terminal_children = 0;
        let mut consumed_children = 0;
        let mut cleanup_runs = 0;
        for child in &self.children {
            match child.state {
                ChildState::Pending | ChildState::CancellationRequested => pending_children += 1,
                ChildState::Terminal { .. } => terminal_children += 1,
                ChildState::Consumed => consumed_children += 1,
            }
            cleanup_runs += usize::from(child.cleanup_runs);
        }
        GroupSnapshot {
            owner_scope: self.owner_scope,
            consumed: self.state == GroupState::Consumed,
            child_count: self.children.len(),
            pending_children,
            terminal_children,
            consumed_children,
            cleanup_runs,
        }
    }

    /// Check all model invariants without exposing internal child storage.
    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.children.len() > MAX_CHILDREN {
            return Err("model exceeded its child limit".into());
        }
        let mut indexes = BTreeSet::new();
        let mut cleanup_runs = 0usize;
        for (position, child) in self.children.iter().enumerate() {
            if child.index != position || !indexes.insert(child.index) {
                return Err("group indexes are not unique and stable".into());
            }
            if child.cleanup_runs > 1 {
                return Err("a group child was cleaned up more than once".into());
            }
            cleanup_runs += usize::from(child.cleanup_runs);
            match &child.state {
                ChildState::Terminal { order, .. } if *order >= self.next_order => {
                    return Err("terminal order is outside the scheduler horizon".into());
                }
                ChildState::Consumed if child.cleanup_runs != 1 => {
                    return Err("consumed child did not run cleanup exactly once".into());
                }
                ChildState::Pending
                | ChildState::CancellationRequested
                | ChildState::Terminal { .. }
                | ChildState::Consumed => {}
            }
        }
        if cleanup_runs != self.snapshot().cleanup_runs {
            return Err("cleanup accounting is inconsistent".into());
        }
        if self.state == GroupState::Consumed
            && self
                .children
                .iter()
                .any(|child| !matches!(child.state, ChildState::Consumed))
        {
            return Err("consumed group retains a live child".into());
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<(), ModelError> {
        (self.state == GroupState::Open)
            .then_some(())
            .ok_or(ModelError::Consumed)
    }

    fn has_pending(&self) -> bool {
        self.children.iter().any(|child| {
            matches!(
                child.state,
                ChildState::Pending | ChildState::CancellationRequested
            )
        })
    }

    fn has_terminal_failure(&self) -> bool {
        self.children.iter().any(|child| {
            matches!(
                child.state,
                ChildState::Terminal {
                    outcome: GroupOutcome::Error(_) | GroupOutcome::Panic(_),
                    ..
                }
            )
        })
    }

    fn has_terminal_panic(&self) -> bool {
        self.children.iter().any(|child| {
            matches!(
                child.state,
                ChildState::Terminal {
                    outcome: GroupOutcome::Panic(_),
                    ..
                }
            )
        })
    }

    fn consume_remaining(&mut self) -> Result<(), ModelError> {
        for child in &mut self.children {
            if matches!(child.state, ChildState::Terminal { .. }) {
                child.state = ChildState::Consumed;
                child.cleanup_runs = child.cleanup_runs.saturating_add(1);
            }
        }
        if self
            .children
            .iter()
            .any(|child| !matches!(child.state, ChildState::Consumed))
        {
            return Err(ModelError::ChildNotPending);
        }
        Ok(())
    }
}

fn record_panic(panic: &mut Option<PanicSummary>, message: &str) {
    if let Some(primary) = panic {
        primary.suppressed.push(message.to_owned());
    } else {
        *panic = Some(PanicSummary {
            primary: message.to_owned(),
            suppressed: Vec::new(),
        });
    }
}

/// Deterministically exercise the model with bounded bytes.  Invalid user
/// operations are deliberately ignored; only invariant failures are returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzSummary {
    pub steps: usize,
    pub accepted_operations: usize,
    pub rejected_operations: usize,
    pub snapshot: GroupSnapshot,
}

pub fn run_fuzz_case(input: &[u8]) -> Result<FuzzSummary, String> {
    let input = &input[..input.len().min(MAX_FUZZ_INPUT_BYTES)];
    let steps = input.len().clamp(1, MAX_FUZZ_STEPS);
    let input_len = input.len().max(1);
    let mut model = GroupModel::<i64, String>::new(0);
    let mut accepted_operations = 0;
    let mut rejected_operations = 0;
    for step in 0..steps {
        let byte = input.get(step % input_len).copied().unwrap_or_default();
        let argument = input
            .get((step + 1) % input_len)
            .copied()
            .unwrap_or_default();
        let operation = byte % 12;
        let result = match operation {
            0 => model.add().map(|_| ()),
            1..=3 => {
                let index = usize::from(argument) % model.snapshot().child_count.max(1);
                let outcome = match operation {
                    1 => GroupOutcome::Value(i64::from(argument)),
                    2 => GroupOutcome::Error(format!("e{argument}")),
                    _ => GroupOutcome::Panic(format!("p{argument}")),
                };
                model.complete(index, u64::from(step as u32), outcome)
            }
            4 => match model.probe_next() {
                Ok(probe) => {
                    let before = model.snapshot();
                    let result = model.rollback_probe(probe);
                    if result.is_ok() && before != model.snapshot() {
                        return Err("select rollback mutated the group".into());
                    }
                    result
                }
                Err(error) => Err(error),
            },
            5 => match model.probe_next() {
                Ok(Some(probe)) => model.commit_probe(probe).map(|_| ()),
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            },
            6 => model.request_cancel().map(|_| ()),
            7 => model.drain_cancel().map(|_| ()),
            8 => model.poll_next().map(|_| ()),
            9 => model.all().map(|_| ()),
            10 => model.settle().map(|_| ()),
            _ => model.transfer(model.owner_scope(), u32::from(argument)),
        };
        if result.is_ok() {
            accepted_operations += 1;
        } else {
            rejected_operations += 1;
        }
        model.assert_invariants()?;
    }
    if !model.snapshot().consumed {
        let _ = model.cancel();
    }
    model.assert_invariants()?;
    Ok(FuzzSummary {
        steps,
        accepted_operations,
        rejected_operations,
        snapshot: model.snapshot(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_preserves_insertion_order_and_lowest_error() {
        let mut group = GroupModel::<i64, &'static str>::new(0);
        let first = group.add().unwrap();
        let second = group.add().unwrap();
        let third = group.add().unwrap();
        group
            .complete(first, 3, GroupOutcome::Error("first"))
            .unwrap();
        group.complete(second, 1, GroupOutcome::Value(2)).unwrap();
        group
            .complete(third, 2, GroupOutcome::Error("second"))
            .unwrap();
        assert_eq!(group.all().unwrap(), AllPoll::Ready(Err("first")));
        assert_eq!(group.snapshot().cleanup_runs, 3);
        assert!(matches!(group.all(), Err(ModelError::Consumed)));
    }

    #[test]
    fn settle_preserves_each_declared_outcome_and_panics_are_suppressed() {
        let mut group = GroupModel::<i64, &'static str>::new(0);
        let value = group.add().unwrap();
        let error = group.add().unwrap();
        let panic = group.add().unwrap();
        let second_panic = group.add().unwrap();
        group.complete(value, 0, GroupOutcome::Value(4)).unwrap();
        group
            .complete(error, 1, GroupOutcome::Error("bad"))
            .unwrap();
        group
            .complete(panic, 2, GroupOutcome::Panic("one".into()))
            .unwrap();
        group
            .complete(second_panic, 3, GroupOutcome::Panic("two".into()))
            .unwrap();
        assert!(matches!(
            group.settle().unwrap(),
            SettlePoll::Panicked(PanicSummary { suppressed, .. }) if suppressed == ["two"]
        ));
        assert_eq!(group.snapshot().cleanup_runs, 4);
    }

    #[test]
    fn terminal_failures_request_a_drain_before_publishing_an_outcome() {
        let mut all_group = GroupModel::<i64, &'static str>::new(0);
        let error = all_group.add().unwrap();
        let pending = all_group.add().unwrap();
        all_group
            .complete(error, 0, GroupOutcome::Error("bad"))
            .unwrap();
        assert!(matches!(all_group.all().unwrap(), AllPoll::Pending));
        assert_eq!(all_group.snapshot().pending_children, 1);
        all_group.drain_cancel().unwrap();
        assert_eq!(all_group.all().unwrap(), AllPoll::Ready(Err("bad")));
        assert_eq!(pending, 1);

        let mut settle_group = GroupModel::<i64, &'static str>::new(0);
        let panic = settle_group.add().unwrap();
        let pending = settle_group.add().unwrap();
        settle_group
            .complete(panic, 0, GroupOutcome::Panic("boom".into()))
            .unwrap();
        assert!(matches!(
            settle_group.settle().unwrap(),
            SettlePoll::Pending
        ));
        settle_group.drain_cancel().unwrap();
        assert!(matches!(
            settle_group.settle().unwrap(),
            SettlePoll::Panicked(PanicSummary { primary, suppressed })
                if primary == "boom" && suppressed.is_empty()
        ));
        assert_eq!(pending, 1);
    }

    #[test]
    fn next_orders_by_completion_then_index_and_skips_cancelled() {
        let mut group = GroupModel::<i64, &'static str>::new(0);
        let first = group.add().unwrap();
        let second = group.add().unwrap();
        let third = group.add().unwrap();
        group.complete(first, 9, GroupOutcome::Value(1)).unwrap();
        group.complete(second, 2, GroupOutcome::Cancelled).unwrap();
        group.complete(third, 2, GroupOutcome::Value(3)).unwrap();
        assert_eq!(
            group.poll_next().unwrap(),
            NextPoll::Completed {
                index: third,
                outcome: Ok(3)
            }
        );
        assert_eq!(
            group.poll_next().unwrap(),
            NextPoll::Completed {
                index: first,
                outcome: Ok(1)
            }
        );
        assert_eq!(group.poll_next().unwrap(), NextPoll::None);
        assert_eq!(group.snapshot().cleanup_runs, 3);
    }

    #[test]
    fn probe_rollback_is_atomic_and_commit_rejects_stale_witnesses() {
        let mut group = GroupModel::<i64, &'static str>::new(4);
        let child = group.add().unwrap();
        group.complete(child, 0, GroupOutcome::Value(7)).unwrap();
        let probe = group.probe_next().unwrap();
        let before = group.snapshot();
        group.rollback_probe(probe).unwrap();
        assert_eq!(group.snapshot(), before);
        let probe = probe.unwrap();
        group.commit_probe(probe).unwrap();
        assert!(matches!(
            group.commit_probe(probe),
            Err(ModelError::InvalidProbe)
        ));
        assert_eq!(group.probe_next(), Ok(None));
    }

    #[test]
    fn cancellation_transfer_limits_and_invariants_are_explicit() {
        let mut group = GroupModel::<i64, &'static str>::new(1);
        assert!(matches!(group.transfer(2, 3), Err(ModelError::NotOwner)));
        group.transfer(1, 3).unwrap();
        assert_eq!(group.owner_scope(), 3);
        let child = group.add().unwrap();
        assert_eq!(group.request_cancel().unwrap(), 1);
        assert!(matches!(group.all().unwrap(), AllPoll::Pending));
        assert_eq!(group.drain_cancel().unwrap(), 1);
        assert_eq!(group.all().unwrap(), AllPoll::Ready(Ok(Vec::new())));
        assert_eq!(group.snapshot().cleanup_runs, 1);
        assert!(matches!(group.add(), Err(ModelError::Consumed)));
        assert_eq!(child, 0);

        let mut limited = GroupModel::<(), ()>::new(0);
        for _ in 0..MAX_CHILDREN {
            limited.add().unwrap();
        }
        assert!(matches!(limited.add(), Err(ModelError::Limit)));
        limited.cancel().unwrap();
        limited.assert_invariants().unwrap();
    }

    #[test]
    fn fuzz_replay_is_bounded_and_deterministic() {
        for seed in 0..128_u64 {
            let bytes = seed.to_le_bytes();
            let first = run_fuzz_case(&bytes).unwrap();
            let second = run_fuzz_case(&bytes).unwrap();
            assert_eq!(first, second);
            assert!(first.steps <= MAX_FUZZ_STEPS);
            assert!(first.snapshot.consumed);
            assert_eq!(
                first.snapshot.cleanup_runs,
                first.snapshot.consumed_children
            );
        }
    }
}
