//! Profiles in pprof's format (`profile.proto` of github.com/google/pprof),
//! gzipped: what `tondo profile` writes, which `go tool pprof`, speedscope,
//! Pyroscope and Datadog read. It needs only the standard library, so that
//! the runtime can use this same file.

use std::collections::HashMap;

/// A frame of a sample: where it is, the function, and its file and line,
/// `0` when unknown.
pub struct Frame<'a> {
    pub address: u64,
    pub function: &'a str,
    pub file: &'a str,
    pub line: i64,
}

/// A profile as it is made: its strings, functions and locations, each
/// once, and its samples.
pub struct Builder {
    strings: Vec<String>,
    string_ids: HashMap<String, i64>,
    functions: HashMap<(i64, i64), u64>,
    function_list: Vec<(u64, i64, i64)>,
    locations: HashMap<(u64, u64, i64), u64>,
    location_list: Vec<(u64, u64, u64, i64)>,
    sample_types: Vec<(i64, i64)>,
    default_type: i64,
    period: (i64, i64, i64),
    samples: Vec<u8>,
    program: i64,
}

impl Builder {
    /// A profile whose samples have values of `sample_types`, as
    /// `("cpu", "nanoseconds")`, taken every `period` of `period_type`.
    pub fn new(program: &str, sample_types: &[(&str, &str)], period_type: (&str, &str), period: i64) -> Builder {
        let mut builder = Builder {
            strings: vec![String::new()],
            string_ids: HashMap::from([(String::new(), 0)]),
            functions: HashMap::new(),
            function_list: Vec::new(),
            locations: HashMap::new(),
            location_list: Vec::new(),
            sample_types: Vec::new(),
            default_type: 0,
            period: (0, 0, period),
            samples: Vec::new(),
            program: 0,
        };
        builder.program = builder.string(program);
        builder.sample_types = sample_types.iter().map(|(t, u)| (builder.string(t), builder.string(u))).collect();
        builder.period = (builder.string(period_type.0), builder.string(period_type.1), period);
        builder
    }

    /// The sample type a reader shows first, as `inuse_space`.
    pub fn default_type(&mut self, kind: &str) {
        self.default_type = self.string(kind);
    }

    fn string(&mut self, text: &str) -> i64 {
        if let Some(&id) = self.string_ids.get(text) {
            return id;
        }
        let id = self.strings.len() as i64;
        self.strings.push(text.to_string());
        self.string_ids.insert(text.to_string(), id);
        id
    }

    fn location(&mut self, frame: &Frame) -> u64 {
        let (name, file) = (self.string(frame.function), self.string(frame.file));
        let next = self.function_list.len() as u64 + 1;
        let function = *self.functions.entry((name, file)).or_insert(next);
        if function == next {
            self.function_list.push((function, name, file));
        }
        let key = (frame.address, function, frame.line);
        let next = self.location_list.len() as u64 + 1;
        let location = *self.locations.entry(key).or_insert(next);
        if location == next {
            self.location_list.push((location, frame.address, function, frame.line));
        }
        location
    }

    /// A sample: its frames, the innermost first, its values, one for each
    /// sample type, and its labels.
    pub fn sample(&mut self, frames: &[Frame], values: &[i64], labels: &[(&str, &str)]) {
        let locations: Vec<u64> = frames.iter().map(|f| self.location(f)).collect();
        let mut sample = Vec::new();
        packed(&mut sample, 1, locations.iter().copied());
        packed(&mut sample, 2, values.iter().map(|&v| v as u64));
        for (key, value) in labels {
            let mut label = Vec::new();
            varint_field(&mut label, 1, self.string(key) as u64);
            varint_field(&mut label, 2, self.string(value) as u64);
            bytes_field(&mut sample, 3, &label);
        }
        bytes_field(&mut self.samples, 2, &sample);
    }

    /// The profile, gzipped, taken at `time` for `duration`, in nanoseconds.
    pub fn finish(self, time: i64, duration: i64) -> Vec<u8> {
        let mut out = Vec::new();
        let value_type = |(kind, unit): (i64, i64)| {
            let mut value = Vec::new();
            varint_field(&mut value, 1, kind as u64);
            varint_field(&mut value, 2, unit as u64);
            value
        };
        for &sample_type in &self.sample_types {
            bytes_field(&mut out, 1, &value_type(sample_type));
        }
        out.extend_from_slice(&self.samples);
        // One mapping, of the program, whose functions and lines are known.
        let mut mapping = Vec::new();
        varint_field(&mut mapping, 1, 1);
        varint_field(&mut mapping, 3, u64::MAX);
        varint_field(&mut mapping, 5, self.program as u64);
        for field in [7, 8, 9] {
            varint_field(&mut mapping, field, 1);
        }
        bytes_field(&mut out, 3, &mapping);
        for &(id, address, function, line) in &self.location_list {
            let mut location = Vec::new();
            varint_field(&mut location, 1, id);
            varint_field(&mut location, 2, 1);
            varint_field(&mut location, 3, address);
            let mut entry = Vec::new();
            varint_field(&mut entry, 1, function);
            varint_field(&mut entry, 2, line as u64);
            bytes_field(&mut location, 4, &entry);
            bytes_field(&mut out, 4, &location);
        }
        for &(id, name, file) in &self.function_list {
            let mut function = Vec::new();
            varint_field(&mut function, 1, id);
            varint_field(&mut function, 2, name as u64);
            varint_field(&mut function, 3, name as u64);
            varint_field(&mut function, 4, file as u64);
            bytes_field(&mut out, 5, &function);
        }
        for text in &self.strings {
            bytes_field(&mut out, 6, text.as_bytes());
        }
        varint_field(&mut out, 9, time as u64);
        varint_field(&mut out, 10, duration as u64);
        bytes_field(&mut out, 11, &value_type((self.period.0, self.period.1)));
        varint_field(&mut out, 12, self.period.2 as u64);
        varint_field(&mut out, 14, self.default_type as u64);
        gzip(&out)
    }
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn varint_field(out: &mut Vec<u8>, field: u64, value: u64) {
    if value != 0 {
        varint(out, field << 3);
        varint(out, value);
    }
}

fn bytes_field(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    varint(out, field << 3 | 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn packed(out: &mut Vec<u8>, field: u64, values: impl Iterator<Item = u64>) {
    let mut bytes = Vec::new();
    for value in values {
        varint(&mut bytes, value);
    }
    if !bytes.is_empty() {
        bytes_field(out, field, &bytes);
    }
}

/// `data` in gzip, in blocks deflate stores as they are: a profile is small
/// and read rarely, so it is not worth compressing here.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut chunks = data.chunks(0xffff).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        out.push(chunks.peek().is_none() as u8);
        let length = chunk.len() as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_gzip() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let zipped = gzip(b"hello");
        assert_eq!(&zipped[..3], &[0x1f, 0x8b, 8]);
        assert_eq!(&zipped[10..15], &[1, 5, 0, 0xfa, 0xff]);
        assert_eq!(&zipped[15..20], b"hello");
    }
}
