//! Independent bounded reference model for `std.executor`.
//!
//! The VM and native bridge own the production implementation.  This model
//! keeps only small scalar payloads and ordinary collections so scheduling,
//! admission, lifecycle, actor, and cleanup laws can be replayed without
//! reusing runtime state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum workers accepted by one model run.
pub const MAX_WORKERS: usize = 8;
/// Maximum explicit queue capacity accepted by one model run.
pub const MAX_CAPACITY: usize = 16;
/// Maximum jobs retained by one model run.
pub const MAX_JOBS: usize = 64;
/// Maximum actor mailbox capacity accepted by one model run.
pub const MAX_ACTOR_MESSAGES: usize = 64;
/// Maximum bytes consumed by one fuzz input.
pub const MAX_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum state transitions in one fuzz input.
pub const MAX_FUZZ_STEPS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Open,
    ShuttingDown,
    Cancelling,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    Success(u8),
    Error(u8),
    Panic(u8),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Queued,
    Running { worker: usize },
    CancellationRequested { worker: usize },
    Terminal(JobOutcome),
    Consumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Job {
    payload: u8,
    state: JobState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitResult {
    Accepted(usize),
    Saturated,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorSendResult {
    Accepted,
    Saturated(u8),
    Closed(u8),
    Cancelled(u8),
    Terminated(u8),
    ResourceLimit(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorOutcome {
    Success,
    Error(u8),
    Panic(u8),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelError {
    InvalidWorkers,
    InvalidCapacity,
    ResourceLimit,
    InvalidLifecycle,
    UnknownJob,
    InvalidTransition,
    Race,
    ActorMissing,
    ActorExists,
    ActorLimit,
    ActorNotReady,
    Invariant(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorSnapshot {
    pub capacity: usize,
    pub lifecycle: String,
    pub mailbox: usize,
    pub in_flight: bool,
    pub processed: Vec<u8>,
    pub discarded: usize,
    pub cleanup_runs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorSnapshot {
    pub lifecycle: Lifecycle,
    pub workers: usize,
    pub capacity: usize,
    pub queued: usize,
    pub running: usize,
    pub terminal: usize,
    pub consumed: usize,
    pub max_running: usize,
    pub admitted_order: Vec<usize>,
    pub completion_order: Vec<usize>,
    pub worker_starts: Vec<usize>,
    pub actor: Option<ActorSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ActorLifecycle {
    Running,
    Stopping,
    Terminated(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActorModel {
    capacity: usize,
    lifecycle: ActorLifecycle,
    mailbox: VecDeque<u8>,
    in_flight: Option<u8>,
    processed: Vec<u8>,
    discarded: usize,
    cleanup_runs: usize,
}

impl ActorModel {
    fn new(capacity: usize) -> Result<Self, ModelError> {
        if capacity > MAX_ACTOR_MESSAGES {
            return Err(ModelError::ActorLimit);
        }
        Ok(Self {
            capacity,
            lifecycle: ActorLifecycle::Running,
            mailbox: VecDeque::new(),
            in_flight: None,
            processed: Vec::new(),
            discarded: 0,
            cleanup_runs: 0,
        })
    }

    fn send(&mut self, payload: u8) -> ActorSendResult {
        match self.lifecycle {
            ActorLifecycle::Running => {
                if self.mailbox.len() >= MAX_ACTOR_MESSAGES {
                    return ActorSendResult::ResourceLimit(payload);
                }
                if self.mailbox.len() >= self.capacity {
                    return ActorSendResult::Saturated(payload);
                }
                self.mailbox.push_back(payload);
                ActorSendResult::Accepted
            }
            ActorLifecycle::Stopping => ActorSendResult::Closed(payload),
            ActorLifecycle::Terminated("cancelled") => ActorSendResult::Cancelled(payload),
            ActorLifecycle::Terminated(_) => ActorSendResult::Terminated(payload),
        }
    }

    fn start(&mut self) -> Result<Option<u8>, ModelError> {
        if !matches!(self.lifecycle, ActorLifecycle::Running) {
            return Err(ModelError::ActorNotReady);
        }
        if self.in_flight.is_some() {
            return Err(ModelError::InvalidTransition);
        }
        Ok(self.mailbox.pop_front().inspect(|payload| {
            self.in_flight = Some(*payload);
        }))
    }

    fn finish(&mut self, outcome: ActorOutcome) -> Result<(), ModelError> {
        let stopping = matches!(self.lifecycle, ActorLifecycle::Stopping);
        if matches!(outcome, ActorOutcome::Success)
            && !matches!(self.lifecycle, ActorLifecycle::Running)
        {
            return Err(ModelError::InvalidTransition);
        }
        let payload = self.in_flight.take().ok_or(ModelError::ActorNotReady)?;
        self.cleanup_runs = self.cleanup_runs.saturating_add(1);
        match outcome {
            ActorOutcome::Success => {
                self.processed.push(payload);
            }
            ActorOutcome::Error(_) | ActorOutcome::Panic(_) => {
                self.discarded = self.discarded.saturating_add(1);
                self.discard_mailbox();
                self.lifecycle = ActorLifecycle::Terminated("failed");
            }
            ActorOutcome::Cancelled => {
                self.discarded = self.discarded.saturating_add(1);
                self.discard_mailbox();
                self.lifecycle =
                    ActorLifecycle::Terminated(if stopping { "stopped" } else { "cancelled" });
            }
        }
        if matches!(self.lifecycle, ActorLifecycle::Stopping) {
            self.lifecycle = ActorLifecycle::Terminated("stopped");
        }
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), ModelError> {
        if !matches!(self.lifecycle, ActorLifecycle::Running) {
            return Err(ModelError::InvalidTransition);
        }
        if self.in_flight.take().is_some() {
            self.cleanup_runs = self.cleanup_runs.saturating_add(1);
            self.discarded = self.discarded.saturating_add(1);
        }
        self.discard_mailbox();
        self.lifecycle = ActorLifecycle::Terminated("cancelled");
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ModelError> {
        if !matches!(self.lifecycle, ActorLifecycle::Running) {
            return Err(ModelError::InvalidTransition);
        }
        self.lifecycle = ActorLifecycle::Stopping;
        self.discard_mailbox();
        if self.in_flight.is_none() {
            self.lifecycle = ActorLifecycle::Terminated("stopped");
        }
        Ok(())
    }

    fn discard_mailbox(&mut self) {
        let discarded = self.mailbox.len();
        self.mailbox.clear();
        self.discarded = self.discarded.saturating_add(discarded);
        self.cleanup_runs = self.cleanup_runs.saturating_add(discarded);
    }

    fn snapshot(&self) -> ActorSnapshot {
        let lifecycle = match self.lifecycle {
            ActorLifecycle::Running => "running",
            ActorLifecycle::Stopping => "stopping",
            ActorLifecycle::Terminated(reason) => reason,
        };
        ActorSnapshot {
            capacity: self.capacity,
            lifecycle: lifecycle.to_owned(),
            mailbox: self.mailbox.len(),
            in_flight: self.in_flight.is_some(),
            processed: self.processed.clone(),
            discarded: self.discarded,
            cleanup_runs: self.cleanup_runs,
        }
    }

    fn assert_invariants(&self) -> Result<(), ModelError> {
        if self.mailbox.len() > self.capacity {
            return Err(ModelError::Invariant("actor mailbox exceeded capacity"));
        }
        if matches!(self.lifecycle, ActorLifecycle::Terminated(_))
            && (self.in_flight.is_some() || !self.mailbox.is_empty())
        {
            return Err(ModelError::Invariant("terminated actor retained work"));
        }
        if self.cleanup_runs != self.processed.len() + self.discarded {
            return Err(ModelError::Invariant("actor cleanup count diverged"));
        }
        Ok(())
    }
}

/// A bounded executor model with explicit worker slots and one serialized
/// actor mailbox.  It is intentionally independent from VM handles and heaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorModel {
    workers: usize,
    capacity: usize,
    lifecycle: Lifecycle,
    jobs: BTreeMap<usize, Job>,
    queue: VecDeque<usize>,
    worker_jobs: Vec<Option<usize>>,
    worker_cursor: usize,
    worker_starts: Vec<usize>,
    next_job: usize,
    admitted_order: Vec<usize>,
    completion_order: Vec<usize>,
    max_running: usize,
    actor: Option<ActorModel>,
}

impl ExecutorModel {
    pub fn new(workers: i64, capacity: i64) -> Result<Self, ModelError> {
        let workers = usize::try_from(workers).map_err(|_| ModelError::InvalidWorkers)?;
        let capacity = usize::try_from(capacity).map_err(|_| ModelError::InvalidCapacity)?;
        if workers == 0 || workers > MAX_WORKERS {
            return Err(ModelError::InvalidWorkers);
        }
        if capacity > MAX_CAPACITY {
            return Err(ModelError::InvalidCapacity);
        }
        Ok(Self {
            workers,
            capacity,
            lifecycle: Lifecycle::Open,
            jobs: BTreeMap::new(),
            queue: VecDeque::new(),
            worker_jobs: vec![None; workers],
            worker_cursor: 0,
            worker_starts: vec![0; workers],
            next_job: 1,
            admitted_order: Vec::new(),
            completion_order: Vec::new(),
            max_running: 0,
            actor: None,
        })
    }

    fn admission_limit(&self) -> usize {
        if self.capacity == 0 {
            self.workers
        } else {
            self.capacity
        }
    }

    fn running_count(&self) -> usize {
        self.worker_jobs.iter().filter(|job| job.is_some()).count()
    }

    fn admitted_count(&self) -> usize {
        self.queue.len() + self.running_count()
    }

    pub fn try_submit(&mut self, payload: u8) -> Result<SubmitResult, ModelError> {
        let terminal = match self.lifecycle {
            Lifecycle::Open => None,
            Lifecycle::ShuttingDown | Lifecycle::Closed => Some(SubmitResult::Closed),
            Lifecycle::Cancelling | Lifecycle::Cancelled => Some(SubmitResult::Cancelled),
        };
        if let Some(result) = terminal {
            return Ok(result);
        }
        if self.admitted_count() >= self.admission_limit() {
            return Ok(SubmitResult::Saturated);
        }
        let id = self.next_job;
        self.next_job = self
            .next_job
            .checked_add(1)
            .ok_or(ModelError::ResourceLimit)?;
        self.jobs.insert(
            id,
            Job {
                payload,
                state: JobState::Queued,
            },
        );
        self.queue.push_back(id);
        self.admitted_order.push(id);
        self.schedule();
        Ok(SubmitResult::Accepted(id))
    }

    /// Model the backpressured `submit`: a saturated call waits for one
    /// deterministic worker turn, retaining the caller's payload until commit.
    pub fn submit(&mut self, payload: u8) -> Result<SubmitResult, ModelError> {
        for _ in 0..MAX_JOBS {
            match self.try_submit(payload)? {
                SubmitResult::Saturated => {
                    if self.running_count() == 0 {
                        return Err(ModelError::InvalidTransition);
                    }
                    self.advance(JobOutcome::Success(0))?;
                }
                result => return Ok(result),
            }
        }
        Err(ModelError::ResourceLimit)
    }

    fn schedule(&mut self) {
        while let Some(job_id) = self.queue.front().copied() {
            let Some(worker) = (0..self.workers)
                .map(|offset| (self.worker_cursor + offset) % self.workers)
                .find(|worker| self.worker_jobs[*worker].is_none())
            else {
                break;
            };
            self.queue.pop_front();
            self.worker_jobs[worker] = Some(job_id);
            self.worker_starts[worker] = self.worker_starts[worker].saturating_add(1);
            self.worker_cursor = (worker + 1) % self.workers;
            if let Some(job) = self.jobs.get_mut(&job_id) {
                job.state = JobState::Running { worker };
            }
        }
        self.max_running = self.max_running.max(self.running_count());
    }

    pub fn complete(&mut self, id: usize, outcome: JobOutcome) -> Result<(), ModelError> {
        let state = self.jobs.get(&id).ok_or(ModelError::UnknownJob)?.state;
        let worker = match state {
            JobState::Running { worker } | JobState::CancellationRequested { worker } => worker,
            JobState::Queued => return Err(ModelError::InvalidTransition),
            JobState::Terminal(_) | JobState::Consumed => return Err(ModelError::Race),
        };
        if self.worker_jobs.get(worker).copied().flatten() != Some(id) {
            return Err(ModelError::Invariant("worker slot lost its job"));
        }
        self.worker_jobs[worker] = None;
        let outcome = if matches!(state, JobState::CancellationRequested { .. }) {
            JobOutcome::Cancelled
        } else {
            outcome
        };
        self.jobs.get_mut(&id).expect("job was checked above").state = JobState::Terminal(outcome);
        self.completion_order.push(id);
        self.schedule();
        self.finish_lifecycle();
        Ok(())
    }

    pub fn advance(&mut self, outcome: JobOutcome) -> Result<Option<usize>, ModelError> {
        let Some(id) = (0..self.workers)
            .map(|offset| (self.worker_cursor + offset) % self.workers)
            .find_map(|worker| self.worker_jobs[worker])
        else {
            self.finish_lifecycle();
            return Ok(None);
        };
        self.complete(id, outcome)?;
        Ok(Some(id))
    }

    pub fn cancel_job(&mut self, id: usize) -> Result<(), ModelError> {
        let state = self.jobs.get(&id).ok_or(ModelError::UnknownJob)?.state;
        match state {
            JobState::Queued => {
                let position = self
                    .queue
                    .iter()
                    .position(|queued| *queued == id)
                    .ok_or(ModelError::Invariant("queued job missing from queue"))?;
                self.queue.remove(position);
                self.jobs.get_mut(&id).expect("job was checked above").state =
                    JobState::Terminal(JobOutcome::Cancelled);
                self.completion_order.push(id);
                self.finish_lifecycle();
                Ok(())
            }
            JobState::Running { worker } => {
                if self.worker_jobs.get(worker).copied().flatten() != Some(id) {
                    return Err(ModelError::Invariant("running job lost its worker"));
                }
                self.jobs.get_mut(&id).expect("job was checked above").state =
                    JobState::CancellationRequested { worker };
                Ok(())
            }
            JobState::CancellationRequested { .. } => Err(ModelError::Race),
            JobState::Terminal(_) | JobState::Consumed => Err(ModelError::Race),
        }
    }

    pub fn shutdown(&mut self) -> Result<(), ModelError> {
        if !matches!(self.lifecycle, Lifecycle::Open) {
            return Err(ModelError::InvalidLifecycle);
        }
        self.lifecycle = Lifecycle::ShuttingDown;
        self.finish_lifecycle();
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), ModelError> {
        if !matches!(self.lifecycle, Lifecycle::Open | Lifecycle::ShuttingDown) {
            return Err(ModelError::InvalidLifecycle);
        }
        self.lifecycle = Lifecycle::Cancelling;
        let queued = self.queue.drain(..).collect::<Vec<_>>();
        for id in queued {
            if let Some(job) = self.jobs.get_mut(&id) {
                job.state = JobState::Terminal(JobOutcome::Cancelled);
                self.completion_order.push(id);
            }
        }
        for (worker, id) in self.worker_jobs.iter().copied().enumerate() {
            if let Some(id) = id
                && let Some(job) = self.jobs.get_mut(&id)
            {
                job.state = JobState::CancellationRequested { worker };
            }
        }
        self.finish_lifecycle();
        Ok(())
    }

    fn finish_lifecycle(&mut self) {
        if self.running_count() != 0 || !self.queue.is_empty() {
            return;
        }
        self.lifecycle = match self.lifecycle {
            Lifecycle::ShuttingDown => Lifecycle::Closed,
            Lifecycle::Cancelling => Lifecycle::Cancelled,
            lifecycle => lifecycle,
        };
    }

    pub fn consume(&mut self, id: usize) -> Result<JobOutcome, ModelError> {
        let job = self.jobs.get_mut(&id).ok_or(ModelError::UnknownJob)?;
        let JobState::Terminal(outcome) = job.state else {
            return Err(match job.state {
                JobState::Consumed => ModelError::Race,
                _ => ModelError::InvalidTransition,
            });
        };
        job.state = JobState::Consumed;
        Ok(outcome)
    }

    pub fn create_actor(&mut self, capacity: i64) -> Result<(), ModelError> {
        if self.actor.is_some() {
            return Err(ModelError::ActorExists);
        }
        let capacity = usize::try_from(capacity).map_err(|_| ModelError::ActorLimit)?;
        self.actor = Some(ActorModel::new(capacity)?);
        Ok(())
    }

    pub fn actor_send(&mut self, payload: u8) -> Result<ActorSendResult, ModelError> {
        self.actor
            .as_mut()
            .ok_or(ModelError::ActorMissing)
            .map(|actor| actor.send(payload))
    }

    pub fn actor_start(&mut self) -> Result<Option<u8>, ModelError> {
        self.actor.as_mut().ok_or(ModelError::ActorMissing)?.start()
    }

    pub fn actor_finish(&mut self, outcome: ActorOutcome) -> Result<(), ModelError> {
        self.actor
            .as_mut()
            .ok_or(ModelError::ActorMissing)?
            .finish(outcome)
    }

    pub fn actor_stop(&mut self) -> Result<(), ModelError> {
        self.actor.as_mut().ok_or(ModelError::ActorMissing)?.stop()
    }

    pub fn actor_cancel(&mut self) -> Result<(), ModelError> {
        self.actor
            .as_mut()
            .ok_or(ModelError::ActorMissing)?
            .cancel()
    }

    pub fn snapshot(&self) -> ExecutorSnapshot {
        let (terminal, consumed) = self
            .jobs
            .values()
            .fold((0, 0), |(terminal, consumed), job| match job.state {
                JobState::Terminal(_) => (terminal + 1, consumed),
                JobState::Consumed => (terminal, consumed + 1),
                _ => (terminal, consumed),
            });
        ExecutorSnapshot {
            lifecycle: self.lifecycle,
            workers: self.workers,
            capacity: self.capacity,
            queued: self.queue.len(),
            running: self.running_count(),
            terminal,
            consumed,
            max_running: self.max_running,
            admitted_order: self.admitted_order.clone(),
            completion_order: self.completion_order.clone(),
            worker_starts: self.worker_starts.clone(),
            actor: self.actor.as_ref().map(ActorModel::snapshot),
        }
    }

    pub fn assert_invariants(&self) -> Result<(), ModelError> {
        let mut queued = BTreeSet::new();
        for id in &self.queue {
            if !queued.insert(*id) {
                return Err(ModelError::Invariant("duplicate queued job"));
            }
            if !matches!(
                self.jobs.get(id).map(|job| job.state),
                Some(JobState::Queued)
            ) {
                return Err(ModelError::Invariant("queue state mismatch"));
            }
        }
        let mut running = BTreeSet::new();
        for (worker, id) in self.worker_jobs.iter().enumerate() {
            if let Some(id) = id {
                if !running.insert(*id) {
                    return Err(ModelError::Invariant("job assigned to two workers"));
                }
                if !matches!(
                    self.jobs.get(id).map(|job| job.state),
                    Some(JobState::Running { worker: owner })
                        | Some(JobState::CancellationRequested { worker: owner })
                        if owner == worker
                ) {
                    return Err(ModelError::Invariant("worker state mismatch"));
                }
            }
        }
        if self.admitted_count() > self.admission_limit() {
            return Err(ModelError::Invariant("admission limit exceeded"));
        }
        if self.running_count() > self.workers || self.max_running < self.running_count() {
            return Err(ModelError::Invariant("running limit violated"));
        }
        if matches!(self.lifecycle, Lifecycle::Closed | Lifecycle::Cancelled)
            && (self.running_count() != 0 || !self.queue.is_empty())
        {
            return Err(ModelError::Invariant("terminal pool retained work"));
        }
        if let Some(actor) = &self.actor {
            actor.assert_invariants()?;
        }
        Ok(())
    }

    /// Finish all accepted work, consume every terminal owner, and verify the
    /// lifecycle is closed.  This is the cleanup oracle used by fuzz replay.
    pub fn finalize(&mut self) -> Result<(), ModelError> {
        if matches!(self.lifecycle, Lifecycle::Open) {
            self.shutdown()?;
        }
        if matches!(self.lifecycle, Lifecycle::ShuttingDown) {
            while self.running_count() != 0 || !self.queue.is_empty() {
                self.advance(JobOutcome::Success(0))?;
            }
        }
        if matches!(self.lifecycle, Lifecycle::Cancelling) {
            while self.running_count() != 0 {
                self.advance(JobOutcome::Cancelled)?;
            }
        }
        if self.running_count() != 0 || !self.queue.is_empty() {
            return Err(ModelError::Invariant("finalize left live work"));
        }
        if let Some(actor) = &mut self.actor {
            if matches!(actor.lifecycle, ActorLifecycle::Running) {
                actor.stop()?;
            }
            if actor.in_flight.is_some() {
                actor.finish(ActorOutcome::Cancelled)?;
            }
        }
        let terminal_ids = self
            .jobs
            .iter()
            .filter_map(|(id, job)| matches!(job.state, JobState::Terminal(_)).then_some(*id))
            .collect::<Vec<_>>();
        for id in terminal_ids {
            self.consume(id)?;
        }
        self.assert_invariants()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzSummary {
    pub steps: usize,
    pub snapshot: ExecutorSnapshot,
}

fn selected_job(model: &ExecutorModel, selector: u8) -> usize {
    if model.admitted_order.is_empty() {
        return 1;
    }
    model.admitted_order[usize::from(selector) % model.admitted_order.len()]
}

/// Replay a bounded byte sequence through executor and actor transitions.
/// Invalid user operations are expected negative cases; invariant failures
/// remain errors and never get converted into a successful fuzz observation.
pub fn run_fuzz_case(input: &[u8]) -> Result<FuzzSummary, ModelError> {
    let bytes = &input[..input.len().min(MAX_FUZZ_INPUT_BYTES)];
    let workers = i64::from(bytes.first().copied().unwrap_or(0) % 4) + 1;
    let capacity = i64::from(bytes.get(1).copied().unwrap_or(0) % 9);
    let mut model = ExecutorModel::new(workers, capacity)?;
    let mut steps = 0;
    for chunk in bytes.get(2..).unwrap_or_default().chunks(3) {
        if steps == MAX_FUZZ_STEPS {
            break;
        }
        let op = chunk[0] % 15;
        let selector = *chunk.get(1).unwrap_or(&0);
        let payload = *chunk.get(2).unwrap_or(&selector);
        let id = selected_job(&model, selector);
        let result = match op {
            0 => model.try_submit(payload).map(|_| ()),
            1 => model.submit(payload).map(|_| ()),
            2 => model.advance(JobOutcome::Success(payload)).map(|_| ()),
            3 => model.complete(id, JobOutcome::Success(payload)),
            4 => model.complete(id, JobOutcome::Error(payload)),
            5 => model.complete(id, JobOutcome::Panic(payload)),
            6 => model.cancel_job(id),
            7 => model.shutdown(),
            8 => model.cancel(),
            9 => model.create_actor(i64::from(payload % 8)),
            10 => model.actor_send(payload).map(|_| ()),
            11 => match model.actor_start() {
                Ok(Some(_)) => {
                    let outcome = match payload % 3 {
                        0 => ActorOutcome::Success,
                        1 => ActorOutcome::Error(payload),
                        _ => ActorOutcome::Panic(payload),
                    };
                    model.actor_finish(outcome)
                }
                Ok(None) | Err(ModelError::ActorMissing | ModelError::ActorNotReady) => Ok(()),
                Err(error) => Err(error),
            },
            12 => match model.actor_stop() {
                Err(ModelError::ActorMissing | ModelError::InvalidTransition) | Ok(()) => Ok(()),
                Err(error) => Err(error),
            },
            13 => match model.actor_cancel() {
                Err(ModelError::ActorMissing | ModelError::InvalidTransition) | Ok(()) => Ok(()),
                Err(error) => Err(error),
            },
            _ => model.consume(id).map(|_| ()),
        };
        if let Err(error @ ModelError::Invariant(_)) = result {
            return Err(error);
        }
        model.assert_invariants()?;
        steps += 1;
    }
    model.finalize()?;
    Ok(FuzzSummary {
        steps: steps.max(1),
        snapshot: model.snapshot(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_fifo_backpressure_and_round_robin_are_explicit() {
        assert_eq!(ExecutorModel::new(0, 1), Err(ModelError::InvalidWorkers));
        assert_eq!(ExecutorModel::new(1, -1), Err(ModelError::InvalidCapacity));
        assert_eq!(ExecutorModel::new(9, 1), Err(ModelError::InvalidWorkers));
        assert_eq!(ExecutorModel::new(1, 17), Err(ModelError::InvalidCapacity));

        let mut model = ExecutorModel::new(3, 5).unwrap();
        let mut accepted = Vec::new();
        for payload in 0..5 {
            accepted.push(match model.try_submit(payload).unwrap() {
                SubmitResult::Accepted(id) => id,
                result => panic!("expected admission, got {result:?}"),
            });
        }
        assert_eq!(model.try_submit(99).unwrap(), SubmitResult::Saturated);
        assert_eq!(accepted, vec![1, 2, 3, 4, 5]);
        model.assert_invariants().unwrap();
        while model.advance(JobOutcome::Success(7)).unwrap().is_some() {}
        assert_eq!(model.snapshot().max_running, 3);
        assert!(
            model
                .snapshot()
                .worker_starts
                .iter()
                .all(|count| *count > 0)
        );
        for id in accepted {
            assert!(matches!(model.consume(id), Ok(JobOutcome::Success(7))));
        }
        model.shutdown().unwrap();
        assert_eq!(model.try_submit(1).unwrap(), SubmitResult::Closed);
        assert_eq!(model.shutdown(), Err(ModelError::InvalidLifecycle));
        model.assert_invariants().unwrap();
    }

    #[test]
    fn submit_waits_without_losing_payload_and_cancel_drains() {
        let mut model = ExecutorModel::new(1, 1).unwrap();
        let first = match model.try_submit(11).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected first admission, got {result:?}"),
        };
        let second = match model.submit(22).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected backpressured admission, got {result:?}"),
        };
        assert_eq!(model.completion_order, vec![first]);
        assert_eq!(model.jobs.get(&second).map(|job| job.payload), Some(22));
        model.cancel().unwrap();
        model.advance(JobOutcome::Cancelled).unwrap();
        assert_eq!(model.snapshot().lifecycle, Lifecycle::Cancelled);
        assert_eq!(model.try_submit(33).unwrap(), SubmitResult::Cancelled);
        assert_eq!(model.consume(second), Ok(JobOutcome::Cancelled));
        assert_eq!(model.consume(first), Ok(JobOutcome::Success(0)));
        model.assert_invariants().unwrap();
    }

    #[test]
    fn cancellation_panics_and_races_are_terminal() {
        let mut model = ExecutorModel::new(2, 4).unwrap();
        let first = match model.try_submit(1).unwrap() {
            SubmitResult::Accepted(id) => id,
            _ => unreachable!(),
        };
        let second = match model.try_submit(2).unwrap() {
            SubmitResult::Accepted(id) => id,
            _ => unreachable!(),
        };
        let queued = match model.try_submit(3).unwrap() {
            SubmitResult::Accepted(id) => id,
            _ => unreachable!(),
        };
        model.complete(first, JobOutcome::Panic(9)).unwrap();
        assert_eq!(model.consume(first), Ok(JobOutcome::Panic(9)));
        assert_eq!(
            model.complete(first, JobOutcome::Success(0)),
            Err(ModelError::Race)
        );
        model.cancel_job(queued).unwrap();
        model.cancel_job(second).unwrap();
        assert_eq!(model.cancel_job(second), Err(ModelError::Race));
        model.complete(second, JobOutcome::Success(0)).unwrap();
        model.complete(queued, JobOutcome::Success(0)).unwrap();
        assert_eq!(model.consume(second), Ok(JobOutcome::Cancelled));
        assert_eq!(model.consume(queued), Ok(JobOutcome::Cancelled));
        model.shutdown().unwrap();
        assert_eq!(model.snapshot().lifecycle, Lifecycle::Closed);
        model.assert_invariants().unwrap();
    }

    #[test]
    fn actor_fifo_failure_stop_and_terminal_message_ownership_are_explicit() {
        let mut model = ExecutorModel::new(1, 2).unwrap();
        assert_eq!(model.actor_send(1), Err(ModelError::ActorMissing));
        model.create_actor(2).unwrap();
        assert_eq!(model.create_actor(2), Err(ModelError::ActorExists));
        assert_eq!(model.actor_send(1), Ok(ActorSendResult::Accepted));
        assert_eq!(model.actor_send(2), Ok(ActorSendResult::Accepted));
        assert_eq!(model.actor_send(3), Ok(ActorSendResult::Saturated(3)));
        assert_eq!(model.actor_start().unwrap(), Some(1));
        assert_eq!(model.actor_start(), Err(ModelError::InvalidTransition));
        model.actor_finish(ActorOutcome::Success).unwrap();
        assert_eq!(model.actor_start().unwrap(), Some(2));
        model.actor_finish(ActorOutcome::Error(7)).unwrap();
        assert_eq!(model.actor_send(4), Ok(ActorSendResult::Terminated(4)));
        assert_eq!(model.actor_stop(), Err(ModelError::InvalidTransition));

        let mut stopping = ExecutorModel::new(1, 1).unwrap();
        stopping.create_actor(1).unwrap();
        stopping.actor_send(8).unwrap();
        assert_eq!(stopping.actor_start().unwrap(), Some(8));
        stopping.actor_stop().unwrap();
        assert_eq!(stopping.actor_send(9), Ok(ActorSendResult::Closed(9)));
        stopping.actor_finish(ActorOutcome::Cancelled).unwrap();
        assert_eq!(stopping.snapshot().actor.unwrap().lifecycle, "stopped");

        let mut cancelled = ExecutorModel::new(1, 1).unwrap();
        cancelled.create_actor(1).unwrap();
        cancelled.actor_send(6).unwrap();
        cancelled.actor_cancel().unwrap();
        assert_eq!(cancelled.actor_send(7), Ok(ActorSendResult::Cancelled(7)));
        assert_eq!(cancelled.actor_cancel(), Err(ModelError::InvalidTransition));

        let mut stopped = ExecutorModel::new(1, 1).unwrap();
        stopped.create_actor(1).unwrap();
        stopped.actor_send(9).unwrap();
        stopped.actor_stop().unwrap();
        assert_eq!(stopped.actor_send(10), Ok(ActorSendResult::Terminated(10)));
        assert_eq!(stopped.actor_start(), Err(ModelError::ActorNotReady));
        let snapshot = stopped.snapshot().actor.unwrap();
        assert_eq!(snapshot.lifecycle, "stopped");
        assert_eq!(snapshot.discarded, 1);
        stopped.assert_invariants().unwrap();

        let mut resource_limited = ExecutorModel::new(1, 1).unwrap();
        resource_limited
            .create_actor(MAX_ACTOR_MESSAGES as i64)
            .unwrap();
        for payload in 0..MAX_ACTOR_MESSAGES {
            assert_eq!(
                resource_limited.actor_send(payload as u8),
                Ok(ActorSendResult::Accepted)
            );
        }
        assert_eq!(
            resource_limited.actor_send(255),
            Ok(ActorSendResult::ResourceLimit(255))
        );
        resource_limited.actor_stop().unwrap();
        resource_limited.assert_invariants().unwrap();
    }

    #[test]
    fn negative_transitions_and_invariant_failures_are_explicit() {
        assert_eq!(
            ActorModel::new(MAX_ACTOR_MESSAGES + 1),
            Err(ModelError::ActorLimit)
        );
        let mut actor = ActorModel::new(1).unwrap();
        assert_eq!(
            actor.finish(ActorOutcome::Success),
            Err(ModelError::ActorNotReady)
        );
        actor.send(1);
        assert_eq!(actor.start(), Ok(Some(1)));
        assert_eq!(actor.start(), Err(ModelError::InvalidTransition));
        assert_eq!(actor.cancel(), Ok(()));
        assert_eq!(actor.send(2), ActorSendResult::Cancelled(2));

        let mut stopping = ActorModel::new(1).unwrap();
        stopping.send(3);
        assert_eq!(stopping.start(), Ok(Some(3)));
        assert_eq!(stopping.stop(), Ok(()));
        assert_eq!(stopping.send(4), ActorSendResult::Closed(4));
        assert_eq!(stopping.finish(ActorOutcome::Cancelled), Ok(()));
        assert_eq!(stopping.snapshot().lifecycle, "stopped");

        let mut failed = ActorModel::new(1).unwrap();
        failed.lifecycle = ActorLifecycle::Terminated("failed");
        assert_eq!(failed.send(5), ActorSendResult::Terminated(5));
        assert_eq!(failed.start(), Err(ModelError::ActorNotReady));

        let mut stopping_success = ActorModel::new(1).unwrap();
        stopping_success.lifecycle = ActorLifecycle::Stopping;
        stopping_success.in_flight = Some(6);
        assert_eq!(
            stopping_success.finish(ActorOutcome::Success),
            Err(ModelError::InvalidTransition)
        );
        assert_eq!(stopping_success.finish(ActorOutcome::Cancelled), Ok(()));

        let mut mailbox = ActorModel::new(1).unwrap();
        mailbox.mailbox.push_back(7);
        mailbox.mailbox.push_back(8);
        assert_eq!(
            mailbox.assert_invariants(),
            Err(ModelError::Invariant("actor mailbox exceeded capacity"))
        );
        let mut retained = ActorModel::new(1).unwrap();
        retained.lifecycle = ActorLifecycle::Terminated("failed");
        retained.in_flight = Some(8);
        assert_eq!(
            retained.assert_invariants(),
            Err(ModelError::Invariant("terminated actor retained work"))
        );
        let mut cleanup = ActorModel::new(1).unwrap();
        cleanup.processed.push(9);
        assert_eq!(
            cleanup.assert_invariants(),
            Err(ModelError::Invariant("actor cleanup count diverged"))
        );

        let mut model = ExecutorModel::new(1, 1).unwrap();
        assert_eq!(model.advance(JobOutcome::Success(0)), Ok(None));
        assert_eq!(model.consume(99), Err(ModelError::UnknownJob));
        assert_eq!(model.actor_send(1), Err(ModelError::ActorMissing));
        assert_eq!(model.actor_start(), Err(ModelError::ActorMissing));
        assert_eq!(
            model.actor_finish(ActorOutcome::Success),
            Err(ModelError::ActorMissing)
        );
        assert_eq!(model.actor_stop(), Err(ModelError::ActorMissing));
        assert_eq!(model.actor_cancel(), Err(ModelError::ActorMissing));
        assert_eq!(model.create_actor(-1), Err(ModelError::ActorLimit));
        assert_eq!(
            model.create_actor((MAX_ACTOR_MESSAGES + 1) as i64),
            Err(ModelError::ActorLimit)
        );

        let mut overflow = ExecutorModel::new(1, 1).unwrap();
        overflow.next_job = usize::MAX;
        assert_eq!(overflow.try_submit(1), Err(ModelError::ResourceLimit));

        let mut saturated_without_worker = ExecutorModel::new(1, 1).unwrap();
        saturated_without_worker.queue.push_back(42);
        assert_eq!(
            saturated_without_worker.submit(1),
            Err(ModelError::InvalidTransition)
        );

        let mut queued = ExecutorModel::new(1, 2).unwrap();
        let running = match queued.try_submit(1).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected running job, got {result:?}"),
        };
        let queued_id = match queued.try_submit(2).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected queued job, got {result:?}"),
        };
        assert_eq!(
            queued.complete(queued_id, JobOutcome::Success(0)),
            Err(ModelError::InvalidTransition)
        );
        assert_eq!(queued.cancel_job(queued_id), Ok(()));
        assert_eq!(queued.consume(queued_id), Ok(JobOutcome::Cancelled));
        assert_eq!(queued.consume(running), Err(ModelError::InvalidTransition));

        let mut missing_queue = ExecutorModel::new(1, 1).unwrap();
        missing_queue.jobs.insert(
            7,
            Job {
                payload: 0,
                state: JobState::Queued,
            },
        );
        assert_eq!(
            missing_queue.cancel_job(7),
            Err(ModelError::Invariant("queued job missing from queue"))
        );

        let mut lost_worker = ExecutorModel::new(1, 1).unwrap();
        let lost_id = match lost_worker.try_submit(1).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected accepted job, got {result:?}"),
        };
        lost_worker.worker_jobs[0] = None;
        assert_eq!(
            lost_worker.cancel_job(lost_id),
            Err(ModelError::Invariant("running job lost its worker"))
        );
        assert_eq!(
            lost_worker.complete(lost_id, JobOutcome::Success(0)),
            Err(ModelError::Invariant("worker slot lost its job"))
        );

        let mut cancelled = ExecutorModel::new(1, 1).unwrap();
        let cancelled_id = match cancelled.try_submit(1).unwrap() {
            SubmitResult::Accepted(id) => id,
            result => panic!("expected accepted job, got {result:?}"),
        };
        cancelled.cancel_job(cancelled_id).unwrap();
        assert_eq!(cancelled.cancel_job(cancelled_id), Err(ModelError::Race));
        cancelled
            .complete(cancelled_id, JobOutcome::Success(0))
            .unwrap();
        assert_eq!(cancelled.consume(cancelled_id), Ok(JobOutcome::Cancelled));
        assert_eq!(cancelled.consume(cancelled_id), Err(ModelError::Race));
        cancelled.cancel().unwrap();
        assert_eq!(cancelled.shutdown(), Err(ModelError::InvalidLifecycle));

        let mut duplicate_queue = ExecutorModel::new(1, 2).unwrap();
        duplicate_queue.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Queued,
            },
        );
        duplicate_queue.queue.extend([1, 1]);
        assert_eq!(
            duplicate_queue.assert_invariants(),
            Err(ModelError::Invariant("duplicate queued job"))
        );

        let mut queue_mismatch = ExecutorModel::new(1, 2).unwrap();
        queue_mismatch.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Terminal(JobOutcome::Success(0)),
            },
        );
        queue_mismatch.queue.push_back(1);
        assert_eq!(
            queue_mismatch.assert_invariants(),
            Err(ModelError::Invariant("queue state mismatch"))
        );

        let mut duplicate_workers = ExecutorModel::new(2, 2).unwrap();
        duplicate_workers.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Running { worker: 0 },
            },
        );
        duplicate_workers.worker_jobs = vec![Some(1), Some(1)];
        duplicate_workers.max_running = 2;
        assert_eq!(
            duplicate_workers.assert_invariants(),
            Err(ModelError::Invariant("job assigned to two workers"))
        );

        let mut worker_mismatch = ExecutorModel::new(2, 2).unwrap();
        worker_mismatch.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Running { worker: 1 },
            },
        );
        worker_mismatch.worker_jobs[0] = Some(1);
        assert_eq!(
            worker_mismatch.assert_invariants(),
            Err(ModelError::Invariant("worker state mismatch"))
        );

        let mut admission_overflow = ExecutorModel::new(1, 1).unwrap();
        admission_overflow.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Queued,
            },
        );
        admission_overflow.jobs.insert(
            2,
            Job {
                payload: 0,
                state: JobState::Queued,
            },
        );
        admission_overflow.queue.extend([1, 2]);
        assert_eq!(
            admission_overflow.assert_invariants(),
            Err(ModelError::Invariant("admission limit exceeded"))
        );

        let mut running_overflow = ExecutorModel::new(1, 1).unwrap();
        running_overflow.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Running { worker: 0 },
            },
        );
        running_overflow.worker_jobs[0] = Some(1);
        assert_eq!(
            running_overflow.assert_invariants(),
            Err(ModelError::Invariant("running limit violated"))
        );

        let mut terminal_work = ExecutorModel::new(1, 1).unwrap();
        terminal_work.lifecycle = Lifecycle::Closed;
        terminal_work.jobs.insert(
            1,
            Job {
                payload: 0,
                state: JobState::Running { worker: 0 },
            },
        );
        terminal_work.worker_jobs[0] = Some(1);
        terminal_work.max_running = 1;
        assert_eq!(
            terminal_work.assert_invariants(),
            Err(ModelError::Invariant("terminal pool retained work"))
        );
        assert_eq!(
            terminal_work.finalize(),
            Err(ModelError::Invariant("finalize left live work"))
        );
    }

    #[test]
    fn fuzz_replay_is_bounded_and_cleanup_is_exact() {
        for seed in 0..256_u64 {
            let mut bytes = [0_u8; 48];
            let mut value = seed.wrapping_mul(0x9e37_79b9);
            for byte in &mut bytes {
                value ^= value << 7;
                value ^= value >> 9;
                *byte = value as u8;
            }
            let first = run_fuzz_case(&bytes).unwrap();
            let second = run_fuzz_case(&bytes).unwrap();
            assert_eq!(first, second, "executor replay diverged for seed {seed}");
            assert!(first.steps <= MAX_FUZZ_STEPS);
            assert!(matches!(
                first.snapshot.lifecycle,
                Lifecycle::Closed | Lifecycle::Cancelled
            ));
            assert_eq!(first.snapshot.queued, 0);
            assert_eq!(first.snapshot.running, 0);
            if let Some(actor) = first.snapshot.actor {
                assert!(!actor.in_flight);
                assert_eq!(actor.mailbox, 0);
                assert_eq!(actor.cleanup_runs, actor.processed.len() + actor.discarded);
            }
        }
        let empty = run_fuzz_case(&[]).unwrap();
        assert_eq!(empty.steps, 1);
        assert_eq!(empty.snapshot.consumed, 0);
    }
}
