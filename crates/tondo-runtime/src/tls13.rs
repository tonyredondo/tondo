//! Zero-allocation TLS 1.3 record layer for `std.net` streams.
//!
//! `rustls` handles the full TLS handshake (certificates, verification, key
//! exchange, session tickets and key updates via `KernelConnection`). Once a
//! TLS 1.3 handshake completes, `Fast13` takes over the record layer using
//! `ring::aead::LessSafeKey` directly on persistent buffers and stack frames,
//! avoiding `rustls`'s per-record `PrefixedPayload` and `received_plaintext`
//! heap allocations. Connections that negotiate TLS 1.2 stay on `rustls`'s
//! unbuffered connection.

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use rustls::client::{ClientConnectionData, UnbufferedClientConnection};
use rustls::kernel::KernelConnection;
use rustls::server::{ServerConnectionData, UnbufferedServerConnection};
use rustls::unbuffered::{ConnectionState, EncodeError, EncryptError, UnbufferedStatus};
use rustls::{AlertDescription, ConnectionTrafficSecrets, Error as TlsError, ProtocolVersion};
use std::io::{ErrorKind, Read, Write};
use std::sync::Arc;

use crate::poll::Direction;

const RECORD_HEADER_LEN: usize = 5;
const TAG_LEN: usize = 16;
const MAX_PLAINTEXT: usize = 16_384;
const MAX_CIPHERTEXT: usize = MAX_PLAINTEXT + 256;
const MAX_RECORD_BUF: usize = RECORD_HEADER_LEN + MAX_CIPHERTEXT;
const STACK_WRITE_LIMIT: usize = 2048;
const MAX_CONSECUTIVE_EMPTY: u8 = 32;

const CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;
const CONTENT_ALERT: u8 = 21;
const CONTENT_HANDSHAKE: u8 = 22;
const CONTENT_APPLICATION_DATA: u8 = 23;

const HANDSHAKE_NEW_SESSION_TICKET: u8 = 4;
const HANDSHAKE_KEY_UPDATE: u8 = 24;

/// A TLS connection for `std.net`: drives the handshake through rustls, then
/// switches TLS 1.3 connections to the zero-allocation `Fast13` record layer
/// while keeping TLS 1.2 on rustls's unbuffered state machine.
pub(crate) enum TlsStream {
    Shaking(Box<UnbufferedConn>),
    Tls13(Box<Fast13>),
    Tls12(Box<UnbufferedConn>),
    Closed,
}

impl TlsStream {
    pub(crate) fn client(
        config: Arc<rustls::ClientConfig>,
        name: rustls_pki_types::ServerName<'static>,
    ) -> Result<TlsStream, TlsError> {
        let conn = UnbufferedClientConnection::new(config, name)?;
        Ok(TlsStream::Shaking(Box::new(UnbufferedConn::new(UnbufferedSide::Client(conn)))))
    }

    pub(crate) fn server(config: Arc<rustls::ServerConfig>) -> Result<TlsStream, TlsError> {
        let conn = UnbufferedServerConnection::new(config)?;
        Ok(TlsStream::Shaking(Box::new(UnbufferedConn::new(UnbufferedSide::Server(conn)))))
    }

    /// Drives one step of the handshake: returns `Ok(None)` once ready for
    /// application data, or `Ok(Some(direction))` when waiting on the socket.
    pub(crate) fn shake(&mut self, io: &mut (impl Read + Write)) -> std::io::Result<Option<Direction>> {
        let TlsStream::Shaking(conn) = self else {
            return Ok(None);
        };
        if let Some(direction) = conn.shake_step(io)? {
            return Ok(Some(direction));
        }
        let TlsStream::Shaking(conn) = std::mem::replace(self, TlsStream::Closed) else {
            unreachable!();
        };
        if conn.side.protocol_version() == Some(ProtocolVersion::TLSv1_3) {
            let fast = conn.into_fast13()?;
            *self = TlsStream::Tls13(Box::new(fast));
            return Ok(None);
        }
        *self = TlsStream::Tls12(conn);
        Ok(None)
    }

    pub(crate) fn read(&mut self, io: &mut (impl Read + Write), buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            TlsStream::Tls13(fast) => fast.read(io, buffer),
            TlsStream::Tls12(conn) => conn.read(io, buffer),
            TlsStream::Shaking(_) => Err(ErrorKind::WouldBlock.into()),
            TlsStream::Closed => Ok(0),
        }
    }

    pub(crate) fn write(&mut self, io: &mut impl Write, data: &[u8]) -> std::io::Result<()> {
        match self {
            TlsStream::Tls13(fast) => fast.write(io, data),
            TlsStream::Tls12(conn) => conn.write(io, data),
            TlsStream::Shaking(_) => Err(ErrorKind::WouldBlock.into()),
            TlsStream::Closed => Err(ErrorKind::BrokenPipe.into()),
        }
    }

    pub(crate) fn push(&mut self, io: &mut impl Write) -> std::io::Result<()> {
        match self {
            TlsStream::Tls13(fast) => fast.push(io),
            TlsStream::Tls12(conn) | TlsStream::Shaking(conn) => conn.push(io),
            TlsStream::Closed => Ok(()),
        }
    }

    pub(crate) fn wants_write(&self) -> bool {
        match self {
            TlsStream::Tls13(fast) => fast.wants_write(),
            TlsStream::Tls12(conn) | TlsStream::Shaking(conn) => conn.wants_write(),
            TlsStream::Closed => false,
        }
    }

    pub(crate) fn send_close_notify(&mut self, io: &mut impl Write) {
        match self {
            TlsStream::Tls13(fast) => fast.send_close_notify(io),
            TlsStream::Tls12(conn) => conn.send_close_notify(io),
            TlsStream::Shaking(_) | TlsStream::Closed => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Handshake & TLS 1.2 unbuffered driver
// ---------------------------------------------------------------------------

enum UnbufferedSide {
    Client(UnbufferedClientConnection),
    Server(UnbufferedServerConnection),
}

impl UnbufferedSide {
    fn protocol_version(&self) -> Option<ProtocolVersion> {
        match self {
            UnbufferedSide::Client(c) => c.protocol_version(),
            UnbufferedSide::Server(s) => s.protocol_version(),
        }
    }
}

enum KernelSide {
    Client(KernelConnection<ClientConnectionData>),
    Server(KernelConnection<ServerConnectionData>),
}

impl KernelSide {
    fn confidentiality_limit(&self) -> u64 {
        let suite = match self {
            KernelSide::Client(c) => c.negotiated_cipher_suite(),
            KernelSide::Server(s) => s.negotiated_cipher_suite(),
        };
        match suite {
            rustls::SupportedCipherSuite::Tls13(s) => s.common.confidentiality_limit,
            rustls::SupportedCipherSuite::Tls12(s) => s.common.confidentiality_limit,
        }
    }

    fn update_tx_secret(&mut self) -> Result<(u64, ConnectionTrafficSecrets), TlsError> {
        match self {
            KernelSide::Client(c) => c.update_tx_secret(),
            KernelSide::Server(s) => s.update_tx_secret(),
        }
    }

    fn update_rx_secret(&mut self) -> Result<(u64, ConnectionTrafficSecrets), TlsError> {
        match self {
            KernelSide::Client(c) => c.update_rx_secret(),
            KernelSide::Server(s) => s.update_rx_secret(),
        }
    }

    fn handle_new_session_ticket(&mut self, payload: &[u8]) -> Result<(), TlsError> {
        match self {
            KernelSide::Client(c) => c.handle_new_session_ticket(payload),
            KernelSide::Server(_) => Err(TlsError::InappropriateHandshakeMessage {
                expect_types: vec![],
                got_type: rustls::HandshakeType::NewSessionTicket,
            }),
        }
    }
}

pub(crate) struct UnbufferedConn {
    side: UnbufferedSide,
    rx_buf: Vec<u8>,
    rx_pos: usize,
    rx_len: usize,
    tx_buf: Vec<u8>,
    tx_pos: usize,
    pending_plain: Vec<u8>,
    pending_plain_pos: usize,
    peer_closed: bool,
    seen_eof: bool,
    sent_close_notify: bool,
}

impl UnbufferedConn {
    fn new(side: UnbufferedSide) -> UnbufferedConn {
        UnbufferedConn {
            side,
            rx_buf: vec![0u8; 4096],
            rx_pos: 0,
            rx_len: 0,
            tx_buf: Vec::new(),
            tx_pos: 0,
            pending_plain: Vec::new(),
            pending_plain_pos: 0,
            peer_closed: false,
            seen_eof: false,
            sent_close_notify: false,
        }
    }

    fn wants_write(&self) -> bool {
        self.tx_pos < self.tx_buf.len()
    }

    fn push_tx(tx_buf: &mut Vec<u8>, tx_pos: &mut usize, io: &mut impl Write) -> std::io::Result<()> {
        while *tx_pos < tx_buf.len() {
            match io.write(&tx_buf[*tx_pos..]) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => *tx_pos += n,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        *tx_pos = 0;
        tx_buf.clear();
        Ok(())
    }

    fn push(&mut self, io: &mut impl Write) -> std::io::Result<()> {
        Self::push_tx(&mut self.tx_buf, &mut self.tx_pos, io)
    }

    fn discard_rx(&mut self, count: usize) {
        self.rx_pos += count;
        if self.rx_pos >= self.rx_len {
            self.rx_pos = 0;
            self.rx_len = 0;
        }
    }

    fn compact_rx(&mut self) {
        if self.rx_pos > 0 {
            let rem = self.rx_len - self.rx_pos;
            if rem > 0 {
                self.rx_buf.copy_within(self.rx_pos..self.rx_len, 0);
            }
            self.rx_pos = 0;
            self.rx_len = rem;
        }
    }

    fn read_more_rx(&mut self, io: &mut impl Read) -> std::io::Result<usize> {
        self.compact_rx();
        if self.rx_buf.len() - self.rx_len < 4096 {
            let target = (self.rx_buf.len() * 2).max(self.rx_len + 4096).min(64 * 1024);
            if self.rx_buf.len() < target {
                self.rx_buf.resize(target, 0);
            }
        }
        let n = io.read(&mut self.rx_buf[self.rx_len..])?;
        self.rx_len += n;
        Ok(n)
    }

    fn encode_into_tx<Data>(
        tx_buf: &mut Vec<u8>,
        tx_pos: &mut usize,
        enc: &mut rustls::unbuffered::EncodeTlsData<'_, Data>,
    ) -> std::io::Result<()> {
        if *tx_pos == tx_buf.len() {
            *tx_pos = 0;
            tx_buf.clear();
        }
        let base = tx_buf.len();
        let mut space = 4096usize;
        loop {
            tx_buf.resize(base + space, 0);
            match enc.encode(&mut tx_buf[base..]) {
                Ok(written) => {
                    tx_buf.truncate(base + written);
                    return Ok(());
                }
                Err(EncodeError::InsufficientSize(e)) => {
                    space = e.required_size;
                }
                Err(other) => {
                    tx_buf.truncate(base);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, other));
                }
            }
        }
    }

    /// Best-effort flush of any fatal alert queued by rustls after an error.
    fn flush_alert_after_error(&mut self, io: &mut impl Write) {
        for _ in 0..4 {
            let (discard, done) = match &mut self.side {
                UnbufferedSide::Client(c) => {
                    let status = c.process_tls_records(&mut self.rx_buf[self.rx_pos..self.rx_len]);
                    let done = match status.state {
                        Ok(ConnectionState::EncodeTlsData(mut enc)) => {
                            let _ = Self::encode_into_tx(&mut self.tx_buf, &mut self.tx_pos, &mut enc);
                            false
                        }
                        Ok(ConnectionState::TransmitTlsData(tx)) => {
                            let _ = Self::push_tx(&mut self.tx_buf, &mut self.tx_pos, io);
                            if self.tx_pos == self.tx_buf.len() {
                                tx.done();
                            }
                            true
                        }
                        _ => true,
                    };
                    (status.discard, done)
                }
                UnbufferedSide::Server(s) => {
                    let status = s.process_tls_records(&mut self.rx_buf[self.rx_pos..self.rx_len]);
                    let done = match status.state {
                        Ok(ConnectionState::EncodeTlsData(mut enc)) => {
                            let _ = Self::encode_into_tx(&mut self.tx_buf, &mut self.tx_pos, &mut enc);
                            false
                        }
                        Ok(ConnectionState::TransmitTlsData(tx)) => {
                            let _ = Self::push_tx(&mut self.tx_buf, &mut self.tx_pos, io);
                            if self.tx_pos == self.tx_buf.len() {
                                tx.done();
                            }
                            true
                        }
                        _ => true,
                    };
                    (status.discard, done)
                }
            };
            self.discard_rx(discard);
            if done {
                let _ = self.push(io);
                break;
            }
        }
    }

    fn shake_step(&mut self, io: &mut (impl Read + Write)) -> std::io::Result<Option<Direction>> {
        loop {
            if self.wants_write() {
                self.push(io)?;
                if self.wants_write() {
                    return Ok(Some(Direction::Write));
                }
            }
            let outcome = Self::step_once(
                &mut self.side,
                &mut self.rx_buf[self.rx_pos..self.rx_len],
                &mut self.tx_buf,
                &mut self.tx_pos,
                &mut self.pending_plain,
                io,
            );
            match outcome {
                StepOutcome::Continue(discard) => self.discard_rx(discard),
                StepOutcome::WriteTraffic(discard) => {
                    self.discard_rx(discard);
                    return Ok(None);
                }
                StepOutcome::NeedWrite(discard) => {
                    self.discard_rx(discard);
                    return Ok(Some(Direction::Write));
                }
                StepOutcome::NeedRead(discard) => {
                    self.discard_rx(discard);
                    match self.read_more_rx(io) {
                        Ok(0) => {
                            let why = "the other side ended the connection in the TLS handshake";
                            return Err(std::io::Error::new(ErrorKind::UnexpectedEof, why));
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return Ok(Some(Direction::Read));
                        }
                        Err(error) => return Err(error),
                    }
                }
                StepOutcome::PeerClosed(discard) => {
                    self.discard_rx(discard);
                    self.peer_closed = true;
                    let why = "the other side ended the connection in the TLS handshake";
                    return Err(std::io::Error::new(ErrorKind::UnexpectedEof, why));
                }
                StepOutcome::TlsError(discard, err) => {
                    self.discard_rx(discard);
                    self.flush_alert_after_error(io);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, err));
                }
                StepOutcome::IoError(discard, err) => {
                    self.discard_rx(discard);
                    return Err(err);
                }
            }
        }
    }

    fn step_once(
        side: &mut UnbufferedSide,
        incoming: &mut [u8],
        tx_buf: &mut Vec<u8>,
        tx_pos: &mut usize,
        pending_plain: &mut Vec<u8>,
        io: &mut impl Write,
    ) -> StepOutcome {
        match side {
            UnbufferedSide::Client(c) => {
                Self::handle_status(c.process_tls_records(incoming), tx_buf, tx_pos, pending_plain, io)
            }
            UnbufferedSide::Server(s) => {
                Self::handle_status(s.process_tls_records(incoming), tx_buf, tx_pos, pending_plain, io)
            }
        }
    }

    fn handle_status<Data>(
        status: UnbufferedStatus<'_, '_, Data>,
        tx_buf: &mut Vec<u8>,
        tx_pos: &mut usize,
        pending_plain: &mut Vec<u8>,
        io: &mut impl Write,
    ) -> StepOutcome {
        let mut discard = status.discard;
        match status.state {
            Err(err) => StepOutcome::TlsError(discard, err),
            Ok(ConnectionState::EncodeTlsData(mut enc)) => match Self::encode_into_tx(tx_buf, tx_pos, &mut enc) {
                Ok(()) => StepOutcome::Continue(discard),
                Err(err) => StepOutcome::IoError(discard, err),
            },
            Ok(ConnectionState::TransmitTlsData(tx)) => {
                while *tx_pos < tx_buf.len() {
                    match io.write(&tx_buf[*tx_pos..]) {
                        Ok(0) => return StepOutcome::IoError(discard, ErrorKind::WriteZero.into()),
                        Ok(n) => *tx_pos += n,
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            return StepOutcome::NeedWrite(discard);
                        }
                        Err(error) => return StepOutcome::IoError(discard, error),
                    }
                }
                *tx_pos = 0;
                tx_buf.clear();
                tx.done();
                StepOutcome::Continue(discard)
            }
            Ok(ConnectionState::BlockedHandshake) => StepOutcome::NeedRead(discard),
            Ok(ConnectionState::ReadTraffic(mut traffic)) => {
                while let Some(rec) = traffic.next_record() {
                    match rec {
                        Ok(record) => {
                            discard += record.discard;
                            pending_plain.extend_from_slice(record.payload);
                        }
                        Err(err) => return StepOutcome::TlsError(discard, err),
                    }
                }
                StepOutcome::Continue(discard)
            }
            Ok(ConnectionState::WriteTraffic(_)) => StepOutcome::WriteTraffic(discard),
            Ok(ConnectionState::PeerClosed | ConnectionState::Closed) => StepOutcome::PeerClosed(discard),
            Ok(_) => StepOutcome::Continue(discard),
        }
    }

    fn into_fast13(mut self: Box<Self>) -> std::io::Result<Fast13> {
        // Compact any remaining handshake-coalesced ciphertext to offset 0.
        self.compact_rx();
        let (secrets, kernel) = match self.side {
            UnbufferedSide::Client(c) => {
                let (secrets, kernel) =
                    c.dangerous_into_kernel_connection().map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
                (secrets, KernelSide::Client(kernel))
            }
            UnbufferedSide::Server(s) => {
                let (secrets, kernel) =
                    s.dangerous_into_kernel_connection().map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
                (secrets, KernelSide::Server(kernel))
            }
        };
        let tx = TrafficCipher::new(secrets.tx.0, secrets.tx.1)
            .map_err(|()| std::io::Error::new(ErrorKind::InvalidData, TlsError::EncryptError))?;
        let rx = TrafficCipher::new(secrets.rx.0, secrets.rx.1)
            .map_err(|()| std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError))?;
        let tx_limit = kernel.confidentiality_limit();
        let rx_len = self.rx_len;
        let mut rx_buf = self.rx_buf;
        if rx_buf.len() < 4096 {
            rx_buf.resize(4096, 0);
        }
        Ok(Fast13 {
            kernel,
            tx,
            rx,
            tx_limit,
            rx_buf,
            rx_pos: 0,
            rx_len,
            plain_start: 0,
            plain_end: 0,
            pending_plain: self.pending_plain,
            pending_plain_pos: self.pending_plain_pos,
            hs_buf: Vec::new(),
            tx_buf: Vec::new(),
            tx_pos: 0,
            received_close_notify: self.peer_closed,
            sent_close_notify: self.sent_close_notify,
            seen_eof: self.seen_eof,
            empty_records: 0,
        })
    }

    // --- TLS 1.2 fallback read / write ---

    fn read(&mut self, io: &mut (impl Read + Write), buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pending_plain_pos < self.pending_plain.len() {
                let n = buffer.len().min(self.pending_plain.len() - self.pending_plain_pos);
                buffer[..n].copy_from_slice(&self.pending_plain[self.pending_plain_pos..self.pending_plain_pos + n]);
                self.pending_plain_pos += n;
                if self.pending_plain_pos == self.pending_plain.len() {
                    self.pending_plain_pos = 0;
                    self.pending_plain.clear();
                }
                return Ok(n);
            }
            if self.peer_closed {
                return Ok(0);
            }
            if self.seen_eof {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            let outcome = Self::step_once(
                &mut self.side,
                &mut self.rx_buf[self.rx_pos..self.rx_len],
                &mut self.tx_buf,
                &mut self.tx_pos,
                &mut self.pending_plain,
                io,
            );
            match outcome {
                StepOutcome::Continue(discard) => {
                    self.discard_rx(discard);
                    if self.pending_plain_pos < self.pending_plain.len() {
                        continue;
                    }
                    if discard == 0 {
                        match self.read_more_rx(io) {
                            Ok(0) => self.seen_eof = true,
                            Ok(_) => {}
                            Err(error) => return Err(error),
                        }
                    }
                }
                StepOutcome::WriteTraffic(discard) | StepOutcome::NeedRead(discard) => {
                    self.discard_rx(discard);
                    if self.pending_plain_pos < self.pending_plain.len() {
                        continue;
                    }
                    match self.read_more_rx(io) {
                        Ok(0) => self.seen_eof = true,
                        Ok(_) => {}
                        Err(error) => return Err(error),
                    }
                }
                StepOutcome::NeedWrite(discard) => {
                    self.discard_rx(discard);
                    if self.pending_plain_pos < self.pending_plain.len() {
                        continue;
                    }
                    match self.read_more_rx(io) {
                        Ok(0) => self.seen_eof = true,
                        Ok(_) => {}
                        Err(error) => return Err(error),
                    }
                }
                StepOutcome::PeerClosed(discard) => {
                    self.discard_rx(discard);
                    self.peer_closed = true;
                    if self.pending_plain_pos < self.pending_plain.len() {
                        continue;
                    }
                    return Ok(0);
                }
                StepOutcome::TlsError(discard, err) => {
                    self.discard_rx(discard);
                    self.flush_alert_after_error(io);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, err));
                }
                StepOutcome::IoError(discard, err) => {
                    self.discard_rx(discard);
                    return Err(err);
                }
            }
        }
    }

    fn write(&mut self, io: &mut impl Write, data: &[u8]) -> std::io::Result<()> {
        self.push(io)?;
        for chunk in data.chunks(MAX_PLAINTEXT) {
            if self.tx_pos == self.tx_buf.len() {
                self.tx_pos = 0;
                self.tx_buf.clear();
            }
            let base = self.tx_buf.len();
            let mut space = chunk.len() + 64;
            loop {
                self.tx_buf.resize(base + space, 0);
                let (discard, outcome) = Self::encrypt_unbuffered(
                    &mut self.side,
                    &mut self.rx_buf[self.rx_pos..self.rx_len],
                    chunk,
                    &mut self.tx_buf,
                    &mut self.tx_pos,
                    base,
                    &mut self.pending_plain,
                    io,
                );
                self.discard_rx(discard);
                match outcome {
                    EncryptStep::Done(written) => {
                        self.tx_buf.truncate(base + written);
                        break;
                    }
                    EncryptStep::Grow(required) => {
                        space = required;
                    }
                    EncryptStep::Continue => {
                        self.tx_buf.truncate(base);
                    }
                    EncryptStep::Err(err) => {
                        self.tx_buf.truncate(base);
                        return Err(err);
                    }
                }
            }
            self.push(io)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn encrypt_unbuffered(
        side: &mut UnbufferedSide,
        incoming: &mut [u8],
        chunk: &[u8],
        tx_buf: &mut Vec<u8>,
        tx_pos: &mut usize,
        base: usize,
        pending_plain: &mut Vec<u8>,
        io: &mut impl Write,
    ) -> (usize, EncryptStep) {
        match side {
            UnbufferedSide::Client(c) => Self::handle_encrypt_status(
                c.process_tls_records(incoming),
                chunk,
                tx_buf,
                tx_pos,
                base,
                pending_plain,
                io,
            ),
            UnbufferedSide::Server(s) => Self::handle_encrypt_status(
                s.process_tls_records(incoming),
                chunk,
                tx_buf,
                tx_pos,
                base,
                pending_plain,
                io,
            ),
        }
    }

    fn handle_encrypt_status<Data>(
        status: UnbufferedStatus<'_, '_, Data>,
        chunk: &[u8],
        tx_buf: &mut Vec<u8>,
        tx_pos: &mut usize,
        base: usize,
        pending_plain: &mut Vec<u8>,
        io: &mut impl Write,
    ) -> (usize, EncryptStep) {
        let mut discard = status.discard;
        match status.state {
            Ok(ConnectionState::WriteTraffic(mut wt)) => match wt.encrypt(chunk, &mut tx_buf[base..]) {
                Ok(written) => (discard, EncryptStep::Done(written)),
                Err(EncryptError::InsufficientSize(e)) => (discard, EncryptStep::Grow(e.required_size)),
                Err(other) => (discard, EncryptStep::Err(std::io::Error::new(ErrorKind::InvalidData, other))),
            },
            Ok(ConnectionState::EncodeTlsData(mut enc)) => {
                tx_buf.truncate(base);
                match Self::encode_into_tx(tx_buf, tx_pos, &mut enc) {
                    Ok(()) => (discard, EncryptStep::Continue),
                    Err(e) => (discard, EncryptStep::Err(e)),
                }
            }
            Ok(ConnectionState::TransmitTlsData(tx)) => {
                tx_buf.truncate(base);
                let _ = Self::push_tx(tx_buf, tx_pos, io);
                if *tx_pos == tx_buf.len() {
                    tx.done();
                }
                (discard, EncryptStep::Continue)
            }
            Ok(ConnectionState::ReadTraffic(mut traffic)) => {
                while let Some(rec) = traffic.next_record() {
                    match rec {
                        Ok(record) => {
                            discard += record.discard;
                            pending_plain.extend_from_slice(record.payload);
                        }
                        Err(err) => {
                            return (discard, EncryptStep::Err(std::io::Error::new(ErrorKind::InvalidData, err)));
                        }
                    }
                }
                (discard, EncryptStep::Continue)
            }
            Err(e) => (discard, EncryptStep::Err(std::io::Error::new(ErrorKind::InvalidData, e))),
            _ => (discard, EncryptStep::Err(ErrorKind::BrokenPipe.into())),
        }
    }

    fn send_close_notify(&mut self, io: &mut impl Write) {
        if self.sent_close_notify {
            return;
        }
        self.sent_close_notify = true;
        let base = self.tx_buf.len();
        self.tx_buf.resize(base + 128, 0);
        let (discard, written) = match &mut self.side {
            UnbufferedSide::Client(c) => {
                let status = c.process_tls_records(&mut self.rx_buf[self.rx_pos..self.rx_len]);
                let written = match status.state {
                    Ok(ConnectionState::WriteTraffic(mut wt)) => wt.queue_close_notify(&mut self.tx_buf[base..]).ok(),
                    _ => None,
                };
                (status.discard, written)
            }
            UnbufferedSide::Server(s) => {
                let status = s.process_tls_records(&mut self.rx_buf[self.rx_pos..self.rx_len]);
                let written = match status.state {
                    Ok(ConnectionState::WriteTraffic(mut wt)) => wt.queue_close_notify(&mut self.tx_buf[base..]).ok(),
                    _ => None,
                };
                (status.discard, written)
            }
        };
        self.discard_rx(discard);
        if let Some(n) = written {
            self.tx_buf.truncate(base + n);
        } else {
            self.tx_buf.truncate(base);
        }
        let _ = self.push(io);
    }
}

enum EncryptStep {
    Done(usize),
    Grow(usize),
    Continue,
    Err(std::io::Error),
}

enum StepOutcome {
    Continue(usize),
    WriteTraffic(usize),
    NeedWrite(usize),
    NeedRead(usize),
    PeerClosed(usize),
    TlsError(usize, TlsError),
    IoError(usize, std::io::Error),
}

// ---------------------------------------------------------------------------
// Zero-allocation TLS 1.3 record layer
// ---------------------------------------------------------------------------

struct TrafficCipher {
    key: LessSafeKey,
    iv: [u8; 12],
    seq: u64,
}

impl TrafficCipher {
    fn new(seq: u64, secret: ConnectionTrafficSecrets) -> Result<TrafficCipher, ()> {
        let (alg, key_bytes, iv_bytes) = match secret {
            ConnectionTrafficSecrets::Aes128Gcm { key, iv } => (&aead::AES_128_GCM, key, iv),
            ConnectionTrafficSecrets::Aes256Gcm { key, iv } => (&aead::AES_256_GCM, key, iv),
            ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv } => (&aead::CHACHA20_POLY1305, key, iv),
            _ => return Err(()),
        };
        let unbound = UnboundKey::new(alg, key_bytes.as_ref()).map_err(|_| ())?;
        let mut iv = [0u8; 12];
        iv.copy_from_slice(iv_bytes.as_ref());
        Ok(TrafficCipher { key: LessSafeKey::new(unbound), iv, seq })
    }

    #[inline]
    fn next_nonce(&mut self) -> Result<Nonce, TlsError> {
        let seq = self.seq;
        self.seq = seq.checked_add(1).ok_or(TlsError::EncryptError)?;
        let mut n = self.iv;
        let be = seq.to_be_bytes();
        for i in 0..8 {
            n[4 + i] ^= be[i];
        }
        Ok(Nonce::assume_unique_for_key(n))
    }

    #[inline]
    fn seal_in_place(&mut self, header: &[u8; 5], in_out: &mut [u8]) -> Result<aead::Tag, TlsError> {
        let nonce = self.next_nonce()?;
        self.key.seal_in_place_separate_tag(nonce, Aad::from(*header), in_out).map_err(|_| TlsError::EncryptError)
    }

    #[inline]
    fn open_in_place(&mut self, header: &[u8; 5], in_out: &mut [u8]) -> Result<usize, TlsError> {
        let nonce = self.next_nonce().map_err(|_| TlsError::DecryptError)?;
        let plain = self.key.open_in_place(nonce, Aad::from(*header), in_out).map_err(|_| TlsError::DecryptError)?;
        Ok(plain.len())
    }
}

pub(crate) struct Fast13 {
    kernel: KernelSide,
    tx: TrafficCipher,
    rx: TrafficCipher,
    tx_limit: u64,
    /// Persistent receive buffer holding unread ciphertext (`rx_pos..rx_len`)
    /// and any leftover decrypted plaintext (`plain_start..plain_end`, with
    /// `plain_start <= plain_end <= rx_pos <= rx_len`).
    rx_buf: Vec<u8>,
    rx_pos: usize,
    rx_len: usize,
    plain_start: usize,
    plain_end: usize,
    /// Plaintext decrypted during handshake (only populated if application
    /// data arrived coalesced with the final handshake flight).
    pending_plain: Vec<u8>,
    pending_plain_pos: usize,
    /// Reassembly buffer for post-handshake messages fragmented across records.
    hs_buf: Vec<u8>,
    /// Unsent encrypted bytes when the socket could not accept a write at once.
    tx_buf: Vec<u8>,
    tx_pos: usize,
    received_close_notify: bool,
    sent_close_notify: bool,
    seen_eof: bool,
    empty_records: u8,
}

impl Fast13 {
    fn wants_write(&self) -> bool {
        self.tx_pos < self.tx_buf.len()
    }

    fn push(&mut self, io: &mut impl Write) -> std::io::Result<()> {
        while self.tx_pos < self.tx_buf.len() {
            match io.write(&self.tx_buf[self.tx_pos..]) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => self.tx_pos += n,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        self.tx_pos = 0;
        self.tx_buf.clear();
        if self.tx_buf.capacity() > 64 * 1024 {
            self.tx_buf.shrink_to(16 * 1024);
        }
        Ok(())
    }

    fn encrypt_into_tx(&mut self, content_type: u8, payload: &[u8]) -> std::io::Result<()> {
        debug_assert!(payload.len() <= MAX_PLAINTEXT);
        let enc_len = payload.len() + 1 + TAG_LEN;
        let header = [CONTENT_APPLICATION_DATA, 0x03, 0x03, (enc_len >> 8) as u8, enc_len as u8];
        if self.tx_pos == self.tx_buf.len() {
            self.tx_pos = 0;
            self.tx_buf.clear();
        }
        let start = self.tx_buf.len();
        self.tx_buf.reserve(RECORD_HEADER_LEN + enc_len);
        self.tx_buf.extend_from_slice(&header);
        self.tx_buf.extend_from_slice(payload);
        self.tx_buf.push(content_type);
        let plain_start = start + RECORD_HEADER_LEN;
        let plain_end = plain_start + payload.len() + 1;
        let tag = self
            .tx
            .seal_in_place(&header, &mut self.tx_buf[plain_start..plain_end])
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        self.tx_buf.extend_from_slice(tag.as_ref());
        Ok(())
    }

    fn rotate_tx_key(&mut self, request_peer_update: u8) -> std::io::Result<()> {
        // RFC 8446 §4.6.3: HandshakeType::KeyUpdate(24), len = 1, request_update.
        let msg = [HANDSHAKE_KEY_UPDATE, 0, 0, 1, request_peer_update];
        self.encrypt_into_tx(CONTENT_HANDSHAKE, &msg)?;
        let (seq, secret) =
            self.kernel.update_tx_secret().map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        self.tx = TrafficCipher::new(seq, secret)
            .map_err(|()| std::io::Error::new(ErrorKind::InvalidData, TlsError::EncryptError))?;
        Ok(())
    }

    fn write(&mut self, io: &mut impl Write, data: &[u8]) -> std::io::Result<()> {
        if self.wants_write() {
            self.push(io)?;
        }
        // Zero-allocation fast path: encrypt small writes directly onto the
        // stack and hand them to the socket in a single `write` syscall.
        if !self.wants_write() && data.len() <= STACK_WRITE_LIMIT && self.tx.seq < self.tx_limit.saturating_sub(2) {
            let enc_len = data.len() + 1 + TAG_LEN;
            let total = RECORD_HEADER_LEN + enc_len;
            let header = [CONTENT_APPLICATION_DATA, 0x03, 0x03, (enc_len >> 8) as u8, enc_len as u8];
            let mut frame = [0u8; RECORD_HEADER_LEN + STACK_WRITE_LIMIT + 1 + TAG_LEN];
            frame[..RECORD_HEADER_LEN].copy_from_slice(&header);
            frame[RECORD_HEADER_LEN..RECORD_HEADER_LEN + data.len()].copy_from_slice(data);
            frame[RECORD_HEADER_LEN + data.len()] = CONTENT_APPLICATION_DATA;
            let tag = self
                .tx
                .seal_in_place(&header, &mut frame[RECORD_HEADER_LEN..RECORD_HEADER_LEN + data.len() + 1])
                .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
            frame[RECORD_HEADER_LEN + data.len() + 1..total].copy_from_slice(tag.as_ref());
            match io.write(&frame[..total]) {
                Ok(n) if n == total => return Ok(()),
                Ok(n) => {
                    self.tx_buf.extend_from_slice(&frame[n..total]);
                    return Ok(());
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    self.tx_buf.extend_from_slice(&frame[..total]);
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
        for chunk in data.chunks(MAX_PLAINTEXT) {
            if self.tx.seq >= self.tx_limit.saturating_sub(2) {
                self.rotate_tx_key(0)?;
            }
            self.encrypt_into_tx(CONTENT_APPLICATION_DATA, chunk)?;
            self.push(io)?;
        }
        Ok(())
    }

    fn send_close_notify(&mut self, io: &mut impl Write) {
        if self.sent_close_notify {
            return;
        }
        self.sent_close_notify = true;
        // AlertLevel::Warning(1), AlertDescription::CloseNotify(0).
        let _ = self.encrypt_into_tx(CONTENT_ALERT, &[1, 0]);
        let _ = self.push(io);
    }

    fn send_fatal_alert(&mut self, io: &mut impl Write, desc: AlertDescription) {
        if !self.sent_close_notify {
            self.sent_close_notify = true;
            // AlertLevel::Fatal(2).
            let _ = self.encrypt_into_tx(CONTENT_ALERT, &[2, u8::from(desc)]);
            let _ = self.push(io);
        }
    }

    fn read(&mut self, io: &mut (impl Read + Write), buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.pending_plain_pos < self.pending_plain.len() {
            let n = buffer.len().min(self.pending_plain.len() - self.pending_plain_pos);
            buffer[..n].copy_from_slice(&self.pending_plain[self.pending_plain_pos..self.pending_plain_pos + n]);
            self.pending_plain_pos += n;
            if self.pending_plain_pos == self.pending_plain.len() {
                self.pending_plain = Vec::new();
                self.pending_plain_pos = 0;
            }
            return Ok(n);
        }
        if self.plain_start < self.plain_end {
            let n = buffer.len().min(self.plain_end - self.plain_start);
            buffer[..n].copy_from_slice(&self.rx_buf[self.plain_start..self.plain_start + n]);
            self.plain_start += n;
            if self.plain_start == self.plain_end {
                self.plain_start = 0;
                self.plain_end = 0;
                if self.rx_pos == self.rx_len {
                    self.rx_pos = 0;
                    self.rx_len = 0;
                }
            }
            return Ok(n);
        }
        if self.received_close_notify {
            return Ok(0);
        }
        if self.seen_eof {
            return Err(ErrorKind::UnexpectedEof.into());
        }

        loop {
            // Ensure at least the 5-byte TLS record header is buffered.
            if !self.ensure_rx_bytes(io, RECORD_HEADER_LEN)? {
                return if self.received_close_notify { Ok(0) } else { Err(ErrorKind::UnexpectedEof.into()) };
            }
            let outer_type = self.rx_buf[self.rx_pos];
            let rec_len = u16::from_be_bytes([self.rx_buf[self.rx_pos + 3], self.rx_buf[self.rx_pos + 4]]) as usize;
            if rec_len > MAX_CIPHERTEXT {
                self.send_fatal_alert(io, AlertDescription::RecordOverflow);
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    TlsError::InappropriateMessage {
                        expect_types: vec![rustls::ContentType::ApplicationData],
                        got_type: rustls::ContentType::from(outer_type),
                    },
                ));
            }
            let total = RECORD_HEADER_LEN + rec_len;
            if !self.ensure_rx_bytes(io, total)? {
                return Err(ErrorKind::UnexpectedEof.into());
            }

            // Legacy middlebox ChangeCipherSpec (RFC 8446 §5): ignore 1-byte [0x01].
            if outer_type == CONTENT_CHANGE_CIPHER_SPEC {
                if rec_len != 1 || self.rx_buf[self.rx_pos + RECORD_HEADER_LEN] != 1 {
                    self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                }
                self.advance_rx(total);
                continue;
            }
            if outer_type == CONTENT_ALERT {
                if rec_len != 2 {
                    return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                }
                let level = self.rx_buf[self.rx_pos + RECORD_HEADER_LEN];
                let desc = self.rx_buf[self.rx_pos + RECORD_HEADER_LEN + 1];
                self.advance_rx(total);
                if desc == u8::from(AlertDescription::CloseNotify) {
                    self.received_close_notify = true;
                    return Ok(0);
                }
                if level == 1 && desc == u8::from(AlertDescription::UserCanceled) {
                    continue;
                }
                return Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    TlsError::AlertReceived(AlertDescription::from(desc)),
                ));
            }
            if outer_type != CONTENT_APPLICATION_DATA || rec_len < 1 + TAG_LEN {
                self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
            }

            let hdr = [
                self.rx_buf[self.rx_pos],
                self.rx_buf[self.rx_pos + 1],
                self.rx_buf[self.rx_pos + 2],
                self.rx_buf[self.rx_pos + 3],
                self.rx_buf[self.rx_pos + 4],
            ];
            let payload_start = self.rx_pos + RECORD_HEADER_LEN;
            let decrypted_len =
                match self.rx.open_in_place(&hdr, &mut self.rx_buf[payload_start..payload_start + rec_len]) {
                    Ok(len) => len,
                    Err(err) => {
                        self.send_fatal_alert(io, AlertDescription::BadRecordMac);
                        return Err(std::io::Error::new(ErrorKind::InvalidData, err));
                    }
                };
            self.rx_pos += total;

            // RFC 8446 §5.4: strip trailing zero padding to find inner ContentType.
            let mut end = payload_start + decrypted_len;
            while end > payload_start && self.rx_buf[end - 1] == 0 {
                end -= 1;
            }
            if end == payload_start {
                self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
            }
            let inner_type = self.rx_buf[end - 1];
            let plain_len = (end - 1) - payload_start;

            match inner_type {
                CONTENT_APPLICATION_DATA => {
                    if plain_len == 0 {
                        self.empty_records += 1;
                        if self.empty_records > MAX_CONSECUTIVE_EMPTY {
                            self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                            return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                        }
                        if self.rx_pos == self.rx_len {
                            self.rx_pos = 0;
                            self.rx_len = 0;
                        }
                        continue;
                    }
                    self.empty_records = 0;
                    let n = buffer.len().min(plain_len);
                    buffer[..n].copy_from_slice(&self.rx_buf[payload_start..payload_start + n]);
                    if n < plain_len {
                        self.plain_start = payload_start + n;
                        self.plain_end = payload_start + plain_len;
                    } else if self.rx_pos == self.rx_len {
                        self.rx_pos = 0;
                        self.rx_len = 0;
                    }
                    return Ok(n);
                }
                CONTENT_HANDSHAKE => {
                    self.empty_records = 0;
                    self.handle_post_handshake(io, payload_start, plain_len)?;
                    if self.rx_pos == self.rx_len {
                        self.rx_pos = 0;
                        self.rx_len = 0;
                    }
                }
                CONTENT_ALERT => {
                    if plain_len != 2 {
                        self.send_fatal_alert(io, AlertDescription::DecodeError);
                        return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                    }
                    let level = self.rx_buf[payload_start];
                    let desc = self.rx_buf[payload_start + 1];
                    if self.rx_pos == self.rx_len {
                        self.rx_pos = 0;
                        self.rx_len = 0;
                    }
                    if desc == u8::from(AlertDescription::CloseNotify) {
                        self.received_close_notify = true;
                        return Ok(0);
                    }
                    if level == 1 && desc == u8::from(AlertDescription::UserCanceled) {
                        continue;
                    }
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        TlsError::AlertReceived(AlertDescription::from(desc)),
                    ));
                }
                _ => {
                    self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                }
            }
        }
    }

    fn advance_rx(&mut self, count: usize) {
        self.rx_pos += count;
        if self.rx_pos == self.rx_len {
            self.rx_pos = 0;
            self.rx_len = 0;
        }
    }

    /// Ensures `rx_len - rx_pos >= needed`. Returns `Ok(false)` on clean EOF.
    fn ensure_rx_bytes(&mut self, io: &mut impl Read, needed: usize) -> std::io::Result<bool> {
        while self.rx_len - self.rx_pos < needed {
            if self.rx_pos > 0 {
                let rem = self.rx_len - self.rx_pos;
                if rem > 0 {
                    self.rx_buf.copy_within(self.rx_pos..self.rx_len, 0);
                }
                self.rx_pos = 0;
                self.rx_len = rem;
            }
            if self.rx_buf.len() < needed {
                let new_size = (self.rx_buf.len() * 2).max(needed).min(MAX_RECORD_BUF);
                self.rx_buf.resize(new_size, 0);
            }
            let n = io.read(&mut self.rx_buf[self.rx_len..])?;
            if n == 0 {
                self.seen_eof = true;
                return Ok(false);
            }
            self.rx_len += n;
        }
        Ok(true)
    }

    fn handle_post_handshake(
        &mut self,
        io: &mut impl Write,
        payload_start: usize,
        plain_len: usize,
    ) -> std::io::Result<()> {
        if self.hs_buf.is_empty() {
            // Fast path: process directly from `rx_buf` without allocating.
            let mut offset = payload_start;
            let end = payload_start + plain_len;
            while offset < end {
                if end - offset < 4 {
                    self.hs_buf.extend_from_slice(&self.rx_buf[offset..end]);
                    return Ok(());
                }
                let msg_type = self.rx_buf[offset];
                let msg_len =
                    u32::from_be_bytes([0, self.rx_buf[offset + 1], self.rx_buf[offset + 2], self.rx_buf[offset + 3]])
                        as usize;
                if msg_len > 64 * 1024 {
                    self.send_fatal_alert(io, AlertDescription::DecodeError);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                }
                if end - offset < 4 + msg_len {
                    self.hs_buf.extend_from_slice(&self.rx_buf[offset..end]);
                    return Ok(());
                }
                self.process_post_handshake_msg(io, msg_type, offset + 4, msg_len, false)?;
                offset += 4 + msg_len;
            }
            return Ok(());
        }

        self.hs_buf.extend_from_slice(&self.rx_buf[payload_start..payload_start + plain_len]);
        let mut pos = 0;
        while pos < self.hs_buf.len() {
            if self.hs_buf.len() - pos < 4 {
                break;
            }
            let msg_type = self.hs_buf[pos];
            let msg_len =
                u32::from_be_bytes([0, self.hs_buf[pos + 1], self.hs_buf[pos + 2], self.hs_buf[pos + 3]]) as usize;
            if msg_len > 64 * 1024 {
                self.send_fatal_alert(io, AlertDescription::DecodeError);
                return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
            }
            if self.hs_buf.len() - pos < 4 + msg_len {
                break;
            }
            self.process_post_handshake_msg(io, msg_type, pos + 4, msg_len, true)?;
            pos += 4 + msg_len;
        }
        if pos > 0 {
            self.hs_buf.drain(..pos);
        }
        Ok(())
    }

    fn process_post_handshake_msg(
        &mut self,
        io: &mut impl Write,
        msg_type: u8,
        start: usize,
        len: usize,
        from_hs_buf: bool,
    ) -> std::io::Result<()> {
        match msg_type {
            HANDSHAKE_NEW_SESSION_TICKET => {
                let payload =
                    if from_hs_buf { &self.hs_buf[start..start + len] } else { &self.rx_buf[start..start + len] };
                if let Err(err) = self.kernel.handle_new_session_ticket(payload) {
                    self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, err));
                }
                Ok(())
            }
            HANDSHAKE_KEY_UPDATE => {
                let payload =
                    if from_hs_buf { &self.hs_buf[start..start + len] } else { &self.rx_buf[start..start + len] };
                if payload.len() != 1 || payload[0] > 1 {
                    self.send_fatal_alert(io, AlertDescription::IllegalParameter);
                    return Err(std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError));
                }
                let requested = payload[0];
                let (seq, secret) =
                    self.kernel.update_rx_secret().map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
                self.rx = TrafficCipher::new(seq, secret)
                    .map_err(|()| std::io::Error::new(ErrorKind::InvalidData, TlsError::DecryptError))?;
                if requested == 1 {
                    self.rotate_tx_key(0)?;
                    let _ = self.push(io);
                }
                Ok(())
            }
            _ => {
                self.send_fatal_alert(io, AlertDescription::UnexpectedMessage);
                Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    TlsError::InappropriateHandshakeMessage {
                        expect_types: vec![],
                        got_type: rustls::HandshakeType::from(msg_type),
                    },
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Pipe {
        rx: VecDeque<u8>,
        tx: VecDeque<u8>,
        peer_eof: bool,
    }

    impl Pipe {
        fn new() -> Pipe {
            Pipe { rx: VecDeque::new(), tx: VecDeque::new(), peer_eof: false }
        }
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.rx.is_empty() {
                return if self.peer_eof { Ok(0) } else { Err(ErrorKind::WouldBlock.into()) };
            }
            let n = buf.len().min(self.rx.len());
            for slot in &mut buf[..n] {
                *slot = self.rx.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.tx.extend(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn pump(a: &mut Pipe, b: &mut Pipe) {
        b.rx.extend(a.tx.drain(..));
        a.rx.extend(b.tx.drain(..));
    }

    fn make_configs(
        versions: &'static [&'static rustls::SupportedProtocolVersion],
    ) -> (Arc<rustls::ClientConfig>, Arc<rustls::ServerConfig>) {
        // Minimal self-signed DER cert+key generated via rustls test keys? No,
        // use `std.net` test cert from `bench/tondo/src/tls/` if present, or
        // construct with PEM embedded in test fixtures.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/tondo/src/tls");
        let ca = crate::server::certificates(root.join("ca.pem").to_str().unwrap(), "ca").unwrap();
        let cert = crate::server::certificates(root.join("cert.pem").to_str().unwrap(), "cert").unwrap();
        let key = crate::server::private_key(root.join("key.pem").to_str().unwrap()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut server = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(versions)
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(cert, key)
            .unwrap();
        server.enable_secret_extraction = true;
        let mut roots = rustls::RootCertStore::empty();
        for c in ca {
            roots.add(c).unwrap();
        }
        let mut client = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.enable_secret_extraction = true;
        (Arc::new(client), Arc::new(server))
    }

    fn complete_handshake(c: &mut TlsStream, s: &mut TlsStream, cp: &mut Pipe, sp: &mut Pipe) {
        let (mut c_done, mut s_done) = (false, false);
        for _ in 0..32 {
            if !c_done {
                c_done = c.shake(cp).unwrap().is_none();
                pump(cp, sp);
            }
            if !s_done {
                s_done = s.shake(sp).unwrap().is_none();
                pump(cp, sp);
            }
            if c_done && s_done {
                return;
            }
        }
        panic!("handshake did not finish");
    }

    #[test]
    fn tls13_fast_path_round_trips_large_payloads_key_updates_and_close() {
        static ONLY_13: [&rustls::SupportedProtocolVersion; 1] = [&rustls::version::TLS13];
        let (client_cfg, server_cfg) = make_configs(&ONLY_13);
        let name = rustls_pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
        let mut client = TlsStream::client(client_cfg, name).unwrap();
        let mut server = TlsStream::server(server_cfg).unwrap();
        let (mut cp, mut sp) = (Pipe::new(), Pipe::new());
        complete_handshake(&mut client, &mut server, &mut cp, &mut sp);
        assert!(matches!(client, TlsStream::Tls13(_)));
        assert!(matches!(server, TlsStream::Tls13(_)));

        // Small echo round-trips (stack fast path).
        for i in 0..20 {
            let msg = format!("ping {i}\n");
            client.write(&mut cp, msg.as_bytes()).unwrap();
            pump(&mut cp, &mut sp);
            let mut got = [0u8; 64];
            let n = server.read(&mut sp, &mut got).unwrap();
            assert_eq!(&got[..n], msg.as_bytes());
            server.write(&mut sp, &got[..n]).unwrap();
            pump(&mut cp, &mut sp);
            let n = client.read(&mut cp, &mut got).unwrap();
            assert_eq!(&got[..n], msg.as_bytes());
        }

        // Trigger a TLS 1.3 KeyUpdate with update_requested = 1 from client -> server.
        if let TlsStream::Tls13(fast) = &mut client {
            fast.rotate_tx_key(1).unwrap();
            fast.push(&mut cp).unwrap();
        }
        client.write(&mut cp, b"after-key-update").unwrap();
        pump(&mut cp, &mut sp);
        let mut got = [0u8; 64];
        let n = server.read(&mut sp, &mut got).unwrap();
        assert_eq!(&got[..n], b"after-key-update");
        // Server responded with its own KeyUpdate(0) and rotated its TX key too.
        server.write(&mut sp, b"server-rotated").unwrap();
        pump(&mut cp, &mut sp);
        let n = client.read(&mut cp, &mut got).unwrap();
        assert_eq!(&got[..n], b"server-rotated");

        // Multi-record large payload (100 KiB > 6 * 16 KiB).
        let big: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        client.write(&mut cp, &big).unwrap();
        pump(&mut cp, &mut sp);
        let mut received = Vec::new();
        let mut chunk = [0u8; 4096];
        while received.len() < big.len() {
            let n = server.read(&mut sp, &mut chunk).unwrap();
            received.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(received, big);

        // Clean close_notify.
        client.send_close_notify(&mut cp);
        pump(&mut cp, &mut sp);
        assert_eq!(server.read(&mut sp, &mut chunk).unwrap(), 0);
    }

    #[test]
    fn tls12_fallback_round_trips_and_closes() {
        static ONLY_12: [&rustls::SupportedProtocolVersion; 1] = [&rustls::version::TLS12];
        let (client_cfg, server_cfg) = make_configs(&ONLY_12);
        let name = rustls_pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
        let mut client = TlsStream::client(client_cfg, name).unwrap();
        let mut server = TlsStream::server(server_cfg).unwrap();
        let (mut cp, mut sp) = (Pipe::new(), Pipe::new());
        complete_handshake(&mut client, &mut server, &mut cp, &mut sp);
        assert!(matches!(client, TlsStream::Tls12(_)));
        assert!(matches!(server, TlsStream::Tls12(_)));

        client.write(&mut cp, b"hello tls12").unwrap();
        pump(&mut cp, &mut sp);
        let mut buf = [0u8; 64];
        let n = server.read(&mut sp, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello tls12");
        server.write(&mut sp, b"reply tls12").unwrap();
        pump(&mut cp, &mut sp);
        let n = client.read(&mut cp, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"reply tls12");

        client.send_close_notify(&mut cp);
        pump(&mut cp, &mut sp);
        assert_eq!(server.read(&mut sp, &mut buf).unwrap(), 0);
    }

    #[test]
    fn tls_handshake_failure_returns_error_without_panicking() {
        static ONLY_13: [&rustls::SupportedProtocolVersion; 1] = [&rustls::version::TLS13];
        let (client_cfg, server_cfg) = make_configs(&ONLY_13);
        // Server cert is for 127.0.0.1; requesting "wrong.example" must fail
        // certificate verification cleanly (including flushing the TLS alert).
        let wrong_name = rustls_pki_types::ServerName::try_from("wrong.example").unwrap().to_owned();
        let mut client = TlsStream::client(client_cfg, wrong_name).unwrap();
        let mut server = TlsStream::server(server_cfg).unwrap();
        let (mut cp, mut sp) = (Pipe::new(), Pipe::new());
        let mut client_err = None;
        for _ in 0..16 {
            if client_err.is_none() {
                match client.shake(&mut cp) {
                    Ok(_) => {}
                    Err(e) => client_err = Some(e),
                }
                pump(&mut cp, &mut sp);
            }
            let _ = server.shake(&mut sp);
            pump(&mut cp, &mut sp);
            if client_err.is_some() {
                break;
            }
        }
        assert!(client_err.is_some(), "expected certificate name mismatch error");
    }
}
