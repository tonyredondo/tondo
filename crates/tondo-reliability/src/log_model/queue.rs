//! Independent finite logging queue, writer-prefix and consuming-close oracle.
use std::collections::VecDeque;

pub const MAX_RECORD_BYTES: usize = 2048;
pub const MAX_QUEUE: usize = 8;
pub const MAX_STEPS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidLimit,
    ResourceLimit,
    Backpressure,
    Closed,
    Cancelled,
    Io,
    OutsideDomain,
    Invariant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Block,
    Reject,
    Drop,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receipt {
    Accepted,
    Dropped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteStep {
    Count(usize),
    Fail(Error),
}

/// Reports describe a controlled writer. Invalid counts deliberately write no
/// bytes, as in the sealed host fixture; valid counts commit their exact prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Writer {
    steps: VecDeque<WriteStep>,
    flushes: VecDeque<Result<(), Error>>,
    chunk: usize,
    output: Vec<u8>,
    close_count: usize,
}
impl Writer {
    pub fn new(
        chunk: usize,
        steps: &[WriteStep],
        flushes: &[Result<(), Error>],
    ) -> Result<Self, Error> {
        if chunk == 0 {
            return Err(Error::InvalidLimit);
        }
        if steps.len() > MAX_STEPS || flushes.len() > MAX_STEPS {
            return Err(Error::OutsideDomain);
        }
        let provider_error =
            |error: Error| matches!(error, Error::Io | Error::Cancelled | Error::ResourceLimit);
        if steps
            .iter()
            .any(|step| matches!(step, WriteStep::Fail(error) if !provider_error(*error)))
            || flushes
                .iter()
                .any(|outcome| outcome.is_err_and(|error| !provider_error(error)))
        {
            return Err(Error::OutsideDomain);
        }
        Ok(Self {
            steps: steps.iter().copied().collect(),
            flushes: flushes.iter().copied().collect(),
            chunk,
            output: Vec::new(),
            close_count: 0,
        })
    }
    fn write(&mut self, remaining: &[u8]) -> Result<usize, Error> {
        let count = match self.steps.pop_front() {
            Some(WriteStep::Fail(error)) => return Err(error),
            Some(WriteStep::Count(count)) => count,
            None => remaining.len().min(self.chunk),
        };
        if count > 0 && count <= remaining.len() {
            if self.output.len().saturating_add(count) > MAX_RECORD_BYTES * MAX_STEPS {
                return Err(Error::OutsideDomain);
            }
            self.output.extend_from_slice(&remaining[..count]);
        }
        Ok(count)
    }
    pub fn output(&self) -> &[u8] {
        &self.output
    }
    pub fn close_count(&self) -> usize {
        self.close_count
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub steps: usize,
    pub accepted: usize,
    pub dropped: usize,
    pub refused: usize,
    pub output: Vec<u8>,
    pub peak_bytes: usize,
    pub terminal_owners: usize,
    pub terminal_bytes: usize,
    pub close_count: usize,
}

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// Deterministic bounded schedules check conservation after every operation.
/// A failing close may leave a physical prefix, but no logical queue or writer.
pub fn replay(mut seed: u64, steps: usize) -> Result<Replay, Error> {
    if steps == 0 || steps > MAX_STEPS {
        return Err(Error::OutsideDomain);
    }
    let policy = [Policy::Block, Policy::Reject, Policy::Drop][(random(&mut seed) % 3) as usize];
    let capacity = (random(&mut seed) % 4 + 1) as usize;
    let failure =
        [Error::Io, Error::Cancelled, Error::ResourceLimit][(random(&mut seed) % 3) as usize];
    let writer_steps = if random(&mut seed) & 1 == 0 {
        vec![WriteStep::Count(2), WriteStep::Fail(failure)]
    } else {
        vec![]
    };
    let flushes = if random(&mut seed) & 1 == 0 {
        vec![Err(failure)]
    } else {
        vec![]
    };
    let writer = Writer::new(
        (random(&mut seed) % 8 + 1) as usize,
        &writer_steps,
        &flushes,
    )?;
    let mut queue = Queue::new(capacity, 32, policy, writer)?;
    let mut accepted_bytes = Vec::new();
    let mut result = Replay {
        steps,
        accepted: 0,
        dropped: 0,
        refused: 0,
        output: Vec::new(),
        peak_bytes: 0,
        terminal_owners: 1,
        terminal_bytes: 0,
        close_count: 0,
    };
    for _ in 0..steps {
        let outcome = if random(&mut seed).is_multiple_of(4) {
            queue.flush().map(|_| None)
        } else {
            let token = random(&mut seed);
            let record = if token & 15 == 0 {
                format!("oversized:{token:032x}\n")
            } else {
                format!("record:{token:016x}\n")
            };
            match queue.submit(record.as_bytes()) {
                Ok(Receipt::Accepted) => {
                    accepted_bytes.extend_from_slice(record.as_bytes());
                    result.accepted += 1;
                    Ok(Some(Receipt::Accepted))
                }
                Ok(Receipt::Dropped) => {
                    result.dropped += 1;
                    Ok(Some(Receipt::Dropped))
                }
                Err(error) => Err(error),
            }
        };
        if let Err(error) = outcome {
            if error == Error::OutsideDomain || error == Error::InvalidLimit {
                return Err(error);
            }
            result.refused += 1;
        }
        if !accepted_bytes.starts_with(queue.writer().output())
            || queue.queued() > capacity
            || queue.live_bytes() > capacity * 32
            || queue.offset() > 32
        {
            return Err(Error::Invariant);
        }
    }
    let close = queue.close();
    if close.is_ok() && queue.writer().output() != accepted_bytes {
        return Err(Error::Invariant);
    }
    if !accepted_bytes.starts_with(queue.writer().output()) {
        return Err(Error::Invariant);
    }
    result.output = queue.writer().output().to_vec();
    result.peak_bytes = queue.peak_bytes();
    result.terminal_owners = queue.live_owners();
    result.terminal_bytes = queue.live_bytes();
    result.close_count = queue.writer().close_count();
    if result.terminal_owners != 0 || result.terminal_bytes != 0 || result.close_count != 1 {
        return Err(Error::Invariant);
    }
    Ok(result)
}

/// Only complete formatted records enter the queue. Logical bytes retain the
/// whole record until delivery finishes; a short-write offset does not free it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    capacity: usize,
    record_limit: usize,
    policy: Policy,
    records: VecDeque<Vec<u8>>,
    offset: usize,
    failure: Option<Error>,
    owned: bool,
    writer: Writer,
    peak_bytes: usize,
}
impl Queue {
    pub fn new(
        capacity: usize,
        record_limit: usize,
        policy: Policy,
        writer: Writer,
    ) -> Result<Self, Error> {
        if capacity == 0 || record_limit == 0 {
            return Err(Error::InvalidLimit);
        }
        if capacity > MAX_QUEUE || record_limit > MAX_RECORD_BYTES {
            return Err(Error::OutsideDomain);
        }
        Ok(Self {
            capacity,
            record_limit,
            policy,
            records: VecDeque::new(),
            offset: 0,
            failure: None,
            owned: true,
            writer,
            peak_bytes: 0,
        })
    }
    fn live(&self) -> Result<(), Error> {
        if !self.owned || self.failure.is_some() {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }
    fn fail(&mut self, error: Error) -> Error {
        if error == Error::Io {
            self.failure = Some(error);
        }
        error
    }
    fn drain_one(&mut self) -> Result<(), Error> {
        while let Some(record) = self.records.front() {
            if self.offset == record.len() {
                self.records.pop_front();
                self.offset = 0;
                return Ok(());
            }
            let remaining = &record[self.offset..];
            let available = remaining.len();
            let count = match self.writer.write(remaining) {
                Ok(count) => count,
                Err(error) => return Err(self.fail(error)),
            };
            if count == 0 || count > available {
                return Err(self.fail(Error::Io));
            }
            self.offset += count;
        }
        Err(Error::OutsideDomain)
    }
    pub fn submit(&mut self, record: &[u8]) -> Result<Receipt, Error> {
        self.live()?;
        if record.len() > self.record_limit {
            return Err(Error::ResourceLimit);
        }
        if record.is_empty() || record.last() != Some(&b'\n') {
            return Err(Error::OutsideDomain);
        }
        if self.records.len() == self.capacity {
            match self.policy {
                Policy::Reject => return Err(Error::Backpressure),
                Policy::Drop => return Ok(Receipt::Dropped),
                Policy::Block => self.drain_one()?,
            }
        }
        self.records.push_back(record.to_vec());
        self.peak_bytes = self.peak_bytes.max(self.live_bytes());
        Ok(Receipt::Accepted)
    }
    pub fn flush(&mut self) -> Result<(), Error> {
        self.live()?;
        while !self.records.is_empty() {
            self.drain_one()?;
        }
        match self.writer.flushes.pop_front().unwrap_or(Ok(())) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }
    pub fn close(&mut self) -> Result<(), Error> {
        if !self.owned {
            return Err(Error::Closed);
        }
        let outcome = match self.failure {
            Some(error) => Err(error),
            None => self.flush(),
        };
        self.records.clear();
        self.offset = 0;
        self.owned = false;
        self.writer.close_count += 1;
        outcome
    }
    pub fn writer(&self) -> &Writer {
        &self.writer
    }
    pub fn live_bytes(&self) -> usize {
        self.records.iter().map(Vec::len).sum()
    }
    pub fn live_owners(&self) -> usize {
        usize::from(self.owned)
    }
    pub fn queued(&self) -> usize {
        self.records.len()
    }
    pub fn offset(&self) -> usize {
        self.offset
    }
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queue(policy: Policy, steps: &[WriteStep], flushes: &[Result<(), Error>]) -> Queue {
        Queue::new(1, 8, policy, Writer::new(2, steps, flushes).unwrap()).unwrap()
    }
    #[test]
    fn admission_is_queued_and_each_full_policy_preserves_accepted_records() {
        for policy in [Policy::Block, Policy::Reject, Policy::Drop] {
            let mut q = queue(policy, &[], &[]);
            assert_eq!(q.submit(b"first\n"), Ok(Receipt::Accepted));
            assert!(q.writer().output().is_empty());
            let expected = match policy {
                Policy::Block => Ok(Receipt::Accepted),
                Policy::Reject => Err(Error::Backpressure),
                Policy::Drop => Ok(Receipt::Dropped),
            };
            assert_eq!(q.submit(b"second\n"), expected);
            assert_eq!(q.close(), Ok(()));
            assert_eq!(
                q.writer().output(),
                if policy == Policy::Block {
                    b"first\nsecond\n".as_slice()
                } else {
                    b"first\n".as_slice()
                }
            );
            assert_eq!(q.live_bytes(), 0);
            assert_eq!(q.live_owners(), 0);
            assert_eq!(q.writer().close_count(), 1);
        }
    }
    #[test]
    fn cancellation_and_admission_refusal_resume_only_the_unwritten_suffix() {
        for error in [Error::Cancelled, Error::ResourceLimit] {
            let mut q = queue(
                Policy::Reject,
                &[WriteStep::Count(2), WriteStep::Fail(error)],
                &[],
            );
            q.submit(b"first\n").unwrap();
            assert_eq!(q.flush(), Err(error));
            assert_eq!(q.writer().output(), b"fi");
            assert_eq!(q.offset(), 2);
            assert_eq!(q.live_bytes(), 6);
            q.flush().unwrap();
            q.close().unwrap();
            assert_eq!(q.writer().output(), b"first\n");
            assert_eq!(q.live_owners(), 0);
        }
    }
    #[test]
    fn late_io_and_invalid_counts_are_visible_and_terminal() {
        for steps in [
            vec![WriteStep::Count(2), WriteStep::Fail(Error::Io)],
            vec![WriteStep::Count(0)],
            vec![WriteStep::Count(7)],
        ] {
            let mut q = queue(Policy::Block, &steps, &[]);
            q.submit(b"first\n").unwrap();
            assert_eq!(q.flush(), Err(Error::Io));
            let prefix = q.writer().output().to_vec();
            assert!(prefix.is_empty() || prefix == b"fi");
            assert_eq!(q.flush(), Err(Error::Closed));
            assert_eq!(q.submit(b"next\n"), Err(Error::Closed));
            assert_eq!(q.close(), Err(Error::Io));
            assert_eq!(q.writer().output(), prefix);
            assert_eq!(q.live_bytes(), 0);
            assert_eq!(q.live_owners(), 0);
            assert_eq!(q.close(), Err(Error::Closed));
            assert_eq!(q.writer().close_count(), 1);
        }
    }
    #[test]
    fn consuming_close_retires_the_writer_even_when_drain_or_flush_fails() {
        for error in [Error::Cancelled, Error::ResourceLimit, Error::Io] {
            for failure_in_write in [false, true] {
                let steps = if failure_in_write {
                    vec![WriteStep::Fail(error)]
                } else {
                    vec![]
                };
                let flushes = if failure_in_write {
                    vec![]
                } else {
                    vec![Err(error)]
                };
                let mut q = queue(Policy::Reject, &steps, &flushes);
                q.submit(b"first\n").unwrap();
                assert_eq!(q.close(), Err(error));
                assert_eq!(q.live_owners(), 0);
                assert_eq!(q.live_bytes(), 0);
                assert_eq!(q.writer().close_count(), 1);
            }
        }
    }
    #[test]
    fn formatting_limit_refuses_before_full_drop_or_any_delivery() {
        for policy in [Policy::Block, Policy::Drop, Policy::Reject] {
            let mut q = queue(policy, &[], &[]);
            q.submit(b"first\n").unwrap();
            let before = q.clone();
            assert_eq!(q.submit(b"oversized\n"), Err(Error::ResourceLimit));
            assert_eq!(q, before);
            q.close().unwrap();
        }
    }
    #[test]
    fn generated_schedules_conserve_bytes_and_close_exactly_once() {
        for seed in 0..4096 {
            let seed = seed ^ 0x9e3779b97f4a7c15;
            let result = replay(seed, 96).unwrap();
            assert_eq!(result, replay(seed, 96).unwrap());
            assert_eq!(result.terminal_owners, 0);
            assert_eq!(result.terminal_bytes, 0);
            assert_eq!(result.close_count, 1);
            assert!(result.peak_bytes <= MAX_QUEUE * 32);
        }
    }
    #[test]
    fn reference_domain_rejects_unknown_provider_reports_and_unbounded_replays() {
        assert_eq!(Writer::new(0, &[], &[]), Err(Error::InvalidLimit));
        assert_eq!(
            Writer::new(1, &vec![WriteStep::Count(1); MAX_STEPS + 1], &[]),
            Err(Error::OutsideDomain)
        );
        assert_eq!(
            Writer::new(1, &[], &vec![Ok(()); MAX_STEPS + 1]),
            Err(Error::OutsideDomain)
        );
        for (capacity, bytes, error) in [
            (0, 8, Error::InvalidLimit),
            (1, 0, Error::InvalidLimit),
            (MAX_QUEUE + 1, 8, Error::OutsideDomain),
            (1, MAX_RECORD_BYTES + 1, Error::OutsideDomain),
        ] {
            assert_eq!(
                Queue::new(
                    capacity,
                    bytes,
                    Policy::Reject,
                    Writer::new(1, &[], &[]).unwrap()
                ),
                Err(error)
            );
        }
        let mut bounded = queue(Policy::Reject, &[], &[]);
        for record in [b"".as_slice(), b"missing"] {
            let before = bounded.clone();
            assert_eq!(bounded.submit(record), Err(Error::OutsideDomain));
            assert_eq!(bounded, before);
        }
        bounded.close().unwrap();
        assert_eq!(
            Writer::new(2, &[WriteStep::Fail(Error::Closed)], &[]),
            Err(Error::OutsideDomain)
        );
        assert_eq!(
            Writer::new(2, &[], &[Err(Error::Backpressure)]),
            Err(Error::OutsideDomain)
        );
        assert_eq!(replay(1, 0), Err(Error::OutsideDomain));
        assert_eq!(replay(1, MAX_STEPS + 1), Err(Error::OutsideDomain));
    }
}
