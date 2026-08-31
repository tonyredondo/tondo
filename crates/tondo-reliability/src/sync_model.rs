//! Independent, bounded models for `std.sync`.
//!
//! The production implementation lives in the hosted VM and native runtime.
//! These models intentionally use only ordinary Rust collections and explicit
//! transitions.  They are an oracle for ordering, wakeups, cancellation and
//! cleanup; they do not share state or code with either runtime.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum number of modeled tasks in one bounded run.
pub const MAX_SYNC_TASKS: usize = 64;
/// Maximum bytes accepted by a sync fuzz case.
pub const MAX_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum transitions accepted by a sync fuzz case.
pub const MAX_FUZZ_STEPS: usize = 1_024;

/// Memory orders exposed by `std.sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryOrder {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

impl MemoryOrder {
    /// Every public memory order in its normative source order.
    pub const ALL: [Self; 5] = [
        Self::Relaxed,
        Self::Acquire,
        Self::Release,
        Self::AcqRel,
        Self::SeqCst,
    ];

    /// Whether this order is valid for an atomic load.
    pub const fn valid_load(self) -> bool {
        matches!(self, Self::Relaxed | Self::Acquire | Self::SeqCst)
    }

    /// Whether this order is valid for an atomic store.
    pub const fn valid_store(self) -> bool {
        matches!(self, Self::Relaxed | Self::Release | Self::SeqCst)
    }

    /// Whether this order is valid as a compare-exchange failure order.
    pub const fn valid_failure(self) -> bool {
        matches!(self, Self::Relaxed | Self::Acquire | Self::SeqCst)
    }

    /// The closed compatibility relation for compare-exchange success/failure.
    pub const fn valid_compare_exchange(success: Self, failure: Self) -> bool {
        match success {
            Self::Relaxed => matches!(failure, Self::Relaxed),
            Self::Acquire => matches!(failure, Self::Relaxed | Self::Acquire),
            Self::Release => matches!(failure, Self::Relaxed),
            Self::AcqRel => matches!(failure, Self::Relaxed | Self::Acquire),
            Self::SeqCst => matches!(failure, Self::Relaxed | Self::Acquire | Self::SeqCst),
        }
    }

    const fn strength(self) -> u8 {
        match self {
            Self::Relaxed => 0,
            Self::Acquire | Self::Release => 1,
            Self::AcqRel => 2,
            Self::SeqCst => 3,
        }
    }

    /// Whether a release store and an acquire load form a publication edge.
    pub const fn synchronizes_with(writer: Self, reader: Self) -> bool {
        matches!(writer, Self::Release | Self::AcqRel | Self::SeqCst)
            && matches!(reader, Self::Acquire | Self::SeqCst)
            && reader.strength() >= Self::Acquire.strength()
    }
}

/// Errors produced by invalid model transitions.  They are expected negative
/// cases and never represent a panic in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncModelError {
    InvalidCapacity,
    InvalidParties,
    Limit,
    NotOwner,
    Reentrant,
    UnknownTask,
    NotWaiting,
    AlreadyHeld,
    AlreadyReleased,
    InvalidOrder,
    Invariant,
}

/// Result of trying to acquire a mutex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPoll {
    Acquired,
    Waiting,
    Reentrant,
}

/// A compact mutex state machine with FIFO registration and cancellation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutexModel {
    owner: Option<u32>,
    waiters: VecDeque<u32>,
    cleanup_runs: BTreeMap<u32, u8>,
    wakeups: usize,
}

impl MutexModel {
    pub fn new() -> Self {
        Self {
            owner: None,
            waiters: VecDeque::new(),
            cleanup_runs: BTreeMap::new(),
            wakeups: 0,
        }
    }

    pub fn owner(&self) -> Option<u32> {
        self.owner
    }

    pub fn waiting(&self) -> Vec<u32> {
        self.waiters.iter().copied().collect()
    }

    pub fn wakeups(&self) -> usize {
        self.wakeups
    }

    pub fn cleanup_runs(&self, task: u32) -> u8 {
        self.cleanup_runs.get(&task).copied().unwrap_or(0)
    }

    /// Try to acquire without registering a waiter.
    pub fn try_lock(&mut self, task: u32) -> Result<bool, SyncModelError> {
        if self.owner == Some(task) {
            return Err(SyncModelError::Reentrant);
        }
        if self.owner.is_none() && self.waiters.is_empty() {
            self.owner = Some(task);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Register and, when possible, acquire in one transition.
    pub fn lock(&mut self, task: u32) -> Result<LockPoll, SyncModelError> {
        if self.owner == Some(task) {
            return Ok(LockPoll::Reentrant);
        }
        if self.owner.is_none() && self.waiters.is_empty() {
            self.owner = Some(task);
            return Ok(LockPoll::Acquired);
        }
        if self.waiters.contains(&task) {
            return Err(SyncModelError::AlreadyHeld);
        }
        self.waiters.push_back(task);
        Ok(LockPoll::Waiting)
    }

    /// Cancel a queued waiter without changing ownership.
    pub fn cancel_wait(&mut self, task: u32) -> Result<(), SyncModelError> {
        let Some(index) = self.waiters.iter().position(|candidate| *candidate == task) else {
            return Err(SyncModelError::NotWaiting);
        };
        self.waiters.remove(index);
        Ok(())
    }

    /// Release the guard and hand it directly to the oldest waiter.
    pub fn unlock(&mut self, task: u32) -> Result<Option<u32>, SyncModelError> {
        if self.owner != Some(task) {
            return Err(SyncModelError::NotOwner);
        }
        self.owner = self.waiters.pop_front();
        self.cleanup_runs
            .entry(task)
            .and_modify(|runs| *runs = runs.saturating_add(1))
            .or_insert(1);
        if self.owner.is_some() {
            self.wakeups = self.wakeups.saturating_add(1);
        }
        Ok(self.owner)
    }

    /// Deterministically release a held guard during teardown.
    pub fn cleanup_owner(&mut self) -> Result<(), SyncModelError> {
        if let Some(owner) = self.owner {
            self.unlock(owner)?;
        }
        Ok(())
    }

    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.waiters.len() > MAX_SYNC_TASKS {
            return Err("mutex waiter limit exceeded".into());
        }
        let unique = self.waiters.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != self.waiters.len() {
            return Err("mutex registered a task twice".into());
        }
        if let Some(owner) = self.owner
            && self.waiters.contains(&owner)
        {
            return Err("mutex owner remained in its waiter queue".into());
        }
        if self.cleanup_runs.values().any(|runs| *runs > 1) {
            return Err("mutex guard cleanup ran more than once".into());
        }
        Ok(())
    }
}

impl Default for MutexModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Observation returned by a condition wait transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionPoll {
    Waiting,
    Reacquired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConditionWaiter {
    task: u32,
    notified: bool,
}

/// Condition model.  `wait` is intentionally atomic: release and registration
/// happen in one method, so a notify cannot fall into an unlock/register gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionModel {
    guard_owner: Option<u32>,
    waiters: VecDeque<ConditionWaiter>,
    wakeups: usize,
}

impl ConditionModel {
    pub fn new() -> Self {
        Self {
            guard_owner: None,
            waiters: VecDeque::new(),
            wakeups: 0,
        }
    }

    pub fn guard_owner(&self) -> Option<u32> {
        self.guard_owner
    }

    pub fn waiting(&self) -> usize {
        self.waiters.len()
    }

    pub fn wakeups(&self) -> usize {
        self.wakeups
    }

    pub fn lock_for(&mut self, task: u32) -> Result<(), SyncModelError> {
        if self.waiters.iter().any(|waiter| waiter.task == task) {
            return Err(SyncModelError::AlreadyHeld);
        }
        if self.guard_owner.is_some() {
            return Err(SyncModelError::AlreadyHeld);
        }
        self.guard_owner = Some(task);
        Ok(())
    }

    /// Release the guard and register the waiter as one indivisible step.
    pub fn wait(&mut self, task: u32) -> Result<ConditionPoll, SyncModelError> {
        if self.guard_owner != Some(task) {
            return Err(SyncModelError::NotOwner);
        }
        if self.waiters.iter().any(|waiter| waiter.task == task) {
            return Err(SyncModelError::AlreadyHeld);
        }
        self.guard_owner = None;
        self.waiters.push_back(ConditionWaiter {
            task,
            notified: false,
        });
        Ok(ConditionPoll::Waiting)
    }

    pub fn notify_one(&mut self) -> Option<u32> {
        let waiter = self.waiters.iter_mut().find(|waiter| !waiter.notified)?;
        waiter.notified = true;
        self.wakeups = self.wakeups.saturating_add(1);
        Some(waiter.task)
    }

    pub fn notify_all(&mut self) -> usize {
        let mut count = 0;
        for waiter in &mut self.waiters {
            if !waiter.notified {
                waiter.notified = true;
                count += 1;
            }
        }
        self.wakeups = self.wakeups.saturating_add(count);
        count
    }

    /// A host spurious wake is hidden: it does not make `reacquire` succeed.
    pub fn spurious_wake(&self, task: u32) -> Result<(), SyncModelError> {
        self.waiters
            .iter()
            .any(|waiter| waiter.task == task)
            .then_some(())
            .ok_or(SyncModelError::NotWaiting)
    }

    pub fn reacquire(&mut self, task: u32) -> Result<ConditionPoll, SyncModelError> {
        let Some(index) = self.waiters.iter().position(|waiter| waiter.task == task) else {
            return Err(SyncModelError::NotWaiting);
        };
        if !self.waiters[index].notified || self.guard_owner.is_some() {
            return Ok(ConditionPoll::Waiting);
        }
        self.waiters.remove(index);
        self.guard_owner = Some(task);
        Ok(ConditionPoll::Reacquired)
    }

    /// Cancellation removes the waiter and reacquires the guard before unwind.
    pub fn cancel_wait(&mut self, task: u32) -> Result<ConditionPoll, SyncModelError> {
        let Some(index) = self.waiters.iter().position(|waiter| waiter.task == task) else {
            return Err(SyncModelError::NotWaiting);
        };
        if self.guard_owner.is_some() {
            return Ok(ConditionPoll::Waiting);
        }
        self.waiters.remove(index);
        self.guard_owner = Some(task);
        Ok(ConditionPoll::Cancelled)
    }

    pub fn unlock(&mut self, task: u32) -> Result<(), SyncModelError> {
        if self.guard_owner != Some(task) {
            return Err(SyncModelError::NotOwner);
        }
        self.guard_owner = None;
        Ok(())
    }

    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.waiters.len() > MAX_SYNC_TASKS {
            return Err("condition waiter limit exceeded".into());
        }
        let unique = self
            .waiters
            .iter()
            .map(|waiter| waiter.task)
            .collect::<BTreeSet<_>>();
        if unique.len() != self.waiters.len() {
            return Err("condition registered a task twice".into());
        }
        if self
            .guard_owner
            .is_some_and(|owner| unique.contains(&owner))
        {
            return Err("condition guard owner remained registered".into());
        }
        Ok(())
    }
}

impl Default for ConditionModel {
    fn default() -> Self {
        Self::new()
    }
}

/// A permit identity.  Copying this token in a test is useful for exercising
/// the double-release negative case; the model still rejects the second use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PermitToken {
    pub id: u64,
    pub owner: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemaphorePoll {
    Acquired(PermitToken),
    Waiting,
}

/// Fixed-capacity FIFO semaphore model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemaphoreModel {
    capacity: usize,
    permits: usize,
    next_permit: u64,
    active: BTreeMap<u64, PermitToken>,
    waiters: VecDeque<u32>,
    cleanup_runs: usize,
}

impl SemaphoreModel {
    pub fn new(capacity: usize) -> Result<Self, SyncModelError> {
        if capacity == 0 || capacity > MAX_SYNC_TASKS {
            return Err(SyncModelError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            permits: capacity,
            next_permit: 1,
            active: BTreeMap::new(),
            waiters: VecDeque::new(),
            cleanup_runs: 0,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn permits(&self) -> usize {
        self.permits
    }

    pub fn waiting(&self) -> usize {
        self.waiters.len()
    }

    pub fn cleanup_runs(&self) -> usize {
        self.cleanup_runs
    }

    fn allocate(&mut self, owner: u32) -> Result<PermitToken, SyncModelError> {
        let id = self.next_permit;
        self.next_permit = self
            .next_permit
            .checked_add(1)
            .ok_or(SyncModelError::Invariant)?;
        let token = PermitToken { id, owner };
        self.active.insert(id, token);
        Ok(token)
    }

    pub fn try_acquire(&mut self, task: u32) -> Result<Option<PermitToken>, SyncModelError> {
        if self.permits == 0 || !self.waiters.is_empty() {
            return Ok(None);
        }
        self.permits -= 1;
        self.allocate(task).map(Some)
    }

    pub fn acquire(&mut self, task: u32) -> Result<SemaphorePoll, SyncModelError> {
        if let Some(token) = self.try_acquire(task)? {
            return Ok(SemaphorePoll::Acquired(token));
        }
        if self.waiters.contains(&task) {
            return Err(SyncModelError::AlreadyHeld);
        }
        self.waiters.push_back(task);
        Ok(SemaphorePoll::Waiting)
    }

    /// Release exactly one permit and hand it directly to the oldest waiter.
    pub fn release(&mut self, token: PermitToken) -> Result<Option<PermitToken>, SyncModelError> {
        let Some(active) = self.active.get(&token.id).copied() else {
            return Err(SyncModelError::AlreadyReleased);
        };
        if active != token {
            return Err(SyncModelError::NotOwner);
        }
        self.active.remove(&token.id);
        self.cleanup_runs = self.cleanup_runs.saturating_add(1);
        if let Some(task) = self.waiters.pop_front() {
            let replacement = self.allocate(task)?;
            Ok(Some(replacement))
        } else {
            self.permits = self.permits.saturating_add(1);
            Ok(None)
        }
    }

    pub fn cancel_wait(&mut self, task: u32) -> Result<(), SyncModelError> {
        let Some(index) = self.waiters.iter().position(|candidate| *candidate == task) else {
            return Err(SyncModelError::NotWaiting);
        };
        self.waiters.remove(index);
        Ok(())
    }

    pub fn active_tokens(&self) -> Vec<PermitToken> {
        self.active.values().copied().collect()
    }

    pub fn cleanup_all(&mut self) -> Result<(), SyncModelError> {
        let tokens = self.active_tokens();
        for token in tokens {
            self.release(token)?;
        }
        self.waiters.clear();
        Ok(())
    }

    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.capacity == 0 || self.permits > self.capacity {
            return Err("semaphore capacity invariant failed".into());
        }
        if self.active.len() + self.permits > self.capacity {
            return Err("semaphore created more units than its capacity".into());
        }
        let unique = self.waiters.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != self.waiters.len() {
            return Err("semaphore registered a task twice".into());
        }
        Ok(())
    }
}

/// A bounded result produced by an initializer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnceResolution<T, E> {
    Success(T),
    Error(E),
    Panic,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnceStart<T> {
    Ready(T),
    Initializer,
    Waiting,
    Reentrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OnceState<T> {
    Uninitialized,
    Initializing { owner: u32 },
    Ready(T),
}

/// Once model with retry-on-error/panic/cancellation and FIFO waiter wakeups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnceModel<T, E> {
    state: OnceState<T>,
    waiters: VecDeque<u32>,
    wakeups: usize,
    cleanup_runs: usize,
    _error: std::marker::PhantomData<E>,
}

impl<T, E> OnceModel<T, E>
where
    T: Clone,
{
    pub fn new() -> Self {
        Self {
            state: OnceState::Uninitialized,
            waiters: VecDeque::new(),
            wakeups: 0,
            cleanup_runs: 0,
            _error: std::marker::PhantomData,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.state, OnceState::Ready(_))
    }

    pub fn get(&self) -> Option<T> {
        match &self.state {
            OnceState::Ready(value) => Some(value.clone()),
            OnceState::Uninitialized | OnceState::Initializing { .. } => None,
        }
    }

    pub fn waiting(&self) -> usize {
        self.waiters.len()
    }

    pub fn wakeups(&self) -> usize {
        self.wakeups
    }

    pub fn cleanup_runs(&self) -> usize {
        self.cleanup_runs
    }

    pub fn start(&mut self, task: u32) -> OnceStart<T> {
        match &self.state {
            OnceState::Ready(value) => OnceStart::Ready(value.clone()),
            OnceState::Initializing { owner } if *owner == task => OnceStart::Reentrant,
            OnceState::Initializing { .. } => {
                if !self.waiters.contains(&task) {
                    self.waiters.push_back(task);
                }
                OnceStart::Waiting
            }
            OnceState::Uninitialized => {
                self.state = OnceState::Initializing { owner: task };
                OnceStart::Initializer
            }
        }
    }

    pub fn finish(
        &mut self,
        task: u32,
        resolution: OnceResolution<T, E>,
    ) -> Result<Vec<u32>, SyncModelError> {
        let OnceState::Initializing { owner } = self.state else {
            return Err(SyncModelError::NotOwner);
        };
        if owner != task {
            return Err(SyncModelError::NotOwner);
        }
        self.cleanup_runs = self.cleanup_runs.saturating_add(1);
        self.state = match resolution {
            OnceResolution::Success(value) => OnceState::Ready(value),
            OnceResolution::Error(_) | OnceResolution::Panic | OnceResolution::Cancelled => {
                OnceState::Uninitialized
            }
        };
        let wakeups = self.waiters.drain(..).collect::<Vec<_>>();
        self.wakeups = self.wakeups.saturating_add(wakeups.len());
        Ok(wakeups)
    }

    pub fn cancel_initializer(&mut self, task: u32) -> Result<Vec<u32>, SyncModelError> {
        self.finish(task, OnceResolution::Cancelled)
    }

    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.waiters.len() > MAX_SYNC_TASKS {
            return Err("Once waiter limit exceeded".into());
        }
        let unique = self.waiters.iter().copied().collect::<BTreeSet<_>>();
        if unique.len() != self.waiters.len() {
            return Err("Once registered a task twice".into());
        }
        if let OnceState::Initializing { owner } = self.state
            && self.waiters.contains(&owner)
        {
            return Err("Once initializer remained in its waiter queue".into());
        }
        if self.is_ready() && !self.waiters.is_empty() {
            return Err("ready Once retained waiters".into());
        }
        Ok(())
    }
}

impl<T, E> Default for OnceModel<T, E>
where
    T: Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

/// The public role returned by a completed barrier generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierRole {
    Leader,
    Follower,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarrierPoll {
    Waiting { generation: u64 },
    Complete { generation: u64, role: BarrierRole },
    Broken,
}

/// Reusable generation barrier.  A broken generation wakes all participants;
/// the next arrival starts a fresh generation instead of inheriting arrivals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarrierModel {
    parties: usize,
    generation: u64,
    arrivals: BTreeSet<u32>,
    released: VecDeque<(u32, BarrierPoll)>,
    broken: bool,
}

impl BarrierModel {
    pub fn new(parties: usize) -> Result<Self, SyncModelError> {
        if parties == 0 || parties > MAX_SYNC_TASKS {
            return Err(SyncModelError::InvalidParties);
        }
        Ok(Self {
            parties,
            generation: 0,
            arrivals: BTreeSet::new(),
            released: VecDeque::new(),
            broken: false,
        })
    }

    pub fn parties(&self) -> usize {
        self.parties
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn waiting(&self) -> usize {
        self.arrivals.len()
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn arrive(&mut self, task: u32) -> Result<BarrierPoll, SyncModelError> {
        if self.broken {
            self.broken = false;
            self.arrivals.clear();
            self.released.clear();
            self.generation = self.generation.saturating_add(1);
        }
        if !self.arrivals.insert(task) {
            return Err(SyncModelError::AlreadyHeld);
        }
        if self.arrivals.len() < self.parties {
            return Ok(BarrierPoll::Waiting {
                generation: self.generation,
            });
        }
        let generation = self.generation;
        self.arrivals.remove(&task);
        let followers = std::mem::take(&mut self.arrivals);
        for participant in followers {
            self.released.push_back((
                participant,
                BarrierPoll::Complete {
                    generation,
                    role: BarrierRole::Follower,
                },
            ));
        }
        self.released.push_back((
            task,
            BarrierPoll::Complete {
                generation,
                role: BarrierRole::Leader,
            },
        ));
        self.generation = self.generation.saturating_add(1);
        Ok(BarrierPoll::Complete {
            generation,
            role: BarrierRole::Leader,
        })
    }

    pub fn cancel(&mut self, task: u32) -> Result<usize, SyncModelError> {
        if !self.arrivals.remove(&task) {
            return Err(SyncModelError::NotWaiting);
        }
        let count = self.arrivals.len() + 1;
        let participants = std::mem::take(&mut self.arrivals);
        self.released.extend(
            participants
                .into_iter()
                .map(|participant| (participant, BarrierPoll::Broken)),
        );
        self.released.push_back((task, BarrierPoll::Broken));
        self.broken = true;
        self.generation = self.generation.saturating_add(1);
        Ok(count)
    }

    pub fn take_released(&mut self) -> Vec<(u32, BarrierPoll)> {
        self.released.drain(..).collect()
    }

    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.parties == 0 || self.arrivals.len() >= self.parties {
            return Err("barrier generation has an invalid arrival count".into());
        }
        if self.broken && !self.arrivals.is_empty() {
            return Err("broken barrier retained arrivals".into());
        }
        Ok(())
    }
}

/// Outcomes of a release/acquire publication litmus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationOutcome {
    Initial,
    Published,
    StaleAfterFlag,
}

/// Check a two-variable publication litmus without pretending that Relaxed
/// provides a happens-before edge.  `Initial` is always possible; a stale read
/// after observing the release flag is forbidden only when orders synchronize.
pub const fn publication_outcome_allowed(
    writer: MemoryOrder,
    reader: MemoryOrder,
    outcome: PublicationOutcome,
) -> bool {
    match outcome {
        PublicationOutcome::Initial | PublicationOutcome::Published => true,
        PublicationOutcome::StaleAfterFlag => !MemoryOrder::synchronizes_with(writer, reader),
    }
}

/// Compact result from an adversarial bounded sync run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFuzzSummary {
    pub steps: usize,
    pub accepted_operations: usize,
    pub rejected_operations: usize,
    pub pending_waiters: usize,
    pub cleanup_runs: usize,
    pub wakeups: usize,
    pub state_hash: u64,
}

/// Run deterministic operations against every synchronization model.  Invalid
/// transitions are counted, not hidden; invariants are checked after every
/// step and all live resources are explicitly torn down before returning.
pub fn run_fuzz_case(input: &[u8]) -> Result<SyncFuzzSummary, String> {
    let input = &input[..input.len().min(MAX_FUZZ_INPUT_BYTES)];
    let steps = input.len().clamp(1, MAX_FUZZ_STEPS);
    let input_len = input.len().max(1);
    let mut mutex = MutexModel::new();
    let mut condition = ConditionModel::new();
    let mut semaphore = SemaphoreModel::new(3).map_err(|error| format!("{error:?}"))?;
    let mut once = OnceModel::<i64, &'static str>::new();
    let mut barrier = BarrierModel::new(3).map_err(|error| format!("{error:?}"))?;
    let mut active_once_owner = None;
    let mut accepted_operations = 0;
    let mut rejected_operations = 0;
    let mut state_hash = 0xcbf2_9ce4_8422_2325_u64;

    for step in 0..steps {
        let byte = input.get(step % input_len).copied().unwrap_or_default();
        let argument = input
            .get((step + 1) % input_len)
            .copied()
            .unwrap_or_default();
        let task = u32::from(argument % 8);
        let operation = byte % 16;
        let result = match operation {
            0 => mutex.lock(task).map(|_| ()),
            1 => mutex.unlock(task).map(|_| ()),
            2 => mutex.cancel_wait(task),
            3 => semaphore.acquire(task).map(|_| ()),
            4 => semaphore
                .active_tokens()
                .first()
                .copied()
                .ok_or(SyncModelError::NotWaiting)
                .and_then(|token| semaphore.release(token).map(|_| ())),
            5 => semaphore.cancel_wait(task),
            6 => match once.start(task) {
                OnceStart::Initializer => {
                    active_once_owner = Some(task);
                    Ok(())
                }
                OnceStart::Ready(_) | OnceStart::Waiting | OnceStart::Reentrant => Ok(()),
            },
            7 => match active_once_owner {
                Some(owner) => {
                    let resolution = match argument % 4 {
                        0 => OnceResolution::Success(i64::from(argument)),
                        1 => OnceResolution::Error("retry"),
                        2 => OnceResolution::Panic,
                        _ => OnceResolution::Cancelled,
                    };
                    let result = once.finish(owner, resolution).map(|_| ());
                    if result.is_ok() {
                        active_once_owner = None;
                    }
                    result
                }
                None => Err(SyncModelError::NotOwner),
            },
            8 => {
                let locked = if condition.guard_owner().is_none() {
                    condition.lock_for(task)
                } else {
                    Ok(())
                };
                locked.and_then(|_| condition.wait(task)).map(|_| ())
            }
            9 => condition
                .notify_one()
                .ok_or(SyncModelError::NotWaiting)
                .map(|_| ()),
            10 => condition.reacquire(task).map(|_| ()).or_else(|error| {
                (error == SyncModelError::NotWaiting)
                    .then_some(())
                    .ok_or(error)
            }),
            11 => barrier.arrive(task).map(|_| ()),
            12 => barrier.cancel(task).map(|_| ()),
            13 => {
                let writer = MemoryOrder::ALL[usize::from(argument) % MemoryOrder::ALL.len()];
                let reader = MemoryOrder::ALL[usize::from(byte) % MemoryOrder::ALL.len()];
                MemoryOrder::valid_compare_exchange(writer, reader)
                    .then_some(())
                    .ok_or(SyncModelError::InvalidOrder)
            }
            14 => {
                let mut result = mutex.cleanup_owner();
                if result.is_ok() {
                    for token in semaphore.active_tokens() {
                        result = semaphore.release(token).map(|_| ());
                        if result.is_err() {
                            break;
                        }
                    }
                }
                if result.is_ok()
                    && let Some(owner) = active_once_owner.take()
                {
                    result = once.cancel_initializer(owner).map(|_| ());
                }
                result
            }
            _ => Ok(()),
        };
        if result.is_ok() {
            accepted_operations += 1;
        } else {
            rejected_operations += 1;
        }
        mutex.assert_invariants()?;
        condition.assert_invariants()?;
        semaphore.assert_invariants()?;
        once.assert_invariants()?;
        barrier.assert_invariants()?;
        state_hash = state_hash
            .wrapping_mul(0x1000_0000_01b3)
            .wrapping_add(u64::from(byte) ^ (u64::from(argument) << 8));
    }

    mutex
        .cleanup_owner()
        .map_err(|error| format!("{error:?}"))?;
    for task in (0..MAX_SYNC_TASKS as u32).collect::<Vec<_>>() {
        let _ = mutex.cancel_wait(task);
        let _ = semaphore.cancel_wait(task);
        if condition.guard_owner() == Some(task) {
            let _ = condition.unlock(task);
        }
        let _ = condition.cancel_wait(task);
        let _ = barrier.cancel(task);
    }
    for token in semaphore.active_tokens() {
        semaphore
            .release(token)
            .map_err(|error| format!("{error:?}"))?;
    }
    if let Some(owner) = active_once_owner {
        once.cancel_initializer(owner)
            .map_err(|error| format!("{error:?}"))?;
    }
    mutex.assert_invariants()?;
    condition.assert_invariants()?;
    semaphore.assert_invariants()?;
    once.assert_invariants()?;
    barrier.assert_invariants()?;
    let pending_waiters = mutex.waiters.len()
        + condition.waiters.len()
        + semaphore.waiters.len()
        + once.waiters.len()
        + barrier.arrivals.len();
    if pending_waiters != 0 {
        return Err("sync teardown retained a waiter".into());
    }
    let cleanup_runs = mutex
        .cleanup_runs
        .values()
        .map(|runs| usize::from(*runs))
        .sum::<usize>()
        + semaphore.cleanup_runs
        + once.cleanup_runs;
    let wakeups = mutex.wakeups + condition.wakeups + semaphore.cleanup_runs + once.wakeups;
    Ok(SyncFuzzSummary {
        steps,
        accepted_operations,
        rejected_operations,
        pending_waiters,
        cleanup_runs,
        wakeups,
        state_hash,
    })
}
