//! Independent bounded reference model for std.channel.
//!
//! The production channel lives in the hosted scheduler and in the private
//! native bridge. This model deliberately owns no runtime state. It keeps
//! payload tokens in an explicit ledger so tests can prove that a failed,
//! cancelled, or losing operation never duplicates or loses affine data.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Maximum bounded capacity accepted by one model run.
pub const MAX_CHANNEL_BUFFER: usize = 64;
/// Maximum queue length used by the explicit unbounded model.
pub const MAX_UNBOUNDED_QUEUE: usize = 64;
/// Maximum endpoint count in one model run.
pub const MAX_CHANNEL_HANDLES: usize = 128;
/// Maximum input consumed by one fuzz case.
pub const MAX_CHANNEL_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum transitions accepted by one fuzz case.
pub const MAX_CHANNEL_FUZZ_STEPS: usize = 512;

/// Capacity forms are intentionally explicit: Unbounded is never a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelCapacity {
    /// A finite FIFO buffer. Zero is a rendezvous.
    Bounded(usize),
    /// An explicitly selected queue with a finite model resource limit.
    Unbounded,
}

/// A unique affine payload token used by the model oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Payload {
    /// Stable identity used by the ownership ledger.
    pub token: u64,
    /// Observable payload value.
    pub value: u64,
}

/// Expected invalid operations. These are negative model cases, not panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelModelError {
    /// A constructor received a negative capacity.
    InvalidCapacity,
    /// A bounded model limit was exceeded.
    Limit,
    /// The endpoint token is no longer live.
    StaleEndpoint,
    /// The endpoint has already been consumed by close.
    ClosedEndpoint,
    /// A payload is not currently owned by its caller.
    PayloadNotOwned,
    /// A waiter token is not pending.
    UnknownWaiter,
    /// A select arm or witness is invalid for the current state.
    InvalidSelect,
    /// A select witness was prepared against an older state.
    StaleProbe,
    /// The model detected a broken ownership or wakeup invariant.
    Invariant,
}

/// Result of a send operation, including every path that returns the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// The value crossed the channel's linearization point.
    Committed,
    /// A bounded buffer had no room and the operation did not suspend.
    Full(Payload),
    /// The receiver side was closed.
    Closed(Payload),
    /// The explicit unbounded resource limit was reached.
    ResourceLimit(Payload),
    /// A pending send was cancelled before commit.
    Cancelled(Payload),
}

/// Result of a receive operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveOutcome {
    /// A value crossed the receive linearization point.
    Item(Payload),
    /// The channel is open but no value is ready.
    Empty,
    /// All senders closed and the committed queue is drained.
    Closed,
    /// A pending receive was cancelled without removing a queued value.
    Cancelled,
}

/// One selectable operation. Preparing arms never changes channel state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectArm {
    /// A send arm carrying an affine payload.
    Send { sender: u64, payload: Payload },
    /// A receive arm.
    Receive { receiver: u64 },
}

/// A select witness captured before commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectProbe {
    version: u64,
    winner: Option<usize>,
    arms: Vec<SelectArm>,
    else_allowed: bool,
}

/// Result of committing one select witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectResult {
    /// The send arm won.
    Send(SendOutcome),
    /// The receive arm won.
    Receive(ReceiveOutcome),
    /// No arm was ready and an else branch was present.
    Else,
    /// No arm was ready and the selection remains pending.
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadLocation {
    Caller,
    Queue,
    SendWaiter(u64),
    SendResult(u64),
    ReceiveResult(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SendWaiter {
    id: u64,
    sender: u64,
    payload: Payload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReceiveWaiter {
    id: u64,
    receiver: u64,
}

/// A stable, public observation used by rollback and fuzz assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSnapshot {
    /// Number of live sender endpoints.
    pub sender_count: usize,
    /// Number of live receiver endpoints.
    pub receiver_count: usize,
    /// Whether the last sender has closed the input side.
    pub sender_closed: bool,
    /// Whether the last receiver has closed the output side.
    pub receiver_closed: bool,
    /// Payload tokens committed into the FIFO queue.
    pub queue: Vec<u64>,
    /// Pending send waiter IDs and their payload tokens.
    pub send_waiters: Vec<(u64, u64)>,
    /// Pending receive waiter IDs.
    pub receive_waiters: Vec<u64>,
    /// Completed send waiter IDs awaiting caller observation.
    pub send_results: Vec<u64>,
    /// Completed receive waiter IDs awaiting caller observation.
    pub receive_results: Vec<u64>,
    /// Payload tokens currently owned by a caller.
    pub caller_payloads: Vec<u64>,
    /// Number of successful send commits.
    pub committed_sends: usize,
    /// Number of successful receive commits.
    pub committed_receives: usize,
    /// Number of cancelled waiters.
    pub cancellations: usize,
    /// Number of delivered waiter wakeups.
    pub wakeups: usize,
}

/// A bounded channel state machine with explicit endpoint and payload
/// ownership. It is sequential; concurrency is represented by FIFO waiter
/// registration and explicit progress transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelModel {
    capacity: ChannelCapacity,
    queue: VecDeque<Payload>,
    senders: BTreeSet<u64>,
    receivers: BTreeSet<u64>,
    sender_closed: bool,
    receiver_closed: bool,
    send_waiters: VecDeque<SendWaiter>,
    receive_waiters: VecDeque<ReceiveWaiter>,
    send_results: BTreeMap<u64, SendOutcome>,
    receive_results: BTreeMap<u64, ReceiveOutcome>,
    wakeups: BTreeMap<u64, u8>,
    payloads: BTreeMap<u64, PayloadLocation>,
    next_endpoint: u64,
    next_waiter: u64,
    next_payload: u64,
    version: u64,
    select_rotation: u64,
    committed_sends: usize,
    committed_receives: usize,
    cancellations: usize,
    rollbacks: usize,
}

impl ChannelModel {
    /// Construct a bounded channel, rejecting capacities outside the model.
    pub fn bounded(capacity: i64) -> Result<Self, ChannelModelError> {
        if capacity < 0 {
            return Err(ChannelModelError::InvalidCapacity);
        }
        let capacity = usize::try_from(capacity).map_err(|_| ChannelModelError::InvalidCapacity)?;
        if capacity > MAX_CHANNEL_BUFFER {
            return Err(ChannelModelError::Limit);
        }
        Ok(Self::new(ChannelCapacity::Bounded(capacity)))
    }

    /// Construct the explicit unbounded form.
    pub fn unbounded() -> Self {
        Self::new(ChannelCapacity::Unbounded)
    }

    /// Construct a model from an already validated capacity.
    pub fn new(capacity: ChannelCapacity) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
            senders: BTreeSet::new(),
            receivers: BTreeSet::new(),
            sender_closed: false,
            receiver_closed: false,
            send_waiters: VecDeque::new(),
            receive_waiters: VecDeque::new(),
            send_results: BTreeMap::new(),
            receive_results: BTreeMap::new(),
            wakeups: BTreeMap::new(),
            payloads: BTreeMap::new(),
            next_endpoint: 1,
            next_waiter: 1,
            next_payload: 1,
            version: 0,
            select_rotation: 0,
            committed_sends: 0,
            committed_receives: 0,
            cancellations: 0,
            rollbacks: 0,
        }
    }

    /// Issue one unique payload owned by the caller.
    pub fn issue(&mut self, value: u64) -> Result<Payload, ChannelModelError> {
        if self.payloads.len() >= MAX_CHANNEL_FUZZ_STEPS * 2 {
            return Err(ChannelModelError::Limit);
        }
        let token = self.next_payload;
        self.next_payload = self
            .next_payload
            .checked_add(1)
            .ok_or(ChannelModelError::Limit)?;
        let payload = Payload { token, value };
        self.payloads.insert(token, PayloadLocation::Caller);
        self.bump();
        Ok(payload)
    }

    /// Create one sender endpoint.
    pub fn sender(&mut self) -> Result<u64, ChannelModelError> {
        if self.sender_closed || self.senders.len() + self.receivers.len() >= MAX_CHANNEL_HANDLES {
            return Err(ChannelModelError::ClosedEndpoint);
        }
        let endpoint = self.allocate_endpoint()?;
        self.senders.insert(endpoint);
        self.bump();
        Ok(endpoint)
    }

    /// Create one receiver endpoint.
    pub fn receiver(&mut self) -> Result<u64, ChannelModelError> {
        if self.receiver_closed || self.senders.len() + self.receivers.len() >= MAX_CHANNEL_HANDLES
        {
            return Err(ChannelModelError::ClosedEndpoint);
        }
        let endpoint = self.allocate_endpoint()?;
        self.receivers.insert(endpoint);
        self.bump();
        Ok(endpoint)
    }

    /// Explicitly fork a live sender over the same channel identity.
    pub fn fork_sender(&mut self, sender: u64) -> Result<u64, ChannelModelError> {
        self.ensure_sender(sender)?;
        self.sender()
    }

    /// Explicitly fork a live receiver over the same channel identity.
    pub fn fork_receiver(&mut self, receiver: u64) -> Result<u64, ChannelModelError> {
        self.ensure_receiver(receiver)?;
        self.receiver()
    }

    /// Create a nonblocking send observation. A payload remains caller-owned
    /// on every non-commit outcome.
    pub fn try_send(
        &mut self,
        sender: u64,
        payload: Payload,
    ) -> Result<SendOutcome, ChannelModelError> {
        self.ensure_sender(sender)?;
        self.ensure_caller(payload)?;
        if self.receiver_closed {
            return Ok(SendOutcome::Closed(payload));
        }
        if let Some(waiter) = self.receive_waiters.pop_front() {
            self.set_payload(payload, PayloadLocation::ReceiveResult(waiter.id))?;
            self.receive_results
                .insert(waiter.id, ReceiveOutcome::Item(payload));
            self.wake(waiter.id)?;
            self.committed_sends += 1;
            self.committed_receives += 1;
            self.bump();
            return Ok(SendOutcome::Committed);
        }
        if self.has_room() {
            self.set_payload(payload, PayloadLocation::Queue)?;
            self.queue.push_back(payload);
            self.committed_sends += 1;
            self.bump();
            return Ok(SendOutcome::Committed);
        }
        Ok(if matches!(self.capacity, ChannelCapacity::Unbounded) {
            SendOutcome::ResourceLimit(payload)
        } else {
            SendOutcome::Full(payload)
        })
    }

    /// Create a nonblocking receive observation.
    pub fn try_receive(&mut self, receiver: u64) -> Result<ReceiveOutcome, ChannelModelError> {
        self.ensure_receiver(receiver)?;
        if let Some(payload) = self.queue.pop_front() {
            self.set_payload(payload, PayloadLocation::Caller)?;
            self.committed_receives += 1;
            self.bump();
            self.progress()?;
            return Ok(ReceiveOutcome::Item(payload));
        }
        if let Some(waiter) = self.send_waiters.pop_front() {
            self.set_payload(waiter.payload, PayloadLocation::Caller)?;
            self.send_results.insert(waiter.id, SendOutcome::Committed);
            self.wake(waiter.id)?;
            self.committed_sends += 1;
            self.committed_receives += 1;
            self.bump();
            return Ok(ReceiveOutcome::Item(waiter.payload));
        }
        if self.sender_closed {
            return Ok(ReceiveOutcome::Closed);
        }
        Ok(ReceiveOutcome::Empty)
    }

    /// Register a send waiter without moving the payload past the waiter
    /// ownership boundary.
    pub fn register_send(
        &mut self,
        sender: u64,
        payload: Payload,
    ) -> Result<u64, ChannelModelError> {
        self.ensure_sender(sender)?;
        self.ensure_caller(payload)?;
        let id = self.allocate_waiter()?;
        self.set_payload(payload, PayloadLocation::SendWaiter(id))?;
        self.send_waiters.push_back(SendWaiter {
            id,
            sender,
            payload,
        });
        self.bump();
        Ok(id)
    }

    /// Register a receive waiter without removing a queued value.
    pub fn register_receive(&mut self, receiver: u64) -> Result<u64, ChannelModelError> {
        self.ensure_receiver(receiver)?;
        let id = self.allocate_waiter()?;
        self.receive_waiters
            .push_back(ReceiveWaiter { id, receiver });
        self.bump();
        Ok(id)
    }

    /// Progress all currently compatible waiters in registration order.
    pub fn progress(&mut self) -> Result<usize, ChannelModelError> {
        let mut transitions = 0;
        loop {
            if self.receiver_closed {
                while let Some(waiter) = self.send_waiters.pop_front() {
                    self.set_payload(waiter.payload, PayloadLocation::SendResult(waiter.id))?;
                    self.complete_send(waiter.id, SendOutcome::Closed(waiter.payload))?;
                    transitions += 1;
                }
                while let Some(waiter) = self.receive_waiters.pop_front() {
                    self.complete_receive(waiter.id, ReceiveOutcome::Closed)?;
                    transitions += 1;
                }
                break;
            }
            if !self.queue.is_empty() && !self.receive_waiters.is_empty() {
                let payload = self.queue.pop_front().ok_or(ChannelModelError::Invariant)?;
                let waiter = self
                    .receive_waiters
                    .pop_front()
                    .ok_or(ChannelModelError::Invariant)?;
                self.set_payload(payload, PayloadLocation::ReceiveResult(waiter.id))?;
                self.complete_receive(waiter.id, ReceiveOutcome::Item(payload))?;
                self.committed_receives += 1;
                transitions += 1;
                continue;
            }
            if let (Some(sender), Some(receiver)) = (
                self.send_waiters.front().copied(),
                self.receive_waiters.front().copied(),
            ) {
                self.send_waiters.pop_front();
                self.receive_waiters.pop_front();
                self.set_payload(sender.payload, PayloadLocation::ReceiveResult(receiver.id))?;
                self.complete_send(sender.id, SendOutcome::Committed)?;
                self.complete_receive(receiver.id, ReceiveOutcome::Item(sender.payload))?;
                self.committed_sends += 1;
                self.committed_receives += 1;
                transitions += 1;
                continue;
            }
            if self.has_room()
                && let Some(sender) = self.send_waiters.pop_front()
            {
                self.set_payload(sender.payload, PayloadLocation::Queue)?;
                self.queue.push_back(sender.payload);
                self.complete_send(sender.id, SendOutcome::Committed)?;
                self.committed_sends += 1;
                transitions += 1;
                continue;
            }
            if self.sender_closed && self.queue.is_empty() && self.send_waiters.is_empty() {
                while let Some(waiter) = self.receive_waiters.pop_front() {
                    self.complete_receive(waiter.id, ReceiveOutcome::Closed)?;
                    transitions += 1;
                }
            }
            break;
        }
        if transitions > 0 {
            self.bump();
        }
        Ok(transitions)
    }

    /// Cancel one pending send and return its payload to the caller.
    pub fn cancel_send(&mut self, waiter: u64) -> Result<SendOutcome, ChannelModelError> {
        let position = self
            .send_waiters
            .iter()
            .position(|candidate| candidate.id == waiter)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        let pending = self
            .send_waiters
            .remove(position)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        self.set_payload(pending.payload, PayloadLocation::SendResult(waiter))?;
        self.complete_send(waiter, SendOutcome::Cancelled(pending.payload))?;
        self.cancellations += 1;
        self.bump();
        self.poll_send(waiter)
    }

    /// Cancel one pending receive without removing a queued payload.
    pub fn cancel_receive(&mut self, waiter: u64) -> Result<ReceiveOutcome, ChannelModelError> {
        let position = self
            .receive_waiters
            .iter()
            .position(|candidate| candidate.id == waiter)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        self.receive_waiters
            .remove(position)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        self.complete_receive(waiter, ReceiveOutcome::Cancelled)?;
        self.cancellations += 1;
        self.bump();
        self.poll_receive(waiter)
    }

    /// Observe a completed send waiter and release its payload to the caller.
    pub fn poll_send(&mut self, waiter: u64) -> Result<SendOutcome, ChannelModelError> {
        let outcome = self
            .send_results
            .remove(&waiter)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        if let SendOutcome::Closed(payload)
        | SendOutcome::ResourceLimit(payload)
        | SendOutcome::Cancelled(payload) = outcome
        {
            self.set_payload(payload, PayloadLocation::Caller)?;
        }
        self.bump();
        Ok(outcome)
    }

    /// Observe a completed receive waiter and release an item to the caller.
    pub fn poll_receive(&mut self, waiter: u64) -> Result<ReceiveOutcome, ChannelModelError> {
        let outcome = self
            .receive_results
            .remove(&waiter)
            .ok_or(ChannelModelError::UnknownWaiter)?;
        if let ReceiveOutcome::Item(payload) = outcome {
            self.set_payload(payload, PayloadLocation::Caller)?;
        }
        self.bump();
        Ok(outcome)
    }

    /// Consume one sender endpoint. The last sender closes the input side.
    pub fn close_sender(&mut self, sender: u64) -> Result<(), ChannelModelError> {
        if !self.senders.remove(&sender) {
            return Err(ChannelModelError::StaleEndpoint);
        }
        let mut retained = VecDeque::new();
        while let Some(waiter) = self.send_waiters.pop_front() {
            if waiter.sender == sender {
                self.set_payload(waiter.payload, PayloadLocation::SendResult(waiter.id))?;
                self.complete_send(waiter.id, SendOutcome::Closed(waiter.payload))?;
            } else {
                retained.push_back(waiter);
            }
        }
        self.send_waiters = retained;
        if self.senders.is_empty() {
            self.sender_closed = true;
        }
        self.bump();
        self.progress()?;
        Ok(())
    }

    /// Consume one receiver endpoint. The last receiver closes the output
    /// side and returns already-committed values in FIFO order.
    pub fn close_receiver(&mut self, receiver: u64) -> Result<Vec<Payload>, ChannelModelError> {
        if !self.receivers.remove(&receiver) {
            return Err(ChannelModelError::StaleEndpoint);
        }
        let mut retained = VecDeque::new();
        while let Some(waiter) = self.receive_waiters.pop_front() {
            if waiter.receiver == receiver {
                self.complete_receive(waiter.id, ReceiveOutcome::Cancelled)?;
                self.cancellations += 1;
            } else {
                retained.push_back(waiter);
            }
        }
        self.receive_waiters = retained;
        let last = self.receivers.is_empty();
        if last {
            self.receiver_closed = true;
        }
        self.bump();
        self.progress()?;
        if !last {
            return Ok(Vec::new());
        }
        let mut drained = Vec::with_capacity(self.queue.len());
        while let Some(payload) = self.queue.pop_front() {
            self.set_payload(payload, PayloadLocation::Caller)?;
            drained.push(payload);
        }
        if !drained.is_empty() {
            self.bump();
        }
        Ok(drained)
    }

    /// Capture a selectable set without mutating any channel state.
    pub fn prepare_select(
        &self,
        arms: &[SelectArm],
        else_allowed: bool,
    ) -> Result<SelectProbe, ChannelModelError> {
        if arms.is_empty() {
            return Err(ChannelModelError::InvalidSelect);
        }
        let mut payloads = BTreeSet::new();
        let mut ready = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            match arm {
                SelectArm::Send { sender, payload } => {
                    self.ensure_sender(*sender)?;
                    self.ensure_caller(*payload)?;
                    if !payloads.insert(payload.token) {
                        return Err(ChannelModelError::PayloadNotOwned);
                    }
                    if self.send_ready(*sender)? {
                        ready.push(index);
                    }
                }
                SelectArm::Receive { receiver } => {
                    self.ensure_receiver(*receiver)?;
                    if self.receive_ready(*receiver)? {
                        ready.push(index);
                    }
                }
            }
        }
        let winner = (!ready.is_empty()).then(|| {
            let offset = (self.select_rotation as usize) % ready.len();
            ready[offset]
        });
        Ok(SelectProbe {
            version: self.version,
            winner,
            arms: arms.to_vec(),
            else_allowed,
        })
    }

    /// Roll back a losing selection. This is intentionally a no-op.
    pub fn rollback_select(&mut self, probe: &SelectProbe) -> Result<(), ChannelModelError> {
        if probe.version != self.version {
            return Err(ChannelModelError::StaleProbe);
        }
        self.rollbacks += 1;
        Ok(())
    }

    /// Commit exactly one prepared arm.
    pub fn commit_select(&mut self, probe: SelectProbe) -> Result<SelectResult, ChannelModelError> {
        if probe.version != self.version {
            return Err(ChannelModelError::StaleProbe);
        }
        let Some(index) = probe.winner else {
            return Ok(if probe.else_allowed {
                SelectResult::Else
            } else {
                SelectResult::Pending
            });
        };
        let arm = *probe
            .arms
            .get(index)
            .ok_or(ChannelModelError::InvalidSelect)?;
        let result = match arm {
            SelectArm::Send { sender, payload } => {
                let result = self.try_send(sender, payload)?;
                if matches!(result, SendOutcome::Full(_) | SendOutcome::ResourceLimit(_)) {
                    return Err(ChannelModelError::StaleProbe);
                }
                SelectResult::Send(result)
            }
            SelectArm::Receive { receiver } => {
                let result = self.try_receive(receiver)?;
                if matches!(result, ReceiveOutcome::Empty) {
                    return Err(ChannelModelError::StaleProbe);
                }
                SelectResult::Receive(result)
            }
        };
        self.select_rotation = self.select_rotation.wrapping_add(1);
        self.bump();
        Ok(result)
    }

    /// Return pending sender waiter IDs.
    pub fn pending_send_ids(&self) -> Vec<u64> {
        self.send_waiters.iter().map(|waiter| waiter.id).collect()
    }

    /// Return pending receiver waiter IDs.
    pub fn pending_receive_ids(&self) -> Vec<u64> {
        self.receive_waiters
            .iter()
            .map(|waiter| waiter.id)
            .collect()
    }

    /// Return live sender endpoint IDs.
    pub fn sender_ids(&self) -> Vec<u64> {
        self.senders.iter().copied().collect()
    }

    /// Return live receiver endpoint IDs.
    pub fn receiver_ids(&self) -> Vec<u64> {
        self.receivers.iter().copied().collect()
    }

    /// Return a stable observation of all externally relevant state.
    pub fn snapshot(&self) -> ChannelSnapshot {
        let mut caller_payloads = self
            .payloads
            .iter()
            .filter_map(|(token, location)| {
                (*location == PayloadLocation::Caller).then_some(*token)
            })
            .collect::<Vec<_>>();
        caller_payloads.sort_unstable();
        ChannelSnapshot {
            sender_count: self.senders.len(),
            receiver_count: self.receivers.len(),
            sender_closed: self.sender_closed,
            receiver_closed: self.receiver_closed,
            queue: self.queue.iter().map(|payload| payload.token).collect(),
            send_waiters: self
                .send_waiters
                .iter()
                .map(|waiter| (waiter.id, waiter.payload.token))
                .collect(),
            receive_waiters: self
                .receive_waiters
                .iter()
                .map(|waiter| waiter.id)
                .collect(),
            send_results: self.send_results.keys().copied().collect(),
            receive_results: self.receive_results.keys().copied().collect(),
            caller_payloads,
            committed_sends: self.committed_sends,
            committed_receives: self.committed_receives,
            cancellations: self.cancellations,
            wakeups: self.wakeups.values().map(|count| usize::from(*count)).sum(),
        }
    }

    /// Verify ownership, FIFO storage, endpoint and wakeup invariants.
    pub fn assert_invariants(&self) -> Result<(), String> {
        if self.senders.len() + self.receivers.len() > MAX_CHANNEL_HANDLES {
            return Err("channel exceeded endpoint limit".into());
        }
        if self.sender_closed && !self.senders.is_empty() {
            return Err("closed sender side retains an endpoint".into());
        }
        if self.receiver_closed && !self.receivers.is_empty() {
            return Err("closed receiver side retains an endpoint".into());
        }
        if let ChannelCapacity::Bounded(capacity) = self.capacity {
            if self.queue.len() > capacity {
                return Err("bounded queue exceeded capacity".into());
            }
        } else if self.queue.len() > MAX_UNBOUNDED_QUEUE {
            return Err("unbounded queue exceeded resource limit".into());
        }
        let mut expected = BTreeMap::new();
        for payload in &self.queue {
            insert_expected(&mut expected, payload.token, PayloadLocation::Queue)?;
        }
        for waiter in &self.send_waiters {
            if !self.senders.contains(&waiter.sender) {
                return Err("send waiter references a stale sender".into());
            }
            insert_expected(
                &mut expected,
                waiter.payload.token,
                PayloadLocation::SendWaiter(waiter.id),
            )?;
        }
        for (id, outcome) in &self.send_results {
            if let SendOutcome::Closed(payload)
            | SendOutcome::ResourceLimit(payload)
            | SendOutcome::Cancelled(payload) = outcome
            {
                insert_expected(
                    &mut expected,
                    payload.token,
                    PayloadLocation::SendResult(*id),
                )?;
            }
            if self.wakeups.get(id) != Some(&1) {
                return Err("completed send waiter did not receive one wakeup".into());
            }
        }
        for waiter in &self.receive_waiters {
            if !self.receivers.contains(&waiter.receiver) {
                return Err("receive waiter references a stale receiver".into());
            }
        }
        for (id, outcome) in &self.receive_results {
            if let ReceiveOutcome::Item(payload) = outcome {
                insert_expected(
                    &mut expected,
                    payload.token,
                    PayloadLocation::ReceiveResult(*id),
                )?;
            }
            if self.wakeups.get(id) != Some(&1) {
                return Err("completed receive waiter did not receive one wakeup".into());
            }
        }
        for (token, location) in &self.payloads {
            if *location != PayloadLocation::Caller
                && expected.get(token).copied() != Some(*location)
            {
                return Err("payload ledger disagrees with channel containers".into());
            }
        }
        for (token, location) in expected {
            if self.payloads.get(&token).copied() != Some(location) {
                return Err("channel container references an unknown or duplicated payload".into());
            }
        }
        if self.wakeups.values().any(|count| *count > 1) {
            return Err("a waiter was woken more than once".into());
        }
        Ok(())
    }

    /// Cancel and close every live resource, then prove that all payloads are
    /// externally accounted for. This models structured task teardown.
    pub fn cleanup(&mut self) -> Result<(), ChannelModelError> {
        for waiter in self.pending_send_ids() {
            let _ = self.cancel_send(waiter)?;
        }
        for waiter in self.pending_receive_ids() {
            let _ = self.cancel_receive(waiter)?;
        }
        for sender in self.sender_ids() {
            self.close_sender(sender)?;
        }
        for receiver in self.receiver_ids() {
            let _ = self.close_receiver(receiver)?;
        }
        for waiter in self.send_results.keys().copied().collect::<Vec<_>>() {
            let _ = self.poll_send(waiter)?;
        }
        for waiter in self.receive_results.keys().copied().collect::<Vec<_>>() {
            let _ = self.poll_receive(waiter)?;
        }
        self.assert_clean()
    }

    fn assert_clean(&self) -> Result<(), ChannelModelError> {
        if !self.queue.is_empty()
            || !self.send_waiters.is_empty()
            || !self.receive_waiters.is_empty()
            || !self.send_results.is_empty()
            || !self.receive_results.is_empty()
            || self
                .payloads
                .values()
                .any(|location| *location != PayloadLocation::Caller)
        {
            return Err(ChannelModelError::Invariant);
        }
        Ok(())
    }

    fn allocate_endpoint(&mut self) -> Result<u64, ChannelModelError> {
        let endpoint = self.next_endpoint;
        self.next_endpoint = self
            .next_endpoint
            .checked_add(1)
            .ok_or(ChannelModelError::Limit)?;
        Ok(endpoint)
    }

    fn allocate_waiter(&mut self) -> Result<u64, ChannelModelError> {
        let waiter = self.next_waiter;
        self.next_waiter = self
            .next_waiter
            .checked_add(1)
            .ok_or(ChannelModelError::Limit)?;
        Ok(waiter)
    }

    fn ensure_sender(&self, sender: u64) -> Result<(), ChannelModelError> {
        self.senders
            .contains(&sender)
            .then_some(())
            .ok_or(ChannelModelError::StaleEndpoint)
    }

    fn ensure_receiver(&self, receiver: u64) -> Result<(), ChannelModelError> {
        self.receivers
            .contains(&receiver)
            .then_some(())
            .ok_or(ChannelModelError::StaleEndpoint)
    }

    fn ensure_caller(&self, payload: Payload) -> Result<(), ChannelModelError> {
        (self.payloads.get(&payload.token) == Some(&PayloadLocation::Caller))
            .then_some(())
            .ok_or(ChannelModelError::PayloadNotOwned)
    }

    fn set_payload(
        &mut self,
        payload: Payload,
        location: PayloadLocation,
    ) -> Result<(), ChannelModelError> {
        let entry = self
            .payloads
            .get_mut(&payload.token)
            .ok_or(ChannelModelError::PayloadNotOwned)?;
        *entry = location;
        Ok(())
    }

    fn send_ready(&self, sender: u64) -> Result<bool, ChannelModelError> {
        self.ensure_sender(sender)?;
        Ok(self.receiver_closed || !self.receive_waiters.is_empty() || self.has_room())
    }

    fn receive_ready(&self, receiver: u64) -> Result<bool, ChannelModelError> {
        self.ensure_receiver(receiver)?;
        Ok(!self.queue.is_empty()
            || !self.send_waiters.is_empty()
            || (self.sender_closed && self.queue.is_empty()))
    }

    fn has_room(&self) -> bool {
        match self.capacity {
            ChannelCapacity::Bounded(capacity) => self.queue.len() < capacity,
            ChannelCapacity::Unbounded => self.queue.len() < MAX_UNBOUNDED_QUEUE,
        }
    }

    fn complete_send(
        &mut self,
        waiter: u64,
        outcome: SendOutcome,
    ) -> Result<(), ChannelModelError> {
        if self.send_results.insert(waiter, outcome).is_some() {
            return Err(ChannelModelError::Invariant);
        }
        self.wake(waiter)
    }

    fn complete_receive(
        &mut self,
        waiter: u64,
        outcome: ReceiveOutcome,
    ) -> Result<(), ChannelModelError> {
        if self.receive_results.insert(waiter, outcome).is_some() {
            return Err(ChannelModelError::Invariant);
        }
        self.wake(waiter)
    }

    fn wake(&mut self, waiter: u64) -> Result<(), ChannelModelError> {
        if self.wakeups.insert(waiter, 1).is_some() {
            return Err(ChannelModelError::Invariant);
        }
        Ok(())
    }

    fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

fn insert_expected(
    expected: &mut BTreeMap<u64, PayloadLocation>,
    token: u64,
    location: PayloadLocation,
) -> Result<(), String> {
    if expected.insert(token, location).is_some() {
        return Err("payload appears in more than one channel container".into());
    }
    Ok(())
}

/// Compact result of a deterministic fuzz replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzSummary {
    /// Number of bounded transitions executed.
    pub steps: usize,
    /// Operations accepted by the state machine.
    pub accepted_operations: usize,
    /// Expected invalid or blocked operations.
    pub rejected_operations: usize,
    /// Final channel observation after structured cleanup.
    pub snapshot: ChannelSnapshot,
}

/// Replay a bounded byte sequence against the channel model.
pub fn run_fuzz_case(input: &[u8]) -> Result<FuzzSummary, String> {
    let input = &input[..input.len().min(MAX_CHANNEL_FUZZ_INPUT_BYTES)];
    let steps = input.len().clamp(1, MAX_CHANNEL_FUZZ_STEPS);
    let input_len = input.len().max(1);
    let capacity = match input.first().copied().unwrap_or_default() % 4 {
        0 => ChannelModel::bounded(0),
        1 => ChannelModel::bounded(2),
        2 => ChannelModel::bounded(8),
        _ => Ok(ChannelModel::unbounded()),
    }
    .map_err(|error| format!("channel setup failed: {error:?}"))?;
    let mut model = capacity;
    let sender = model
        .sender()
        .map_err(|error| format!("sender setup failed: {error:?}"))?;
    let receiver = model
        .receiver()
        .map_err(|error| format!("receiver setup failed: {error:?}"))?;
    let mut accepted_operations = 0;
    let mut rejected_operations = 0;
    for step in 0..steps {
        let byte = input.get(step % input_len).copied().unwrap_or_default();
        let argument = input
            .get((step + 1) % input_len)
            .copied()
            .unwrap_or_default();
        let result = match byte % 14 {
            0 => model
                .issue(u64::from(argument))
                .and_then(|payload| model.try_send(sender, payload).map(|_| ())),
            1 => model.try_receive(receiver).map(|_| ()),
            2 => model
                .issue(u64::from(argument))
                .and_then(|payload| model.register_send(sender, payload).map(|_| ())),
            3 => model.register_receive(receiver).map(|_| ()),
            4 => model.progress().map(|_| ()),
            5 => model
                .pending_send_ids()
                .first()
                .copied()
                .ok_or(ChannelModelError::UnknownWaiter)
                .and_then(|waiter| model.cancel_send(waiter).map(|_| ())),
            6 => model
                .pending_receive_ids()
                .first()
                .copied()
                .ok_or(ChannelModelError::UnknownWaiter)
                .and_then(|waiter| model.cancel_receive(waiter).map(|_| ())),
            7 => model.close_sender(sender),
            8 => model.close_receiver(receiver).map(|_| ()),
            9 => model.fork_sender(sender).map(|_| ()),
            10 => model.fork_receiver(receiver).map(|_| ()),
            11 => select_send_receive(
                &mut model,
                sender,
                receiver,
                u64::from(argument),
                argument & 1 == 0,
            ),
            12 => select_receive(&mut model, receiver, argument & 1 == 0),
            _ => model.progress().map(|_| ()),
        };
        if result.is_ok() {
            accepted_operations += 1;
        } else {
            rejected_operations += 1;
        }
        model
            .assert_invariants()
            .map_err(|error| format!("step {step}: {error}"))?;
    }
    model
        .cleanup()
        .map_err(|error| format!("channel cleanup failed: {error:?}"))?;
    model
        .assert_invariants()
        .map_err(|error| format!("post-cleanup: {error}"))?;
    Ok(FuzzSummary {
        steps,
        accepted_operations,
        rejected_operations,
        snapshot: model.snapshot(),
    })
}

fn select_send_receive(
    model: &mut ChannelModel,
    sender: u64,
    receiver: u64,
    value: u64,
    else_allowed: bool,
) -> Result<(), ChannelModelError> {
    let payload = model.issue(value)?;
    let arms = [
        SelectArm::Send { sender, payload },
        SelectArm::Receive { receiver },
    ];
    let probe = model.prepare_select(&arms, else_allowed)?;
    let before = model.snapshot();
    model.rollback_select(&probe)?;
    if model.snapshot() != before {
        return Err(ChannelModelError::Invariant);
    }
    model.commit_select(probe).map(|_| ())
}

fn select_receive(
    model: &mut ChannelModel,
    receiver: u64,
    else_allowed: bool,
) -> Result<(), ChannelModelError> {
    let probe = model.prepare_select(&[SelectArm::Receive { receiver }], else_allowed)?;
    model.commit_select(probe).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(model: &mut ChannelModel, value: u64) -> Payload {
        model.issue(value).unwrap()
    }

    #[test]
    fn bounded_zero_matches_fifo_rendezvous_and_wakes_once() {
        let mut model = ChannelModel::bounded(0).unwrap();
        let sender = model.sender().unwrap();
        let receiver = model.receiver().unwrap();
        let first = payload(&mut model, 11);
        let second = payload(&mut model, 22);
        let first_waiter = model.register_send(sender, first).unwrap();
        let second_waiter = model.register_send(sender, second).unwrap();
        let first_receive = model.register_receive(receiver).unwrap();
        let second_receive = model.register_receive(receiver).unwrap();
        assert_eq!(model.progress().unwrap(), 2);
        assert_eq!(
            model.poll_receive(first_receive).unwrap(),
            ReceiveOutcome::Item(first)
        );
        assert_eq!(
            model.poll_receive(second_receive).unwrap(),
            ReceiveOutcome::Item(second)
        );
        assert_eq!(
            model.poll_send(first_waiter).unwrap(),
            SendOutcome::Committed
        );
        assert_eq!(
            model.poll_send(second_waiter).unwrap(),
            SendOutcome::Committed
        );
        assert_eq!(model.snapshot().wakeups, 4);
        model.assert_invariants().unwrap();
    }

    #[test]
    fn bounded_backpressure_and_unbounded_resource_limit_preserve_payloads() {
        let mut bounded = ChannelModel::bounded(2).unwrap();
        let sender = bounded.sender().unwrap();
        let receiver = bounded.receiver().unwrap();
        let first = payload(&mut bounded, 1);
        let second = payload(&mut bounded, 2);
        let third = payload(&mut bounded, 3);
        assert_eq!(
            bounded.try_send(sender, first).unwrap(),
            SendOutcome::Committed
        );
        assert_eq!(
            bounded.try_send(sender, second).unwrap(),
            SendOutcome::Committed
        );
        assert_eq!(
            bounded.try_send(sender, third).unwrap(),
            SendOutcome::Full(third)
        );
        assert_eq!(
            bounded.try_receive(receiver).unwrap(),
            ReceiveOutcome::Item(first)
        );
        assert_eq!(
            bounded.try_send(sender, third).unwrap(),
            SendOutcome::Committed
        );
        let mut unbounded = ChannelModel::unbounded();
        let sender = unbounded.sender().unwrap();
        let receiver = unbounded.receiver().unwrap();
        for value in 0..MAX_UNBOUNDED_QUEUE {
            let item = payload(&mut unbounded, value as u64);
            assert_eq!(
                unbounded.try_send(sender, item).unwrap(),
                SendOutcome::Committed
            );
        }
        let extra = payload(&mut unbounded, 99);
        assert_eq!(
            unbounded.try_send(sender, extra).unwrap(),
            SendOutcome::ResourceLimit(extra)
        );
        unbounded.close_receiver(receiver).unwrap();
        unbounded.close_sender(sender).unwrap();
        bounded.close_receiver(receiver).unwrap();
        bounded.close_sender(sender).unwrap();
        bounded.assert_invariants().unwrap();
        unbounded.assert_invariants().unwrap();
    }

    #[test]
    fn close_and_cancel_keep_affine_payloads_and_drain_only_last_receiver() {
        let mut model = ChannelModel::bounded(4).unwrap();
        let sender = model.sender().unwrap();
        let receiver = model.receiver().unwrap();
        let other_receiver = model.fork_receiver(receiver).unwrap();
        let queued = payload(&mut model, 7);
        assert_eq!(
            model.try_send(sender, queued).unwrap(),
            SendOutcome::Committed
        );
        assert!(model.close_receiver(receiver).unwrap().is_empty());
        let pending_payload = payload(&mut model, 8);
        let send_waiter = model.register_send(sender, pending_payload).unwrap();
        let drained = model.close_receiver(other_receiver).unwrap();
        assert_eq!(drained, [queued]);
        assert_eq!(
            model.poll_send(send_waiter).unwrap(),
            SendOutcome::Closed(pending_payload)
        );
        let nine = payload(&mut model, 9);
        let stale = model.try_send(sender, nine);
        assert_eq!(stale.unwrap(), SendOutcome::Closed(nine));
        model.close_sender(sender).unwrap();
        model.assert_invariants().unwrap();
    }

    #[test]
    fn select_else_rollback_and_rotation_are_observable_without_double_commit() {
        let mut model = ChannelModel::bounded(1).unwrap();
        let sender = model.sender().unwrap();
        let receiver = model.receiver().unwrap();
        let empty_probe = model
            .prepare_select(&[SelectArm::Receive { receiver }], true)
            .unwrap();
        let before = model.snapshot();
        assert_eq!(
            model.commit_select(empty_probe).unwrap(),
            SelectResult::Else
        );
        assert_eq!(model.snapshot(), before);
        let first = payload(&mut model, 1);
        let first_probe = model
            .prepare_select(
                &[
                    SelectArm::Send {
                        sender,
                        payload: first,
                    },
                    SelectArm::Receive { receiver },
                ],
                false,
            )
            .unwrap();
        let before_rollback = model.snapshot();
        model.rollback_select(&first_probe).unwrap();
        assert_eq!(model.snapshot(), before_rollback);
        assert_eq!(
            model.commit_select(first_probe).unwrap(),
            SelectResult::Send(SendOutcome::Committed)
        );
        let second = payload(&mut model, 2);
        let third = payload(&mut model, 3);
        assert!(matches!(
            model.try_send(sender, second).unwrap(),
            SendOutcome::Full(_)
        ));
        let receive = model
            .prepare_select(
                &[
                    SelectArm::Receive { receiver },
                    SelectArm::Send {
                        sender,
                        payload: third,
                    },
                ],
                false,
            )
            .unwrap();
        assert!(matches!(
            model.commit_select(receive).unwrap(),
            SelectResult::Receive(ReceiveOutcome::Item(_))
        ));
        assert_eq!(model.snapshot().committed_sends, 1);
        model.cleanup().unwrap();
    }

    #[test]
    fn fuzz_replay_is_bounded_and_cleanup_is_exact() {
        for seed in 0..128_u64 {
            let bytes = seed.to_le_bytes();
            let first = run_fuzz_case(&bytes).unwrap();
            let second = run_fuzz_case(&bytes).unwrap();
            assert_eq!(first, second);
            assert!(first.steps <= MAX_CHANNEL_FUZZ_STEPS);
            assert_eq!(first.snapshot.sender_count, 0);
            assert_eq!(first.snapshot.receiver_count, 0);
            assert!(first.snapshot.queue.is_empty());
            assert!(first.snapshot.send_waiters.is_empty());
            assert!(first.snapshot.receive_waiters.is_empty());
            assert!(first.snapshot.send_results.is_empty());
            assert!(first.snapshot.receive_results.is_empty());
        }
    }
}
