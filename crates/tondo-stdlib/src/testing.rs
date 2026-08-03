//! Deterministic, allocation-bounded helpers used by `std.testing`.
//!
//! The runner remains the owner of lifecycle and failure control.  This module
//! contains only pure value helpers so production code cannot acquire a test
//! envelope by importing it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLimits {
    pub max_input_bytes: usize,
    pub max_lines: usize,
    pub max_hunks: usize,
    pub max_output_bytes: usize,
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1 << 20,
            max_lines: 16_384,
            max_hunks: 4_096,
            max_output_bytes: 1 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDiffHunk {
    Equal(String),
    Delete(String),
    Insert(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    pub equal: bool,
    pub hunks: Vec<TextDiffHunk>,
    pub expected_bytes: usize,
    pub actual_bytes: usize,
    pub truncated: bool,
}

impl TextDiff {
    pub fn render(&self) -> String {
        let mut output = String::from("--- expected\n+++ actual\n");
        for hunk in &self.hunks {
            let (prefix, text) = match hunk {
                TextDiffHunk::Equal(text) => (' ', text),
                TextDiffHunk::Delete(text) => ('-', text),
                TextDiffHunk::Insert(text) => ('+', text),
            };
            for line in text.split_inclusive('\n') {
                output.push(prefix);
                output.push_str(line);
            }
            if !text.ends_with('\n') && !text.is_empty() {
                output.push('\n');
            }
        }
        if self.truncated {
            output.push_str("... truncated ...\n");
        }
        output
    }
}

pub fn diff_text(expected: &str, actual: &str) -> TextDiff {
    diff_text_with_limits(expected, actual, DiffLimits::default())
}

pub fn diff_text_with_limits(expected: &str, actual: &str, limits: DiffLimits) -> TextDiff {
    let expected_bytes = expected.len();
    let actual_bytes = actual.len();
    if expected == actual {
        return TextDiff {
            equal: true,
            hunks: Vec::new(),
            expected_bytes,
            actual_bytes,
            truncated: false,
        };
    }
    let mut truncated =
        expected_bytes > limits.max_input_bytes || actual_bytes > limits.max_input_bytes;
    let expected_lines = bounded_lines(
        expected,
        limits.max_input_bytes,
        limits.max_lines,
        &mut truncated,
    );
    let actual_lines = bounded_lines(
        actual,
        limits.max_input_bytes,
        limits.max_lines,
        &mut truncated,
    );
    let rows = expected_lines.len().min(limits.max_lines);
    let cols = actual_lines.len().min(limits.max_lines);
    let mut table = vec![vec![0usize; cols + 1]; rows + 1];
    for row in (0..rows).rev() {
        for col in (0..cols).rev() {
            table[row][col] = if expected_lines[row] == actual_lines[col] {
                table[row + 1][col + 1] + 1
            } else {
                table[row + 1][col].max(table[row][col + 1])
            };
        }
    }
    let mut row = 0;
    let mut col = 0;
    let mut hunks = Vec::new();
    while row < rows || col < cols {
        if row < rows && col < cols && expected_lines[row] == actual_lines[col] {
            push_hunk(
                &mut hunks,
                TextDiffHunk::Equal(expected_lines[row].to_owned()),
            );
            row += 1;
            col += 1;
        } else if col < cols && (row == rows || table[row][col + 1] > table[row + 1][col]) {
            push_hunk(
                &mut hunks,
                TextDiffHunk::Insert(actual_lines[col].to_owned()),
            );
            col += 1;
        } else if row < rows {
            push_hunk(
                &mut hunks,
                TextDiffHunk::Delete(expected_lines[row].to_owned()),
            );
            row += 1;
        }
        if hunks.len() > limits.max_hunks {
            hunks.truncate(limits.max_hunks);
            truncated = true;
            break;
        }
    }
    let mut result = TextDiff {
        equal: false,
        hunks,
        expected_bytes,
        actual_bytes,
        truncated,
    };
    if result.render().len() > limits.max_output_bytes {
        result.truncated = true;
        while result.render().len() > limits.max_output_bytes && !result.hunks.is_empty() {
            result.hunks.pop();
        }
    }
    result
}

fn bounded_lines<'a>(
    text: &'a str,
    max_bytes: usize,
    max_lines: usize,
    truncated: &mut bool,
) -> Vec<&'a str> {
    let end = text.len().min(max_bytes);
    let prefix = &text[..text.floor_char_boundary(end)];
    let mut lines = prefix
        .split_inclusive('\n')
        .take(max_lines)
        .collect::<Vec<_>>();
    if lines.len() < max_lines && !prefix.is_empty() && !prefix.ends_with('\n') {
        // split_inclusive already returned the final unterminated line.
    }
    if prefix.len() != text.len() || lines.len() == max_lines && prefix.lines().count() > max_lines
    {
        *truncated = true;
    }
    lines.shrink_to_fit();
    lines
}

fn push_hunk(hunks: &mut Vec<TextDiffHunk>, next: TextDiffHunk) {
    match (hunks.last_mut(), next) {
        (Some(TextDiffHunk::Equal(previous)), TextDiffHunk::Equal(text)) => {
            previous.push_str(&text)
        }
        (Some(TextDiffHunk::Delete(previous)), TextDiffHunk::Delete(text)) => {
            previous.push_str(&text)
        }
        (Some(TextDiffHunk::Insert(previous)), TextDiffHunk::Insert(text)) => {
            previous.push_str(&text)
        }
        (_, next) => hunks.push(next),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatTolerance {
    pub absolute: f64,
    pub relative: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatToleranceError {
    Negative,
    NonFinite,
    Overflow,
}

impl FloatTolerance {
    pub fn new(absolute: f64, relative: f64) -> Result<Self, FloatToleranceError> {
        if !absolute.is_finite() || !relative.is_finite() {
            return Err(FloatToleranceError::NonFinite);
        }
        if absolute < 0.0 || relative < 0.0 {
            return Err(FloatToleranceError::Negative);
        }
        if absolute + relative == f64::INFINITY {
            return Err(FloatToleranceError::Overflow);
        }
        Ok(Self { absolute, relative })
    }

    pub fn is_near(self, expected: f64, actual: f64) -> bool {
        if expected.is_nan() || actual.is_nan() {
            return false;
        }
        if expected == actual {
            return true;
        }
        if !expected.is_finite() || !actual.is_finite() {
            return false;
        }
        let delta = (actual - expected).abs();
        if delta <= self.absolute {
            return true;
        }
        let scale = expected.abs().max(actual.abs());
        scale > 0.0 && delta / scale <= self.relative
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationId {
    pub seed: u64,
    pub case_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationError {
    InvalidBounds,
    LimitExceeded,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generator {
    seed: u64,
    case_index: u64,
    state: u64,
    draws: u64,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self::for_case(seed, 0)
    }

    pub fn for_case(seed: u64, case_index: u64) -> Self {
        let mut state = (seed ^ 0x9e37_79b9_7f4a_7c15)
            .wrapping_add(case_index.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        if state == 0 {
            state = 0x6a09_e667_f3bc_c909;
        }
        Self {
            seed,
            case_index,
            state,
            draws: 0,
        }
    }

    pub const fn id(self) -> GenerationId {
        GenerationId {
            seed: self.seed,
            case_index: self.case_index,
        }
    }

    pub const fn draw_count(self) -> u64 {
        self.draws
    }

    pub fn next_u64(&mut self) -> Result<u64, GenerationError> {
        self.draws = self
            .draws
            .checked_add(1)
            .ok_or(GenerationError::Exhausted)?;
        self.state ^= self.state << 7;
        self.state ^= self.state >> 9;
        self.state ^= self.state << 8;
        Ok(self.state)
    }

    pub fn next_bool(&mut self) -> Result<bool, GenerationError> {
        Ok(self.next_u64()? & 1 == 1)
    }

    pub fn next_int(&mut self, minimum: i128, maximum: i128) -> Result<i128, GenerationError> {
        if minimum > maximum {
            return Err(GenerationError::InvalidBounds);
        }
        let span = (maximum as u128)
            .wrapping_sub(minimum as u128)
            .wrapping_add(1);
        if span == 0 {
            let high = u128::from(self.next_u64()?);
            let low = u128::from(self.next_u64()?);
            return Ok(i128::from_ne_bytes(((high << 64) | low).to_ne_bytes()));
        }
        let threshold = span.wrapping_neg() % span;
        loop {
            let high = u128::from(self.next_u64()?);
            let low = u128::from(self.next_u64()?);
            let sample = (high << 64) | low;
            if sample >= threshold {
                return Ok(minimum.wrapping_add((sample % span) as i128));
            }
        }
    }

    pub fn next_bytes(&mut self, maximum_length: usize) -> Result<Vec<u8>, GenerationError> {
        let length = usize::try_from(self.next_int(0, maximum_length as i128)?)
            .map_err(|_| GenerationError::LimitExceeded)?;
        let mut output = Vec::with_capacity(length);
        for _ in 0..length {
            output.push(self.next_u64()? as u8);
        }
        Ok(output)
    }

    pub fn next_text(&mut self, maximum_bytes: usize) -> Result<String, GenerationError> {
        let length = usize::try_from(self.next_int(0, maximum_bytes as i128)?)
            .map_err(|_| GenerationError::LimitExceeded)?;
        let mut output = String::with_capacity(length);
        while output.len() < length {
            let mut scalar = (self.next_u64()? % 0x11_0000) as u32;
            if (0xd800..=0xdfff).contains(&scalar) {
                scalar += 0x800;
            }
            let character = char::from_u32(scalar).ok_or(GenerationError::LimitExceeded)?;
            if output.len() + character.len_utf8() > length {
                break;
            }
            output.push(character);
        }
        Ok(output)
    }
}

pub trait Shrink: Clone + PartialEq {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError>;
}

impl Shrink for i128 {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        let mut value = *self;
        let mut result = Vec::new();
        while value != 0 && result.len() < limit {
            value /= 2;
            if !result.contains(&value) {
                result.push(value);
            }
        }
        Ok(result)
    }
}

impl Shrink for String {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for length in 0..=self.len() {
            if result.len() == limit {
                break;
            }
            let candidate = self[..self.floor_char_boundary(length)].to_owned();
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        Ok(result)
    }
}

impl Shrink for f64 {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 || self.is_nan() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for candidate in [0.0, *self / 2.0, -*self / 2.0] {
            if result.len() == limit {
                break;
            }
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        Ok(result)
    }
}

impl<T> Shrink for Vec<T>
where
    T: Shrink + Clone + PartialEq,
{
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for length in 0..=self.len() {
            if result.len() == limit {
                break;
            }
            let candidate = self[..length].to_vec();
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        for index in 0..self.len() {
            for value in self[index].candidates(limit.saturating_sub(result.len()))? {
                if result.len() == limit {
                    break;
                }
                let mut candidate = self.clone();
                candidate[index] = value;
                if !result.contains(&candidate) {
                    result.push(candidate);
                }
            }
        }
        Ok(result)
    }
}

pub fn shrink<T: Shrink>(value: &T, limit: usize) -> Result<Vec<T>, GenerationError> {
    value.candidates(limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_is_stable_and_merges_adjacent_hunks() {
        let diff = diff_text("same\nold\n", "same\nnew\n");
        assert!(!diff.equal);
        assert_eq!(
            diff.hunks,
            vec![
                TextDiffHunk::Equal("same\n".into()),
                TextDiffHunk::Delete("old\n".into()),
                TextDiffHunk::Insert("new\n".into()),
            ]
        );
        assert_eq!(
            diff.render(),
            "--- expected\n+++ actual\n same\n-old\n+new\n"
        );
        assert_eq!(diff_text("x", "x").hunks, Vec::new());
    }

    #[test]
    fn diff_limits_never_hide_that_output_was_truncated() {
        let limits = DiffLimits {
            max_input_bytes: 3,
            max_lines: 1,
            max_hunks: 1,
            max_output_bytes: 8,
        };
        let diff = diff_text_with_limits("abcdef", "uvwxyz", limits);
        assert!(diff.truncated);
        assert!(diff.render().contains("truncated"));
    }

    #[test]
    fn tolerance_follows_nan_infinity_and_relative_rules() {
        let tolerance = FloatTolerance::new(0.01, 0.1).unwrap();
        assert!(tolerance.is_near(10.0, 10.5));
        assert!(!tolerance.is_near(f64::NAN, f64::NAN));
        assert!(tolerance.is_near(f64::INFINITY, f64::INFINITY));
        assert_eq!(
            FloatTolerance::new(-1.0, 0.0),
            Err(FloatToleranceError::Negative)
        );
        assert_eq!(
            FloatTolerance::new(f64::INFINITY, 0.0),
            Err(FloatToleranceError::NonFinite)
        );
    }

    #[test]
    fn generator_replays_and_respects_bounds() {
        let mut left = Generator::for_case(7, 3);
        let mut right = Generator::for_case(7, 3);
        assert_eq!(left.id(), right.id());
        for _ in 0..10 {
            assert_eq!(left.next_u64(), right.next_u64());
        }
        assert!(left.next_int(3, 2).is_err());
        assert!(left.next_int(-2, 2).unwrap().abs() <= 2);
        let mut text_left = Generator::for_case(7, 3);
        let mut text_right = Generator::for_case(7, 3);
        assert_eq!(
            text_left.next_text(4).unwrap(),
            text_right.next_text(4).unwrap()
        );
        for (seed, case_index) in [(0, 0), (u64::MAX, u64::MAX), (7, 3)] {
            let text = Generator::for_case(seed, case_index).next_text(64).unwrap();
            assert!(text.len() <= 64);
            assert!(text.is_char_boundary(text.len()));
            assert!(
                text.chars()
                    .all(|character| !(0xd800..=0xdfff).contains(&(character as u32)))
            );
        }
        assert_eq!(Generator::for_case(7, 3).next_int(4, 4), Ok(4));
        let mut full_range = Generator::for_case(7, 3);
        assert!(full_range.next_int(i128::MIN, i128::MAX).is_ok());
        assert_eq!(full_range.draw_count(), 2);
    }

    #[test]
    fn shrink_candidates_are_bounded_and_deterministic() {
        assert_eq!(shrink(&10_i128, 3).unwrap(), vec![5, 2, 1]);
        assert_eq!(
            shrink(&"tondo".to_owned(), 3).unwrap(),
            vec!["".to_owned(), "t".to_owned(), "to".to_owned()]
        );
        assert_eq!(shrink(&vec![10_i128, 4], 3).unwrap()[0], Vec::<i128>::new());
        assert!(shrink(&10_i128, 0).unwrap().is_empty());
    }
}
