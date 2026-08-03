#![doc = "Portable runner and data contract for the unpublished Tondo draft."]

pub mod document;
pub mod lineage;
pub mod manifest;
pub mod protocol;
pub mod runner;
pub mod seal;

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

pub const SUITE_NAME: &str = "tondo-conformance-draft";
pub const SUITE_FORMAT: &str = "tondo-conformance-manifest-draft";
pub const ADAPTER_PROTOCOL: &str = "tondo-conformance-adapter-draft";
pub const RESULT_FORMAT: &str = "tondo-conformance-result-draft";

pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut result = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

pub fn encode_hex(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

pub fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("hex data must contain an even number of ASCII hexadecimal digits".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_nibble(pair[0])?;
            let low = decode_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hexadecimal digit".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_arbitrary_bytes() {
        let bytes = [0, 1, 15, 16, 127, 128, 255];
        assert_eq!(decode_hex(&encode_hex(&bytes)).unwrap(), bytes);
        assert!(decode_hex("0").is_err());
        assert!(decode_hex("gg").is_err());
    }
}
