//! Ownership/admission reference only. Peer verdicts are explicit inputs;
//! this model neither parses TLS records nor validates certificates.
use super::admission as reference;
use reference::{Deadline, Error, before_commit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    TcpOwner,
    Pending,
    Established,
    Split { reader: bool, writer: bool },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    CertificateRejected,
    HandshakeFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    OutsideDomain,
    InvalidServerName,
    Admission(Error),
    CertificateRejected,
    HandshakeFailed,
}

#[derive(Debug)]
pub struct Tls {
    phase: Phase,
    deadline: Option<Deadline>,
    domain: u64,
    pub handshake_starts: usize,
}

impl Default for Tls {
    fn default() -> Self {
        Self::new()
    }
}

impl Tls {
    pub fn new() -> Self {
        Self {
            phase: Phase::TcpOwner,
            deadline: None,
            domain: 7,
            handshake_starts: 0,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn start(
        &mut self,
        server_valid: bool,
        deadline: Option<Deadline>,
        now: i128,
    ) -> Result<(), Failure> {
        if self.phase != Phase::TcpOwner {
            return Err(Failure::OutsideDomain);
        }
        // By-value transfer precedes argument/outcome refusal, but an admitted
        // success alone may publish the TLS owner.
        self.phase = Phase::Pending;
        self.deadline = deadline;
        let admission = if server_valid {
            before_commit(deadline, self.domain, now, false).map_err(Failure::Admission)
        } else {
            Err(Failure::InvalidServerName)
        };
        if let Err(error) = admission {
            self.phase = Phase::Closed;
            return Err(error);
        }
        self.handshake_starts += 1;
        Ok(())
    }
    pub fn finish(&mut self, verdict: Verdict, now: i128, cancelled: bool) -> Result<(), Failure> {
        if self.phase != Phase::Pending {
            return Err(Failure::OutsideDomain);
        }
        let result = before_commit(self.deadline, self.domain, now, cancelled)
            .map_err(Failure::Admission)
            .and(match verdict {
                Verdict::Verified => Ok(()),
                Verdict::CertificateRejected => Err(Failure::CertificateRejected),
                Verdict::HandshakeFailed => Err(Failure::HandshakeFailed),
            });
        self.phase = if result.is_ok() {
            Phase::Established
        } else {
            Phase::Closed
        };
        result
    }
    pub fn split(&mut self) -> Result<(), Failure> {
        if self.phase != Phase::Established {
            return Err(Failure::OutsideDomain);
        }
        self.phase = Phase::Split {
            reader: true,
            writer: true,
        };
        Ok(())
    }
    pub fn close_half(&mut self, read_half: bool) -> Result<(), Failure> {
        let Phase::Split {
            mut reader,
            mut writer,
        } = self.phase
        else {
            return Err(Failure::OutsideDomain);
        };
        if (read_half && !reader) || (!read_half && !writer) {
            return Err(Failure::OutsideDomain);
        }
        if read_half {
            reader = false;
        } else {
            writer = false;
        }
        self.phase = if reader || writer {
            Phase::Split { reader, writer }
        } else {
            Phase::Closed
        };
        Ok(())
    }
    pub fn close_scope(&mut self) {
        self.phase = Phase::Closed;
    }
    pub fn transport_live(&self) -> bool {
        self.phase != Phase::Closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_admission_consumes_tcp_without_starting_handshake() {
        let mut tls = Tls::new();
        assert_eq!(
            tls.start(false, Some(Deadline::create(7, 0).unwrap()), 0),
            Err(Failure::InvalidServerName)
        );
        assert_eq!(tls.phase(), Phase::Closed);
        assert_eq!(tls.handshake_starts, 0);
        assert!(!tls.transport_live());
        let mut tls = Tls::new();
        assert_eq!(
            tls.start(true, Some(Deadline::create(7, 0).unwrap()), 0),
            Err(Failure::Admission(Error::Timeout))
        );
        assert_eq!(tls.handshake_starts, 0);
        assert!(!tls.transport_live());
    }

    #[test]
    fn handshake_errors_and_cancellation_release_consumed_tcp_before_reply() {
        for (verdict, expected) in [
            (Verdict::CertificateRejected, Failure::CertificateRejected),
            (Verdict::HandshakeFailed, Failure::HandshakeFailed),
        ] {
            let mut tls = Tls::new();
            tls.start(true, None, 0).unwrap();
            assert_eq!(tls.split(), Err(Failure::OutsideDomain));
            assert_eq!(tls.finish(verdict, 0, false), Err(expected));
            assert!(!tls.transport_live());
        }
        let mut tls = Tls::new();
        tls.start(true, Some(Deadline::create(7, 1).unwrap()), 0)
            .unwrap();
        assert_eq!(
            tls.finish(Verdict::Verified, 1, true),
            Err(Failure::Admission(Error::Cancelled))
        );
        assert!(!tls.transport_live());
    }

    #[test]
    fn only_verified_tls_can_split_and_last_half_retires_transport() {
        let mut tls = Tls::new();
        assert_eq!(tls.split(), Err(Failure::OutsideDomain));
        tls.start(true, None, 0).unwrap();
        tls.finish(Verdict::Verified, 0, false).unwrap();
        tls.split().unwrap();
        tls.close_half(true).unwrap();
        assert!(tls.transport_live());
        assert_eq!(tls.close_half(true), Err(Failure::OutsideDomain));
        tls.close_half(false).unwrap();
        assert!(!tls.transport_live());
        tls.close_scope();
        assert_eq!(tls.phase(), Phase::Closed);
    }

    #[test]
    fn terminal_tls_refuses_republication_and_timeout_precedes_peer_verdict() {
        let mut tls = Tls::new();
        assert_eq!(tls.close_half(false), Err(Failure::OutsideDomain));
        assert_eq!(
            tls.finish(Verdict::Verified, 0, false),
            Err(Failure::OutsideDomain)
        );
        tls.start(true, Some(Deadline::create(7, 1).unwrap()), 0)
            .unwrap();
        assert_eq!(
            tls.finish(Verdict::CertificateRejected, 1, false),
            Err(Failure::Admission(Error::Timeout))
        );
        assert_eq!(tls.phase(), Phase::Closed);
        assert_eq!(tls.start(true, None, 0), Err(Failure::OutsideDomain));
        assert_eq!(tls.close_half(true), Err(Failure::OutsideDomain));
        tls.close_scope();
        assert!(!tls.transport_live());
        let mut tls = Tls::default();
        tls.start(true, None, 0).unwrap();
        tls.finish(Verdict::Verified, 0, false).unwrap();
        tls.split().unwrap();
        tls.close_half(false).unwrap();
        assert_eq!(tls.close_half(false), Err(Failure::OutsideDomain));
        assert!(tls.transport_live());
        tls.close_half(true).unwrap();
        assert!(!tls.transport_live());
    }
}
