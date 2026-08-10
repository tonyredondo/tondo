//! Host-independent lexical paths.  No operation in this module touches the
//! filesystem or resolves links.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    InvalidEncoding,
    EmptyComponent,
    Nul,
    ResourceLimit,
    Unsupported,
}

impl Path {
    pub fn from_string(value: &str) -> Result<Self, PathError> {
        Self::from_bytes(value.as_bytes())
    }

    pub fn from_bytes(value: &[u8]) -> Result<Self, PathError> {
        if value.contains(&0) {
            return Err(PathError::Nul);
        }
        if value.len() > 32 * 1024 {
            return Err(PathError::ResourceLimit);
        }
        Ok(Self {
            bytes: value.to_vec(),
        })
    }

    pub fn join(&self, component: &str) -> Result<Self, PathError> {
        if component.is_empty() {
            return Err(PathError::EmptyComponent);
        }
        let component = Self::from_string(component)?;
        if component.bytes.contains(&b'/') {
            return Err(PathError::EmptyComponent);
        }
        let mut bytes = self.bytes.clone();
        if !bytes.is_empty() && !bytes.ends_with(b"/") {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(&component.bytes);
        Self::from_bytes(&bytes)
    }

    pub fn parent(&self) -> Option<Self> {
        let bytes = self.bytes.as_slice();
        let end = bytes.iter().rposition(|byte| *byte == b'/')?;
        if end == 0 && bytes.first() == Some(&b'/') {
            return Self::from_bytes(b"/").ok();
        }
        Self::from_bytes(&bytes[..end]).ok()
    }

    pub fn file_name(&self) -> Option<&str> {
        let name = self.bytes.rsplit(|byte| *byte == b'/').next()?;
        (!name.is_empty())
            .then(|| std::str::from_utf8(name).ok())
            .flatten()
    }

    pub fn extension(&self) -> Option<&str> {
        let name = self.file_name()?;
        let dot = name.rfind('.')?;
        if dot == 0 {
            return None;
        }
        Some(&name[dot + 1..])
    }

    pub fn is_absolute(&self) -> bool {
        self.bytes.first() == Some(&b'/')
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns an owned byte snapshot without applying Unicode conversion or
    /// lexical normalization.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    pub fn to_string(&self) -> Result<String, PathError> {
        String::from_utf8(self.bytes.clone()).map_err(|_| PathError::InvalidEncoding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounded_bytes(seed: &mut u32, length: usize) -> Vec<u8> {
        (0..length)
            .map(|_| {
                *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                // Keep the generated corpus free of NUL and slash so it can be
                // used as a single component while still exercising arbitrary
                // native bytes, including invalid UTF-8.
                let byte = (*seed >> 24) as u8;
                match byte {
                    0 | b'/' => b'x',
                    value => value,
                }
            })
            .collect()
    }

    #[test]
    fn lexical_operations_do_not_normalize_components() {
        let path = Path::from_string("a/../b.txt").unwrap();
        assert_eq!(path.parent().unwrap().to_string().unwrap(), "a/..");
        assert_eq!(path.file_name(), Some("b.txt"));
        assert_eq!(path.extension(), Some("txt"));
        assert_eq!(
            path.join("next").unwrap().to_string().unwrap(),
            "a/../b.txt/next"
        );
        assert!(!path.is_absolute());
    }

    #[test]
    fn roots_and_hidden_files_have_closed_results() {
        let root = Path::from_string("/tmp").unwrap();
        assert_eq!(root.parent().unwrap().to_string().unwrap(), "/");
        assert_eq!(Path::from_string(".config").unwrap().extension(), None);
        assert_eq!(Path::from_string("file.").unwrap().extension(), Some(""));
        assert!(Path::from_string("").unwrap().is_empty());
    }

    #[test]
    fn invalid_components_and_native_bytes_are_preserved() {
        assert_eq!(Path::from_bytes(b"a\0b"), Err(PathError::Nul));
        assert_eq!(
            Path::from_bytes(&[0xff]).unwrap().to_string(),
            Err(PathError::InvalidEncoding)
        );
        let path = Path::from_bytes(&[0xff, b'/', b'x']).unwrap();
        assert_eq!(path.as_bytes(), &[0xff, b'/', b'x']);
        assert_eq!(path.join(""), Err(PathError::EmptyComponent));
        assert_eq!(path.join("a/b"), Err(PathError::EmptyComponent));
    }

    #[test]
    fn utf8_and_native_bytes_round_trip_without_normalization() {
        let composed = Path::from_string("café/α").unwrap();
        assert_eq!(composed.to_string().unwrap(), "café/α");
        assert_eq!(composed.to_bytes(), composed.as_bytes());

        let decomposed = Path::from_string("cafe\u{301}/\u{03b1}").unwrap();
        assert_ne!(composed.as_bytes(), decomposed.as_bytes());
        assert_eq!(decomposed.to_string().unwrap(), "cafe\u{301}/\u{03b1}");

        let native = Path::from_bytes(&[0xff, 0xfe, b'/', 0x80]).unwrap();
        assert_eq!(native.as_bytes(), &[0xff, 0xfe, b'/', 0x80]);
        assert_eq!(native.to_bytes(), native.as_bytes());
        assert_eq!(native.to_string(), Err(PathError::InvalidEncoding));
    }

    #[test]
    fn limits_are_exact_and_rejections_are_atomic() {
        let exact = vec![b'a'; 32 * 1024];
        assert_eq!(
            Path::from_bytes(&exact).unwrap().as_bytes(),
            exact.as_slice()
        );
        assert_eq!(
            Path::from_bytes(&vec![b'a'; 32 * 1024 + 1]),
            Err(PathError::ResourceLimit)
        );

        let base = Path::from_string("base").unwrap();
        assert_eq!(base.join(""), Err(PathError::EmptyComponent));
        assert_eq!(
            base.join("nested/component"),
            Err(PathError::EmptyComponent)
        );
        let oversized = "x".repeat(32 * 1024);
        assert_eq!(base.join(&oversized), Err(PathError::ResourceLimit));
        assert_eq!(base.to_string().unwrap(), "base");
    }

    #[test]
    fn lexical_boundaries_have_deterministic_results() {
        for (input, parent, file_name, extension, absolute) in [
            ("", None, None, None, false),
            ("a", None, Some("a"), None, false),
            ("a/", Some("a"), None, None, false),
            ("/", Some("/"), None, None, true),
            ("/tmp", Some("/"), Some("tmp"), None, true),
            (".config", None, Some(".config"), None, false),
            ("file.", None, Some("file."), Some(""), false),
            (
                "archive.tar.gz",
                None,
                Some("archive.tar.gz"),
                Some("gz"),
                false,
            ),
        ] {
            let path = Path::from_string(input).unwrap();
            assert_eq!(
                path.parent()
                    .as_ref()
                    .and_then(|value| value.to_string().ok())
                    .as_deref(),
                parent
            );
            assert_eq!(path.file_name(), file_name);
            assert_eq!(path.extension(), extension);
            assert_eq!(path.is_absolute(), absolute);
        }
    }

    #[test]
    fn bounded_native_corpus_preserves_bytes_and_never_normalizes() {
        let mut seed = 0x544f_4e44;
        for length in 0..=96 {
            let bytes = bounded_bytes(&mut seed, length);
            let path = Path::from_bytes(&bytes).unwrap();
            assert_eq!(path.as_bytes(), bytes.as_slice());
            assert_eq!(path.to_bytes(), bytes.as_slice());
            assert_eq!(path.is_empty(), bytes.is_empty());

            if std::str::from_utf8(&bytes).is_ok() {
                assert_eq!(path.to_string().unwrap().as_bytes(), bytes.as_slice());
            } else {
                assert_eq!(path.to_string(), Err(PathError::InvalidEncoding));
            }

            let component = if bytes.is_empty() {
                "x".to_owned()
            } else {
                String::from_utf8(
                    bytes
                        .iter()
                        .map(|byte| if byte.is_ascii_graphic() { *byte } else { b'x' })
                        .collect(),
                )
                .unwrap()
            };
            let joined = path.join(&component).unwrap();
            let mut expected = bytes.clone();
            if !expected.is_empty() {
                expected.push(b'/');
            }
            expected.extend_from_slice(component.as_bytes());
            assert_eq!(joined.as_bytes(), expected.as_slice());
        }
    }
}
