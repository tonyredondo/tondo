//! Exact test-stream bytes and their canonical text transport.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tondo_stdlib::serialization::{base64_decode, base64_encode};

/// Capture bytes until the attempt is complete. A UTF-8 scalar may span writes
/// or suite phases, so encoding is selected over the complete stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapturedOutput(Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutputEncoding {
    Utf8,
    Base64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputWire<'a> {
    encoding: OutputEncoding,
    data: Cow<'a, str>,
}

impl CapturedOutput {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Length in original bytes, independent of JSON or Base64 expansion.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The envelope must admit the complete write before appending any bytes.
    pub(crate) fn append(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    pub(crate) fn base64(&self) -> String {
        base64_encode(&self.0)
    }

    fn wire(&self) -> OutputWire<'_> {
        match self.as_text() {
            Some(text) => OutputWire {
                encoding: OutputEncoding::Utf8,
                data: Cow::Borrowed(text),
            },
            None => OutputWire {
                encoding: OutputEncoding::Base64,
                data: Cow::Owned(self.base64()),
            },
        }
    }
}

impl From<Vec<u8>> for CapturedOutput {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<String> for CapturedOutput {
    fn from(text: String) -> Self {
        Self(text.into_bytes())
    }
}

impl From<&str> for CapturedOutput {
    fn from(text: &str) -> Self {
        Self(text.as_bytes().to_vec())
    }
}

impl Serialize for CapturedOutput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CapturedOutput {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = OutputWire::deserialize(deserializer)?;
        match wire.encoding {
            OutputEncoding::Utf8 => Ok(Self(wire.data.into_owned().into_bytes())),
            OutputEncoding::Base64 => {
                let bytes = base64_decode(&wire.data)
                    .map_err(|_| serde::de::Error::custom("invalid output Base64"))?;
                if base64_encode(&bytes) != wire.data || std::str::from_utf8(&bytes).is_ok() {
                    return Err(serde::de::Error::custom(
                        "output requires canonical padded Base64 only for non-UTF-8 bytes",
                    ));
                }
                Ok(Self(bytes))
            }
        }
    }
}

/// Human reports label binary data; they never replace invalid bytes with U+FFFD.
impl fmt::Display for CapturedOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.as_text() {
            Some(text) => formatter.write_str(text),
            None => write!(formatter, "[base64] {}", self.base64()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_transport_preserves_exact_bytes_and_has_one_encoding() {
        for bytes in [
            Vec::new(),
            "split-é🦀\r\n\0".as_bytes().to_vec(),
            vec![0xff],
            (0..=255).collect(),
        ] {
            let output = CapturedOutput::from(bytes.clone());
            let wire = serde_json::to_vec(&output).unwrap();
            let restored: CapturedOutput = serde_json::from_slice(&wire).unwrap();
            assert_eq!(restored.as_bytes(), bytes);
            assert_eq!(serde_json::to_vec(&restored).unwrap(), wire);
            let value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
            assert_eq!(
                value["encoding"],
                if output.as_text().is_some() {
                    "utf8"
                } else {
                    "base64"
                }
            );
            assert_eq!(output.len(), bytes.len());
        }
        let mut split = CapturedOutput::default();
        split.append(&[0xe2]);
        assert_eq!(split.as_text(), None);
        split.append(&[0x82, 0xac]);
        assert_eq!(split.as_text(), Some("€"));
        assert_eq!(split.to_string(), "€");
        assert_eq!(CapturedOutput::from(vec![255]).to_string(), "[base64] /w==");
        assert_eq!(CapturedOutput::from("text".to_owned()).as_bytes(), b"text");
        assert_eq!(
            serde_json::to_string(&CapturedOutput::default()).unwrap(),
            r#"{"encoding":"utf8","data":""}"#
        );
    }

    #[test]
    fn output_transport_rejects_ambiguous_or_malformed_representations() {
        for wire in [
            r#""old text""#,
            r#"{"data":""}"#,
            r#"{"encoding":"utf8"}"#,
            r#"{"encoding":"UTF8","data":""}"#,
            r#"{"encoding":"utf8","data":"","extra":0}"#,
            r#"{"encoding":"utf8","encoding":"base64","data":""}"#,
            r#"{"encoding":"base64","data":""}"#,
            r#"{"encoding":"base64","data":"dGV4dA=="}"#,
            r#"{"encoding":"base64","data":"/w"}"#,
            r#"{"encoding":"base64","data":"/x=="}"#,
            r#"{"encoding":"base64","data":"_w=="}"#,
            r#"{"encoding":"base64","data":"/w==/w=="}"#,
            r#"{"encoding":"base64","data":"/w==\n"}"#,
        ] {
            assert!(
                serde_json::from_str::<CapturedOutput>(wire).is_err(),
                "{wire}"
            );
        }
    }
}
