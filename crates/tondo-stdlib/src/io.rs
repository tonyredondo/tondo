//! Bounded, host-independent I/O protocols used by the standard library.
//!
//! The compiler bridge owns real handles and capabilities.  This module keeps
//! the protocol rules executable without touching a host resource, which makes
//! partial reads, short writes, EOF and resource limits testable in isolation.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoLimits {
    pub max_bytes: usize,
    pub max_read: usize,
}

impl Default for IoLimits {
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024 * 1024,
            max_read: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoError {
    Closed,
    Cancelled,
    InvalidData,
    ResourceLimit,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadResult {
    Data(Vec<u8>),
    Eof,
}

pub trait Reader {
    fn read(&mut self, max: usize) -> Result<ReadResult, IoError>;
}

pub trait Writer {
    fn write(&mut self, data: &[u8]) -> Result<usize, IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

/// Read until EOF while enforcing both the request and aggregate limits.
pub fn read_all<R: Reader>(reader: &mut R, limits: IoLimits) -> Result<Vec<u8>, IoError> {
    if limits.max_bytes == 0 || limits.max_read == 0 {
        return Err(IoError::ResourceLimit);
    }
    let request = limits.max_read.min(limits.max_bytes);
    let mut output = Vec::new();
    loop {
        match reader.read(request)? {
            ReadResult::Eof => return Ok(output),
            ReadResult::Data(chunk) => {
                if chunk.is_empty() || chunk.len() > request {
                    return Err(IoError::InvalidData);
                }
                let next = output
                    .len()
                    .checked_add(chunk.len())
                    .ok_or(IoError::ResourceLimit)?;
                if next > limits.max_bytes {
                    return Err(IoError::ResourceLimit);
                }
                output.extend_from_slice(&chunk);
            }
        }
    }
}

/// Write all bytes, accepting short writes and rejecting a writer that makes
/// no progress.  The input is borrowed only for the duration of this call.
pub fn write_all<W: Writer>(writer: &mut W, data: &[u8]) -> Result<(), IoError> {
    let mut offset = 0;
    while offset < data.len() {
        let written = writer.write(&data[offset..])?;
        if written == 0 || written > data.len() - offset {
            return Err(IoError::InvalidData);
        }
        offset += written;
    }
    writer.flush()
}

#[derive(Debug, Clone)]
pub struct SliceReader {
    bytes: Vec<u8>,
    offset: usize,
    chunk: usize,
}

impl SliceReader {
    pub fn new(bytes: impl Into<Vec<u8>>, chunk: usize) -> Result<Self, IoError> {
        if chunk == 0 {
            return Err(IoError::ResourceLimit);
        }
        Ok(Self {
            bytes: bytes.into(),
            offset: 0,
            chunk,
        })
    }
}

impl Reader for SliceReader {
    fn read(&mut self, max: usize) -> Result<ReadResult, IoError> {
        if max == 0 {
            return Err(IoError::ResourceLimit);
        }
        if self.offset == self.bytes.len() {
            return Ok(ReadResult::Eof);
        }
        let end = self
            .offset
            .saturating_add(self.chunk.min(max))
            .min(self.bytes.len());
        let bytes = self.bytes[self.offset..end].to_vec();
        self.offset = end;
        Ok(ReadResult::Data(bytes))
    }
}

#[derive(Debug, Default)]
pub struct VecWriter {
    bytes: Vec<u8>,
    max_write: Option<usize>,
    flushed: bool,
}

impl VecWriter {
    pub fn with_max_write(max_write: usize) -> Result<Self, IoError> {
        if max_write == 0 {
            return Err(IoError::ResourceLimit);
        }
        Ok(Self {
            bytes: Vec::new(),
            max_write: Some(max_write),
            flushed: false,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn flushed(&self) -> bool {
        self.flushed
    }
}

impl Writer for VecWriter {
    fn write(&mut self, data: &[u8]) -> Result<usize, IoError> {
        let count = self.max_write.unwrap_or(data.len()).min(data.len());
        self.bytes.extend_from_slice(&data[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.flushed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_all_handles_short_reads_and_eof() {
        let mut reader = SliceReader::new(b"tondo".to_vec(), 2).unwrap();
        assert_eq!(
            read_all(
                &mut reader,
                IoLimits {
                    max_bytes: 8,
                    max_read: 3,
                }
            )
            .unwrap(),
            b"tondo"
        );
    }

    #[test]
    fn read_all_rejects_zero_limits_and_overflow() {
        let mut reader = SliceReader::new(b"x".to_vec(), 1).unwrap();
        assert_eq!(
            read_all(
                &mut reader,
                IoLimits {
                    max_bytes: 0,
                    max_read: 1,
                }
            ),
            Err(IoError::ResourceLimit)
        );
        let mut reader = SliceReader::new(b"abcd".to_vec(), 2).unwrap();
        assert_eq!(
            read_all(
                &mut reader,
                IoLimits {
                    max_bytes: 3,
                    max_read: 2,
                }
            ),
            Err(IoError::ResourceLimit)
        );
    }

    #[test]
    fn readers_reject_invalid_chunk_sizes() {
        assert!(matches!(
            SliceReader::new(Vec::new(), 0),
            Err(IoError::ResourceLimit)
        ));
        let mut reader = SliceReader::new(Vec::new(), 1).unwrap();
        assert_eq!(reader.read(0), Err(IoError::ResourceLimit));
    }

    #[test]
    fn write_all_handles_short_writes_and_flushes() {
        let mut writer = VecWriter::with_max_write(2).unwrap();
        write_all(&mut writer, b"tondo").unwrap();
        assert_eq!(writer.bytes(), b"tondo");
        assert!(writer.flushed());
    }

    #[test]
    fn writer_rejects_zero_capacity() {
        assert!(matches!(
            VecWriter::with_max_write(0),
            Err(IoError::ResourceLimit)
        ));
    }
}
