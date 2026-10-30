//! `std.gzip` and `std.zstd` (standard library 14.29): gzip with flate2
//! and Zstandard with ruzstd, whole or as streams of `std.io` over other
//! streams. Data that is not of the format fails as `InvalidData`.

use super::io::{Host, Mode, StreamObject, byte_array, retain, run, stream, tondo_rt_stream_release};
use super::native::{self, NativeObject};
use super::{ArrayHeader, element};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use ruzstd::encoding::CompressionLevel;
use std::io::{ErrorKind, Read, Write};

fn bytes(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(element(array, 1, 0), (*array).len).to_vec() }
}

fn invalid(_: impl std::fmt::Debug) -> std::io::Error {
    ErrorKind::InvalidData.into()
}

fn level(level: i64) -> Compression {
    Compression::new(level.clamp(0, 9) as u32)
}

/// The data of a Zstandard frame, or of several one after another.
fn zstd_frames(mut data: &[u8]) -> std::io::Result<Vec<u8>> {
    if data.is_empty() {
        return Err(ErrorKind::InvalidData.into());
    }
    let mut out = Vec::new();
    while !data.is_empty() {
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut data).map_err(invalid)?;
        decoder.read_to_end(&mut out).map_err(invalid)?;
    }
    Ok(out)
}

/// `gzip.compress`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_gzip(data: *mut ArrayHeader, compression: i64) -> *mut ArrayHeader {
    let mut encoder = GzEncoder::new(Vec::new(), level(compression));
    let _ = encoder.write_all(&bytes(data));
    byte_array(&encoder.finish().unwrap_or_default())
}

/// `zstd.compress`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_zstd(data: *mut ArrayHeader) -> *mut ArrayHeader {
    byte_array(&ruzstd::encoding::compress_to_vec(&bytes(data)[..], CompressionLevel::Fastest))
}

/// `gzip.decompress` (`zstd` 0) or `zstd.decompress` (1): the data, or
/// nothing when it is not of the format.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unpack(data: *mut ArrayHeader, zstd: u8) -> *mut NativeObject {
    let data = bytes(data);
    let unpacked = if zstd != 0 {
        zstd_frames(&data).ok()
    } else {
        let mut out = Vec::new();
        MultiGzDecoder::new(&data[..]).read_to_end(&mut out).ok().map(|_| out)
    };
    native::new(unpacked)
}

/// The data deflated without a header, as zip keeps it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_deflate(data: *mut ArrayHeader, compression: i64) -> *mut ArrayHeader {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), level(compression));
    let _ = encoder.write_all(&bytes(data));
    byte_array(&encoder.finish().unwrap_or_default())
}

/// The data that `deflate` deflated, or nothing when it is not.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_inflate(data: *mut ArrayHeader) -> *mut NativeObject {
    let mut out = Vec::new();
    let inflated = flate2::read::DeflateDecoder::new(&bytes(data)[..]).read_to_end(&mut out).ok().map(|_| out);
    native::new(inflated)
}

/// The CRC-32 of the data, as zip and gzip check it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_crc32(data: *mut ArrayHeader) -> i64 {
    let mut crc = flate2::Crc::new();
    crc.update(&bytes(data));
    crc.sum() as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unpacked(unpacked: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(native::get::<Option<Vec<u8>>>(unpacked).and_then(Option::as_ref).map_or(&[][..], |d| &d[..]))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_unpack_failed(unpacked: *mut NativeObject) -> u8 {
    native::get::<Option<Vec<u8>>>(unpacked).is_none_or(Option::is_none) as u8
}

// ---------------------------------------------------------------------------
// Streams
// ---------------------------------------------------------------------------

/// A stream of `std.io` another one writes to or reads from, which it lets
/// go when it closes.
struct Inner(*mut StreamObject);

unsafe impl Send for Inner {}

impl Drop for Inner {
    fn drop(&mut self) {
        unsafe { tondo_rt_stream_release(self.0) }
    }
}

impl Inner {
    fn write(&self, data: Vec<u8>) -> std::io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        run(self.0, move |s| s.write(&data))
    }

    fn flush(&self) -> std::io::Result<()> {
        run(self.0, |s| s.flush())
    }
}

/// Reads an inner stream, keeping its errors apart from the format's.
struct Source {
    inner: Inner,
    failed: Option<std::io::Error>,
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let max = buffer.len();
        match run(self.inner.0, move |s| s.read(max)) {
            Ok(Some(data)) => {
                buffer[..data.len()].copy_from_slice(&data);
                Ok(data.len())
            }
            Ok(None) => Ok(0),
            Err(error) => {
                let kind = error.kind();
                self.failed = Some(error);
                Err(kind.into())
            }
        }
    }
}

/// The stream's own error, or the format's as `InvalidData`.
fn reading(failed: &mut Option<std::io::Error>, result: std::io::Result<usize>) -> std::io::Result<usize> {
    match (result, failed.take()) {
        (Ok(count), _) => Ok(count),
        (Err(_), Some(error)) => Err(error),
        (Err(_), None) => Err(ErrorKind::InvalidData.into()),
    }
}

/// What a writer has not compressed yet, for Zstandard, which compresses a
/// frame at a time: one per mebibyte, flush or close.
const FRAME: usize = 1 << 20;

enum Packer {
    Gzip(GzEncoder<Vec<u8>>),
    Zstd { pending: Vec<u8>, frames: bool },
}

/// A writer that compresses into another.
struct Packing {
    inner: Inner,
    packer: Option<Packer>,
}

impl Packing {
    /// Compresses what is pending, and the end of the format when `last`.
    fn pack(&mut self, last: bool) -> std::io::Result<()> {
        let out = match self.packer.as_mut() {
            None => return Ok(()),
            Some(Packer::Gzip(encoder)) => {
                if last {
                    encoder.try_finish()?;
                } else {
                    encoder.flush()?;
                }
                std::mem::take(encoder.get_mut())
            }
            Some(Packer::Zstd { pending, frames }) => {
                if pending.is_empty() && (*frames || !last) {
                    Vec::new()
                } else {
                    *frames = true;
                    ruzstd::encoding::compress_to_vec(&std::mem::take(pending)[..], CompressionLevel::Fastest)
                }
            }
        };
        if last {
            self.packer = None;
        }
        self.inner.write(out)
    }
}

impl Host for Packing {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self.packer.as_mut() {
            None => Err(ErrorKind::NotConnected.into()),
            Some(Packer::Gzip(encoder)) => {
                encoder.write_all(data)?;
                let out = std::mem::take(encoder.get_mut());
                self.inner.write(out)
            }
            Some(Packer::Zstd { pending, .. }) => {
                pending.extend_from_slice(data);
                if pending.len() >= FRAME { self.pack(false) } else { Ok(()) }
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.pack(false)?;
        self.inner.flush()
    }

    fn finish(&mut self) -> std::io::Result<()> {
        self.pack(true)?;
        self.inner.flush()
    }
}

/// A reader that decompresses another.
enum Unpacking {
    Gzip(MultiGzDecoder<Source>),
    /// The frame being read, or the source between frames, and whether a
    /// frame came.
    Zstd(Option<ZstdFrames>, bool),
}

enum ZstdFrames {
    Reading(Box<ruzstd::decoding::StreamingDecoder<Ahead, ruzstd::decoding::FrameDecoder>>),
    Between(Ahead),
}

/// A source that can tell whether more comes before reading it.
struct Ahead {
    source: Source,
    peeked: Option<u8>,
}

impl Ahead {
    fn more(&mut self) -> std::io::Result<bool> {
        if self.peeked.is_some() {
            return Ok(true);
        }
        let mut byte = [0u8];
        let count = self.source.read(&mut byte)?;
        self.peeked = (count == 1).then_some(byte[0]);
        Ok(count == 1)
    }
}

impl Read for Ahead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if let Some(byte) = self.peeked.take() {
            buffer[0] = byte;
            return Ok(1);
        }
        self.source.read(buffer)
    }
}

impl Host for Unpacking {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Unpacking::Gzip(decoder) => {
                let read = decoder.read(buffer);
                reading(&mut decoder.get_mut().failed, read)
            }
            Unpacking::Zstd(frames, any) => loop {
                match frames.take() {
                    None => return Ok(0),
                    Some(ZstdFrames::Between(mut ahead)) => {
                        let more = ahead.more();
                        let more = reading(&mut ahead.source.failed, more.map(usize::from))?;
                        match (more, *any) {
                            (0, true) => return Ok(0),
                            (0, false) => return Err(ErrorKind::InvalidData.into()),
                            _ => *any = true,
                        }
                        match ruzstd::decoding::StreamingDecoder::new(ahead) {
                            Ok(decoder) => *frames = Some(ZstdFrames::Reading(Box::new(decoder))),
                            Err(_) => return Err(ErrorKind::InvalidData.into()),
                        }
                    }
                    Some(ZstdFrames::Reading(mut decoder)) => {
                        let read = decoder.read(buffer);
                        let read = reading(&mut decoder.get_mut().source.failed, read)?;
                        *frames = Some(if read == 0 {
                            ZstdFrames::Between(decoder.into_inner())
                        } else {
                            ZstdFrames::Reading(decoder)
                        });
                        if read > 0 {
                            return Ok(read);
                        }
                    }
                }
            },
        }
    }
}

/// `gzip.writer` (`zstd` 0, with its level) or `zstd.writer` (1).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_pack_writer(to: *mut StreamObject, zstd: u8, compression: i64) -> *mut StreamObject {
    let packer = match zstd {
        0 => Packer::Gzip(GzEncoder::new(Vec::new(), level(compression))),
        _ => Packer::Zstd { pending: Vec::new(), frames: false },
    };
    // It may wait on `to` for as long as it takes, as on a pipe.
    stream(Box::new(Packing { inner: Inner(retain(to)), packer: Some(packer) }), Mode::Long, false)
}

/// `gzip.reader` (`zstd` 0) or `zstd.reader` (1).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_unpack_reader(from: *mut StreamObject, zstd: u8) -> *mut StreamObject {
    let source = Source { inner: Inner(retain(from)), failed: None };
    let host = match zstd {
        0 => Unpacking::Gzip(MultiGzDecoder::new(source)),
        _ => Unpacking::Zstd(Some(ZstdFrames::Between(Ahead { source, peeked: None })), false),
    };
    stream(Box::new(host), Mode::Long, false)
}
