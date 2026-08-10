//! Bounded formatting primitives shared by console, diagnostics and codecs.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatLimits {
    pub max_bytes: usize,
}

impl Default for FormatLimits {
    fn default() -> Self {
        Self {
            max_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatError {
    ResourceLimit,
    InvalidFormat,
}

pub trait Display {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError>;
}

#[derive(Debug, Clone)]
pub struct Builder {
    bytes: Vec<u8>,
    limits: FormatLimits,
}

impl Builder {
    pub fn new(limits: FormatLimits) -> Self {
        Self {
            bytes: Vec::new(),
            limits,
        }
    }

    pub fn append(&mut self, value: &str) -> Result<(), FormatError> {
        let next = self
            .bytes
            .len()
            .checked_add(value.len())
            .ok_or(FormatError::ResourceLimit)?;
        if next > self.limits.max_bytes {
            return Err(FormatError::ResourceLimit);
        }
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn finish(self) -> Result<String, FormatError> {
        String::from_utf8(self.bytes).map_err(|_| FormatError::InvalidFormat)
    }
}

impl Display for str {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
        output.append(self)
    }
}

impl Display for &str {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
        (*self).display(output)
    }
}

impl Display for String {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
        self.as_str().display(output)
    }
}

impl Display for i128 {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
        output.append(&self.to_string())
    }
}

impl Display for bool {
    fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
        output.append(if *self { "true" } else { "false" })
    }
}

pub fn format<T: Display>(value: &T, limits: FormatLimits) -> Result<String, FormatError> {
    let mut builder = Builder::new(limits);
    value.display(&mut builder)?;
    builder.finish()
}

pub fn join<T: Display>(
    values: &[T],
    separator: &str,
    limits: FormatLimits,
) -> Result<String, FormatError> {
    let mut builder = Builder::new(limits);
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            builder.append(separator)?;
        }
        value.display(&mut builder)?;
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingDisplay;

    impl Display for FailingDisplay {
        fn display(&self, output: &mut Builder) -> Result<(), FormatError> {
            output.append("prefix")?;
            Err(FormatError::InvalidFormat)
        }
    }

    #[test]
    fn builder_and_display_are_bounded() {
        let mut builder = Builder::new(FormatLimits { max_bytes: 5 });
        builder.append("tondo").unwrap();
        assert_eq!(builder.append("!"), Err(FormatError::ResourceLimit));
        assert_eq!(
            format(&"ok".to_owned(), FormatLimits::default()).unwrap(),
            "ok"
        );
        assert_eq!(format(&42_i128, FormatLimits::default()).unwrap(), "42");
        assert_eq!(format(&true, FormatLimits::default()).unwrap(), "true");
    }

    #[test]
    fn join_preserves_order_and_separator_limits() {
        let values = ["a", "b", "c"];
        assert_eq!(
            join(&values, ",", FormatLimits::default()).unwrap(),
            "a,b,c"
        );
        assert_eq!(
            join(&values, ",", FormatLimits { max_bytes: 3 }),
            Err(FormatError::ResourceLimit)
        );
        assert_eq!(join::<i128>(&[], ",", FormatLimits::default()).unwrap(), "");
    }

    #[test]
    fn limits_are_exact_and_rejected_appends_do_not_mutate_state() {
        for max_bytes in 0..=8 {
            let mut builder = Builder::new(FormatLimits { max_bytes });
            let mut expected = String::new();
            if max_bytes >= 5 {
                assert!(builder.append("tondo").is_ok());
                expected.push_str("tondo");
                if max_bytes >= 6 {
                    assert!(builder.append("!").is_ok());
                    expected.push('!');
                } else {
                    assert_eq!(builder.append("!"), Err(FormatError::ResourceLimit));
                }
            } else {
                assert_eq!(builder.append("tondo"), Err(FormatError::ResourceLimit));
                if max_bytes >= 1 {
                    assert!(builder.append("!").is_ok());
                    expected.push('!');
                } else {
                    assert_eq!(builder.append("!"), Err(FormatError::ResourceLimit));
                }
            }
            assert_eq!(builder.finish().unwrap(), expected);
        }
    }

    #[test]
    fn display_errors_propagate_without_exposing_partial_output() {
        assert_eq!(
            format(&FailingDisplay, FormatLimits::default()),
            Err(FormatError::InvalidFormat)
        );

        let mut builder = Builder::new(FormatLimits { max_bytes: 8 });
        assert_eq!(
            FailingDisplay.display(&mut builder),
            Err(FormatError::InvalidFormat)
        );
        assert_eq!(builder.finish().unwrap(), "prefix");
    }

    #[test]
    fn join_is_deterministic_at_every_materialization_boundary() {
        let values = ["a", "bb", "ccc"];
        let expected = "a|bb|ccc";
        for max_bytes in 0..=expected.len() {
            let result = join(&values, "|", FormatLimits { max_bytes });
            if max_bytes == expected.len() {
                assert_eq!(result.unwrap(), expected);
            } else {
                assert_eq!(result, Err(FormatError::ResourceLimit));
            }
        }
    }
}
