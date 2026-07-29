//! Deterministic lowering from verified MIR to the VM's typed bytecode.

use std::error::Error;
use std::fmt;

use crate::source::Span;

mod lower;

pub use lower::lower_to_bytecode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeLoweringLimits {
    pub max_types: u32,
    pub max_nominals: u32,
    pub max_callables: u32,
    pub max_constants: u32,
    pub max_functions: u32,
    pub max_slots_per_function: u32,
    pub max_blocks_per_function: u32,
    pub max_instructions_per_function: u32,
    pub max_spans_per_function: u32,
    pub max_generic_instantiations: u32,
    pub max_verification_steps: u64,
}

impl Default for BytecodeLoweringLimits {
    fn default() -> Self {
        Self {
            max_types: 4_000_000,
            max_nominals: 1_000_000,
            max_callables: 1_000_000,
            max_constants: 1_000_000,
            max_functions: 100_000,
            max_slots_per_function: 1_000_000,
            max_blocks_per_function: 1_000_000,
            max_instructions_per_function: 4_000_000,
            max_spans_per_function: 4_000_000,
            max_generic_instantiations: 1_000_000,
            max_verification_steps: 32_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BytecodeError {
    NodeLimit {
        span: Option<Span>,
        resource: &'static str,
    },
    Construction {
        context: String,
        message: String,
    },
    VerificationLimit {
        resource: &'static str,
    },
    Invariant(tondo_vm::bytecode::BytecodeVerificationError),
}

impl BytecodeError {
    fn construction(context: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Construction {
            context: context.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeLimit { span, resource } => {
                write!(formatter, "bytecode {resource} limit exceeded")?;
                if let Some(span) = span {
                    write!(
                        formatter,
                        " in {} at byte {}",
                        span.file(),
                        span.range().start()
                    )?;
                }
                Ok(())
            }
            Self::Construction { context, message } => {
                write!(
                    formatter,
                    "bytecode construction failed in {context}: {message}"
                )
            }
            Self::VerificationLimit { resource } => {
                write!(formatter, "bytecode {resource} limit exceeded")
            }
            Self::Invariant(error) => error.fmt(formatter),
        }
    }
}

impl Error for BytecodeError {}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{
        LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput, SourceOrigin, TextRange,
    };

    #[test]
    fn lowering_limits_and_error_text_cover_the_closed_public_boundary() {
        assert_eq!(
            BytecodeLoweringLimits::default(),
            BytecodeLoweringLimits {
                max_types: 4_000_000,
                max_nominals: 1_000_000,
                max_callables: 1_000_000,
                max_constants: 1_000_000,
                max_functions: 100_000,
                max_slots_per_function: 1_000_000,
                max_blocks_per_function: 1_000_000,
                max_instructions_per_function: 4_000_000,
                max_spans_per_function: 4_000_000,
                max_generic_instantiations: 1_000_000,
                max_verification_steps: 32_000_000,
            }
        );

        assert_eq!(
            BytecodeError::NodeLimit {
                span: None,
                resource: "type",
            }
            .to_string(),
            "bytecode type limit exceeded"
        );
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::new(
                SourceId::new("test:bytecode-error").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                SourceOrigin::Virtual,
                Arc::<[u8]>::from(&b"123"[..]),
            ))
            .unwrap();
        let span = sources.span(file, TextRange::new(2, 3).unwrap()).unwrap();
        assert_eq!(
            BytecodeError::NodeLimit {
                span: Some(span),
                resource: "function",
            }
            .to_string(),
            "bytecode function limit exceeded in 0 at byte 2"
        );
        assert_eq!(
            BytecodeError::construction("function `main`", "missing block").to_string(),
            "bytecode construction failed in function `main`: missing block"
        );
        assert_eq!(
            BytecodeError::VerificationLimit {
                resource: "dataflow step",
            }
            .to_string(),
            "bytecode dataflow step limit exceeded"
        );
    }
}
