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

    pub fn to_string(&self) -> Result<String, PathError> {
        String::from_utf8(self.bytes.clone()).map_err(|_| PathError::InvalidEncoding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
