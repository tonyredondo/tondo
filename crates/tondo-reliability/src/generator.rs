//! Reproducible generators shared by properties and fuzz smoke tests.

use serde::{Deserialize, Serialize};

/// Small deterministic generator with a stable algorithm and explicit seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generator {
    seed: u64,
    state: u64,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 7;
        self.state ^= self.state >> 9;
        self.state ^= self.state << 8;
        self.state
    }

    pub fn choose(&mut self, length: usize) -> usize {
        assert!(length > 0, "a generator cannot choose from an empty domain");
        (self.next_u64() as usize) % length
    }

    pub fn bytes(&mut self, maximum: usize) -> Vec<u8> {
        let length = self.choose(maximum.saturating_add(1).max(1));
        (0..length).map(|_| self.next_u64() as u8).collect()
    }

    pub fn identifier(&mut self, prefix: &str) -> String {
        format!("{prefix}{}", self.next_u64() % 1_000_000)
    }

    pub fn integer_expression(&mut self, depth: usize) -> IntegerExpression {
        if depth == 0 {
            return if self.choose(2) == 0 {
                IntegerExpression::Literal((self.next_u64() % 1_000) as i64)
            } else {
                IntegerExpression::Variable
            };
        }
        match self.choose(6) {
            0 => IntegerExpression::Literal((self.next_u64() % 1_000) as i64),
            1 => IntegerExpression::Variable,
            choice => IntegerExpression::Binary {
                operator: match choice {
                    2 => IntegerOperator::Add,
                    3 => IntegerOperator::Subtract,
                    4 => IntegerOperator::Multiply,
                    _ => IntegerOperator::BitXor,
                },
                left: Box::new(self.integer_expression(depth - 1)),
                right: Box::new(self.integer_expression(depth - 1)),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegerOperator {
    Add,
    Subtract,
    Multiply,
    BitXor,
}

impl IntegerOperator {
    fn spelling(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::BitXor => "^",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum IntegerExpression {
    Literal(i64),
    Variable,
    Binary {
        operator: IntegerOperator,
        left: Box<Self>,
        right: Box<Self>,
    },
}

impl IntegerExpression {
    pub fn evaluate(&self, variable: i64) -> Option<i64> {
        match self {
            Self::Literal(value) => Some(*value),
            Self::Variable => Some(variable),
            Self::Binary {
                operator,
                left,
                right,
            } => {
                let left = left.evaluate(variable)?;
                let right = right.evaluate(variable)?;
                match operator {
                    IntegerOperator::Add => left.checked_add(right),
                    IntegerOperator::Subtract => left.checked_sub(right),
                    IntegerOperator::Multiply => left.checked_mul(right),
                    IntegerOperator::BitXor => Some(left ^ right),
                }
            }
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Literal(value) => value.to_string(),
            Self::Variable => "value".into(),
            Self::Binary {
                operator,
                left,
                right,
            } => format!(
                "({} {} {})",
                left.render(),
                operator.spelling(),
                right.render()
            ),
        }
    }

    /// Deterministic structural candidates, smallest first.
    pub fn shrink(&self) -> Vec<Self> {
        match self {
            Self::Literal(0) | Self::Variable => Vec::new(),
            Self::Literal(_) => vec![Self::Literal(0)],
            Self::Binary { left, right, .. } => {
                let mut candidates = vec![Self::Literal(0), *left.clone(), *right.clone()];
                candidates.extend(left.shrink());
                candidates.extend(right.shrink());
                candidates
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureEvidence {
    pub format: String,
    pub seed: u64,
    pub case: u64,
    pub target: String,
    pub input_hex: String,
    pub minimized_hex: String,
    pub observation: String,
}

impl FailureEvidence {
    pub fn new(
        seed: u64,
        case: u64,
        target: impl Into<String>,
        input: &[u8],
        minimized: &[u8],
        observation: impl Into<String>,
    ) -> Self {
        Self {
            format: "tondo-test-failure/1".into(),
            seed,
            case,
            target: target.into(),
            input_hex: encode_hex(input),
            minimized_hex: encode_hex(minimized),
            observation: observation.into(),
        }
    }
}

pub fn minimize_bytes(mut input: Vec<u8>, still_fails: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    if !still_fails(&input) {
        return input;
    }
    let mut chunk = input.len().next_power_of_two() / 2;
    while chunk > 0 {
        let mut offset = 0;
        while offset < input.len() {
            let end = offset.saturating_add(chunk).min(input.len());
            let mut candidate = input.clone();
            candidate.drain(offset..end);
            if still_fails(&candidate) {
                input = candidate;
            } else {
                offset = end;
            }
        }
        chunk /= 2;
    }
    input
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_replays_the_same_generation() {
        let mut left = Generator::new(42);
        let mut right = Generator::new(42);
        assert_eq!(left.bytes(256), right.bytes(256));
        assert_eq!(left.integer_expression(5), right.integer_expression(5));
        assert_eq!(left.identifier("value"), right.identifier("value"));
    }

    #[test]
    fn byte_minimization_is_deterministic_and_preserves_failure() {
        let predicate = |bytes: &[u8]| bytes.windows(2).any(|pair| pair == b"XY");
        let input = b"prefixXXYsuffix".to_vec();
        let first = minimize_bytes(input.clone(), predicate);
        let second = minimize_bytes(input, predicate);
        assert_eq!(first, second);
        assert!(predicate(&first));
        assert_eq!(first, b"XY");
    }

    #[test]
    fn expression_shrinks_have_stable_order_and_preserve_rendering() {
        let expression = IntegerExpression::Binary {
            operator: IntegerOperator::Add,
            left: Box::new(IntegerExpression::Literal(10)),
            right: Box::new(IntegerExpression::Variable),
        };
        assert_eq!(expression.evaluate(5), Some(15));
        assert_eq!(expression.render(), "(10 + value)");
        assert_eq!(
            expression.shrink(),
            [
                IntegerExpression::Literal(0),
                IntegerExpression::Literal(10),
                IntegerExpression::Variable,
                IntegerExpression::Literal(0),
            ]
        );
    }
}
