//! What the linkers of every format share: the image they write, and how
//! they say what is wrong with an input.

pub(super) fn error(input: &str, message: impl std::fmt::Display) -> String {
    format!("linking {input}: {message}")
}

/// The bytes of the output, written at offsets in any order.
pub(super) struct Image {
    pub(super) bytes: Vec<u8>,
}

impl Image {
    pub(super) fn put(&mut self, at: u64, data: &[u8]) {
        let at = at as usize;
        if self.bytes.len() < at + data.len() {
            self.bytes.resize(at + data.len(), 0);
        }
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    pub(super) fn u16(&mut self, at: u64, v: u16) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn u32(&mut self, at: u64, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn u64(&mut self, at: u64, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    pub(super) fn read32(&self, at: u64) -> u32 {
        u32::from_le_bytes(self.bytes[at as usize..at as usize + 4].try_into().unwrap())
    }

    pub(super) fn read64(&self, at: u64) -> u64 {
        u64::from_le_bytes(self.bytes[at as usize..at as usize + 8].try_into().unwrap())
    }
}
