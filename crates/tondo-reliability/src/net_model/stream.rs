//! Finite stream byte ledger with explicit peer input and write credit.

use super::admission as reference;

use reference::{Deadline, Error, Limits, Read, before_commit};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
struct Reservation {
    id: u64,
    requested: usize,
    deadline: Option<Deadline>,
    domain: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits::create(128, 128, 32).unwrap()
    }

    #[test]
    fn readiness_rollback_and_failed_completion_preserve_stream_bytes() {
        let mut stream = Stream::new(8).unwrap();
        stream.feed(b"abc").unwrap();
        let id = stream.prepare(2, limits(), None, 7, 0).unwrap();
        let prepared = stream.snapshot();
        assert_eq!(stream.ready(id), Ok(Some(Read::Data(b"ab".to_vec()))));
        assert_eq!(stream.snapshot(), prepared);
        assert_eq!(
            stream.prepare(1, limits(), None, 7, 0),
            Err(Error::ResourceLimit)
        );
        assert_eq!(stream.snapshot(), prepared);
        assert_eq!(
            stream.finish(id, false, false, 0),
            Ok(Completion::RolledBack)
        );
        assert_eq!(stream.snapshot().queued, b"abc");
        let id = stream.prepare(2, limits(), None, 7, 0).unwrap();
        assert_eq!(stream.finish(id, true, true, 0), Err(Error::Cancelled));
        assert_eq!(stream.snapshot().queued, b"abc");
        let deadline = Some(Deadline::create(7, 5).unwrap());
        let id = stream.prepare(2, limits(), deadline, 7, 0).unwrap();
        assert_eq!(stream.finish(id, true, false, 5), Err(Error::Timeout));
        assert_eq!(stream.snapshot().queued, b"abc");
        let id = stream.prepare(2, limits(), None, 7, 0).unwrap();
        assert_eq!(
            stream.finish(id, true, false, 0),
            Ok(Completion::Committed(Read::Data(b"ab".to_vec())))
        );
        assert_eq!(stream.snapshot().queued, b"c");
        assert!(stream.consistent());
    }

    #[test]
    fn eof_is_after_final_data_and_last_half_owns_transport_cleanup() {
        let mut stream = Stream::new(8).unwrap();
        stream.feed(b"final").unwrap();
        stream.peer_fin();
        let id = stream.prepare(8, limits(), None, 7, 0).unwrap();
        assert_eq!(
            stream.finish(id, true, false, 0),
            Ok(Completion::Committed(Read::Data(b"final".to_vec())))
        );
        let id = stream.prepare(8, limits(), None, 7, 0).unwrap();
        assert_eq!(
            stream.finish(id, true, false, 0),
            Ok(Completion::Committed(Read::Eof))
        );
        stream.shutdown_writer().unwrap();
        assert!(stream.snapshot().writer_open);
        assert!(stream.snapshot().transport_live);
        stream.close_writer();
        assert!(stream.snapshot().transport_live);
        stream.close_reader();
        assert!(!stream.snapshot().transport_live);
        assert!(stream.consistent());
    }

    #[test]
    fn partial_writes_and_peer_credit_do_not_invent_progress() {
        let mut stream = Stream::new(2).unwrap();
        assert_eq!(stream.write(b"abc", 0), Ok(None));
        assert_eq!(stream.snapshot().sent, b"");
        assert_eq!(stream.write(b"abc", 1), Ok(Some(1)));
        assert_eq!(stream.write(b"bc", 2), Ok(Some(2)));
        assert_eq!(stream.write(b"", 0), Ok(Some(0)));
        assert_eq!(stream.snapshot().sent, b"abc");
        assert_eq!(stream.feed(b"abc"), Ok(2));
        assert_eq!(stream.feed(b"c"), Ok(0));
        let id = stream.prepare(1, limits(), None, 7, 0).unwrap();
        assert!(matches!(
            stream.finish(id, true, false, 0),
            Ok(Completion::Committed(_))
        ));
        assert_eq!(stream.feed(b"c"), Ok(1));
        assert_eq!(stream.snapshot().queued, b"bc");
        stream.close_scope();
        assert!(stream.consistent());
        assert_eq!(stream.snapshot().discarded, 2);
    }

    fn replay(seed: u64) -> Snapshot {
        let mut cursor = seed;
        let mut stream = Stream::new(1 + (seed % 16) as usize).unwrap();
        let mut reservation = None;
        for _ in 0..128 {
            cursor = cursor.wrapping_mul(6364136223846793005).wrapping_add(1);
            let bytes = [(cursor >> 8) as u8, (cursor >> 16) as u8];
            match (cursor >> 32) % 7 {
                0 => {
                    stream.feed(&bytes).unwrap();
                }
                1 if reservation.is_none() => {
                    reservation = Some(
                        stream
                            .prepare(1 + (cursor % 8) as i128, limits(), None, 7, 0)
                            .unwrap(),
                    );
                }
                2 if reservation.is_some() => {
                    let before = stream.snapshot();
                    let _ = stream.ready(reservation.unwrap()).unwrap();
                    assert_eq!(stream.snapshot(), before);
                }
                3 if reservation.is_some() => {
                    if stream.finish(reservation.unwrap(), true, false, 0).unwrap()
                        != Completion::Pending
                    {
                        reservation = None;
                    }
                }
                4 if reservation.is_some() => {
                    assert_eq!(
                        stream.finish(reservation.take().unwrap(), false, false, 0),
                        Ok(Completion::RolledBack)
                    );
                }
                5 if reservation.is_some() => {
                    let before = stream.snapshot();
                    assert_eq!(
                        stream.finish(reservation.take().unwrap(), true, true, 0),
                        Err(Error::Cancelled)
                    );
                    assert_eq!(stream.snapshot().queued, before.queued);
                }
                6 => {
                    let _ = stream.write(&bytes, (cursor % 3) as usize);
                }
                _ => {}
            }
            assert!(stream.consistent(), "seed {seed}");
        }
        stream.close_scope();
        let terminal = stream.snapshot();
        assert!(!terminal.transport_live && !terminal.prepared && terminal.queued.is_empty());
        assert!(stream.consistent());
        terminal
    }

    #[test]
    fn generated_schedules_preserve_bytes_and_release_every_owner() {
        for seed in 0..4096 {
            assert_eq!(replay(seed), replay(seed), "seed {seed}");
        }
    }

    #[test]
    fn invalid_model_actions_preserve_owners_and_pending_reads_can_be_retired() {
        assert!(matches!(Stream::new(0), Err(Error::OutsideDomain)));
        assert!(matches!(Stream::new(129), Err(Error::OutsideDomain)));
        let mut stream = Stream::new(8).unwrap();
        let before = stream.snapshot();
        assert_eq!(
            stream.prepare(0, limits(), None, 7, 0),
            Err(Error::InvalidLimit)
        );
        assert_eq!(
            stream.prepare(1, limits(), Some(Deadline::create(8, 9).unwrap()), 7, 0),
            Err(Error::InvalidDeadline)
        );
        assert_eq!(stream.snapshot(), before);
        let id = stream.prepare(1, limits(), None, 7, 0).unwrap();
        assert_eq!(stream.finish(id, true, false, 0), Ok(Completion::Pending));
        let before = stream.snapshot();
        assert_eq!(stream.ready(id + 1), Err(Error::OutsideDomain));
        assert_eq!(
            stream.finish(id + 1, true, false, 0),
            Err(Error::OutsideDomain)
        );
        assert_eq!(stream.snapshot(), before);
        stream.close_reader();
        assert!(!stream.snapshot().prepared && stream.snapshot().transport_live);
        assert_eq!(stream.feed(b"late"), Err(Error::OutsideDomain));
        assert_eq!(
            stream.prepare(1, limits(), None, 7, 0),
            Err(Error::OutsideDomain)
        );
        stream.shutdown_writer().unwrap();
        assert_eq!(stream.write(b"late", 4), Err(Error::OutsideDomain));
        stream.close_writer();
        assert_eq!(stream.shutdown_writer(), Err(Error::OutsideDomain));
        stream.close_scope();
        assert!(!stream.snapshot().transport_live && stream.consistent());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub queued: Vec<u8>,
    pub fed: usize,
    pub observed: usize,
    pub discarded: usize,
    pub sent: Vec<u8>,
    pub prepared: bool,
    pub reader_open: bool,
    pub writer_open: bool,
    pub write_shutdown: bool,
    pub transport_live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    Pending,
    RolledBack,
    Committed(Read),
}

/// Two affine half owners share one abstract transport. Peer credit and input
/// are supplied explicitly; no TCP packet boundary or OS buffer is modeled.
#[derive(Debug)]
pub struct Stream {
    capacity: usize,
    input: VecDeque<u8>,
    peer_fin: bool,
    reader_open: bool,
    writer_open: bool,
    write_shutdown: bool,
    prepared: Option<Reservation>,
    next_id: u64,
    fed: usize,
    observed: usize,
    discarded: usize,
    sent: Vec<u8>,
}

impl Stream {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 || capacity > reference::MAX_REFERENCE_BYTES {
            return Err(Error::OutsideDomain);
        }
        Ok(Self {
            capacity,
            input: VecDeque::new(),
            peer_fin: false,
            reader_open: true,
            writer_open: true,
            write_shutdown: false,
            prepared: None,
            next_id: 1,
            fed: 0,
            observed: 0,
            discarded: 0,
            sent: Vec::new(),
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        if !self.reader_open || self.peer_fin {
            return Err(Error::OutsideDomain);
        }
        let accepted = bytes.len().min(self.capacity - self.input.len());
        self.input.extend(bytes[..accepted].iter().copied());
        self.fed += accepted;
        Ok(accepted)
    }

    pub fn peer_fin(&mut self) {
        self.peer_fin = true;
    }

    pub fn prepare(
        &mut self,
        requested: i128,
        limits: Limits,
        deadline: Option<Deadline>,
        domain: u64,
        now: i128,
    ) -> Result<u64, Error> {
        let requested = limits.request(requested)?;
        before_commit(deadline, domain, now, false)?;
        if !self.reader_open {
            return Err(Error::OutsideDomain);
        }
        if self.prepared.is_some() {
            return Err(Error::ResourceLimit);
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(Error::OutsideDomain)?;
        self.prepared = Some(Reservation {
            id,
            requested,
            deadline,
            domain,
        });
        Ok(id)
    }

    pub fn ready(&self, id: u64) -> Result<Option<Read>, Error> {
        let reservation = self
            .prepared
            .filter(|value| value.id == id)
            .ok_or(Error::OutsideDomain)?;
        if self.input.is_empty() {
            return Ok(self.peer_fin.then_some(Read::Eof));
        }
        Ok(Some(Read::Data(
            self.input
                .iter()
                .copied()
                .take(reservation.requested)
                .collect(),
        )))
    }

    pub fn finish(
        &mut self,
        id: u64,
        winner: bool,
        cancelled: bool,
        now: i128,
    ) -> Result<Completion, Error> {
        let reservation = self
            .prepared
            .filter(|value| value.id == id)
            .ok_or(Error::OutsideDomain)?;
        if !winner {
            self.prepared = None;
            return Ok(Completion::RolledBack);
        }
        if let Err(error) = before_commit(reservation.deadline, reservation.domain, now, cancelled)
        {
            self.prepared = None;
            return Err(error);
        }
        let Some(value) = self.ready(id)? else {
            return Ok(Completion::Pending);
        };
        if let Read::Data(bytes) = &value {
            self.input.drain(..bytes.len());
            self.observed += bytes.len();
        }
        self.prepared = None;
        Ok(Completion::Committed(value))
    }

    pub fn write(&mut self, bytes: &[u8], peer_credit: usize) -> Result<Option<usize>, Error> {
        if !self.writer_open || self.write_shutdown {
            return Err(Error::OutsideDomain);
        }
        if !bytes.is_empty() && peer_credit == 0 {
            return Ok(None);
        }
        let accepted = bytes.len().min(peer_credit);
        if self.sent.len() + accepted > reference::MAX_REFERENCE_BYTES {
            return Err(Error::OutsideDomain);
        }
        self.sent.extend_from_slice(&bytes[..accepted]);
        Ok(Some(accepted))
    }

    pub fn shutdown_writer(&mut self) -> Result<(), Error> {
        if !self.writer_open {
            return Err(Error::OutsideDomain);
        }
        self.write_shutdown = true;
        Ok(())
    }

    pub fn close_reader(&mut self) {
        self.reader_open = false;
        self.prepared = None;
        self.discarded += self.input.len();
        self.input.clear();
    }

    pub fn close_writer(&mut self) {
        self.writer_open = false;
    }

    pub fn close_scope(&mut self) {
        self.close_reader();
        self.close_writer();
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            queued: self.input.iter().copied().collect(),
            fed: self.fed,
            observed: self.observed,
            discarded: self.discarded,
            sent: self.sent.clone(),
            prepared: self.prepared.is_some(),
            reader_open: self.reader_open,
            writer_open: self.writer_open,
            write_shutdown: self.write_shutdown,
            transport_live: self.reader_open || self.writer_open,
        }
    }

    pub fn consistent(&self) -> bool {
        self.fed == self.observed + self.discarded + self.input.len()
            && self.input.len() <= self.capacity
            && self.sent.len() <= reference::MAX_REFERENCE_BYTES
            && (self.reader_open || (self.input.is_empty() && self.prepared.is_none()))
    }
}
