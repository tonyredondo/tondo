//! Unicode 16 scalar regex kernel with an ordered, step-bounded Thompson NFA.
//!
//! This Rust API does not register public Tondo calls, a VM bridge or native ABI.
//! Iteration borrows both the immutable program and input; errors are per item.

mod engine;
#[cfg(test)]
mod performance;
mod syntax;

use std::{fmt, sync::Arc};

use engine::SearchMode;
use sha2::{Digest, Sha256};

pub const UNICODE_VERSION: &str = "16.0.0";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RegexOptions {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_newline: bool,
    pub crlf: bool,
    pub ungreedy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexLimits {
    pub max_pattern_bytes: usize,
    pub max_syntax_depth: usize,
    pub max_capture_groups: usize,
    pub max_class_ranges: usize,
    pub max_repeat: usize,
    pub max_program_states: usize,
    pub max_input_bytes: usize,
    pub max_steps: usize,
    pub max_matches: usize,
    pub max_output_bytes: usize,
    pub max_replacement_bytes: usize,
    /// Logical kernel storage budget in bytes, not RSS or a VM allocator limit.
    pub vm_heap: usize,
}

impl Default for RegexLimits {
    fn default() -> Self {
        Self {
            max_pattern_bytes: 65_536,
            max_syntax_depth: 256,
            max_capture_groups: 256,
            max_class_ranges: 262_144,
            max_repeat: 1_000_000,
            max_program_states: 1_000_000,
            max_input_bytes: 16_777_216,
            max_steps: 100_000_000,
            max_matches: 1_000_000,
            max_output_bytes: 67_108_864,
            max_replacement_bytes: 65_536,
            vm_heap: 134_217_728,
        }
    }
}

impl RegexLimits {
    fn fields(self) -> [(&'static str, usize); 12] {
        [
            ("max_pattern_bytes", self.max_pattern_bytes),
            ("max_syntax_depth", self.max_syntax_depth),
            ("max_capture_groups", self.max_capture_groups),
            ("max_class_ranges", self.max_class_ranges),
            ("max_repeat", self.max_repeat),
            ("max_program_states", self.max_program_states),
            ("max_input_bytes", self.max_input_bytes),
            ("max_steps", self.max_steps),
            ("max_matches", self.max_matches),
            ("max_output_bytes", self.max_output_bytes),
            ("max_replacement_bytes", self.max_replacement_bytes),
            ("vm_heap", self.vm_heap),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexErrorKind {
    InvalidSyntax,
    UnexpectedEnd,
    InvalidEscape,
    InvalidUnicodeScalar,
    InvalidUnicodeProperty,
    InvalidClass,
    InvalidRange,
    InvalidQuantifier,
    InvalidCaptureName,
    DuplicateCaptureName,
    UnsupportedFeature,
    PatternLimitExceeded,
    ProgramLimitExceeded,
    InputLimitExceeded,
    StepLimitExceeded,
    MatchLimitExceeded,
    ReplacementLimitExceeded,
    InvalidReplacement,
    OutputLimitExceeded,
    InvalidBoundary,
    OutOfMemory,
    NoProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegexPhase {
    Compile,
    Match,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexSpan {
    pub start: usize,
    pub end: usize,
}

impl RegexSpan {
    pub fn slice(self, input: &str) -> Result<String, RegexError> {
        let value = input.get(self.start..self.end).ok_or_else(|| {
            RegexError::at(
                RegexErrorKind::InvalidBoundary,
                RegexPhase::Match,
                self.start,
                self.end,
            )
        })?;
        copy_text(value, RegexPhase::Match, self.start)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexError {
    pub kind: RegexErrorKind,
    pub phase: RegexPhase,
    pub offset: usize,
    pub span: Option<RegexSpan>,
    pub limit: Option<&'static str>,
}

impl RegexError {
    fn at(kind: RegexErrorKind, phase: RegexPhase, start: usize, end: usize) -> Self {
        Self {
            kind,
            phase,
            offset: start,
            span: Some(RegexSpan { start, end }),
            limit: None,
        }
    }
    fn limited(
        kind: RegexErrorKind,
        phase: RegexPhase,
        offset: usize,
        limit: &'static str,
    ) -> Self {
        Self {
            limit: Some(limit),
            ..Self::at(kind, phase, offset, offset)
        }
    }
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "regex {:?}: {:?} at byte {}",
            self.phase, self.kind, self.offset
        )
    }
}
impl std::error::Error for RegexError {}

pub type RegexCapture = RegexSpan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexMatch {
    pub span: RegexSpan,
    captures: Vec<Option<RegexCapture>>,
    names: Arc<Vec<Option<String>>>,
}

impl RegexMatch {
    pub fn capture(&self, index: usize) -> Option<RegexCapture> {
        self.captures.get(index).copied().flatten()
    }
    pub fn capture_name(&self, name: &str) -> Option<RegexCapture> {
        self.names
            .iter()
            .position(|item| item.as_deref() == Some(name))
            .and_then(|index| self.capture(index))
    }
}

#[derive(Debug)]
pub struct Regex {
    pattern: String,
    options: RegexOptions,
    limits: RegexLimits,
    names: Arc<Vec<Option<String>>>,
    program: engine::Program,
    fingerprint: [u8; 32],
}

impl Regex {
    pub fn compile(
        pattern: &str,
        options: RegexOptions,
        limits: RegexLimits,
    ) -> Result<Self, RegexError> {
        for (id, value) in limits.fields() {
            if value == 0 || value as u128 > i64::MAX as u128 {
                return Err(RegexError::limited(
                    RegexErrorKind::PatternLimitExceeded,
                    RegexPhase::Compile,
                    0,
                    id,
                ));
            }
        }
        let (hir, names, word) = syntax::parse(pattern, options, limits)?;
        let program = engine::Program::compile(&hir, word, limits)?;
        if program
            .logical_storage()
            .saturating_add(Self::metadata_storage(pattern, &names))
            > limits.vm_heap
        {
            return Err(RegexError::limited(
                RegexErrorKind::OutOfMemory,
                RegexPhase::Compile,
                0,
                "vm_heap",
            ));
        }
        let mut hash = Sha256::new();
        hash.update(b"tondo-regex-kernel/1\0");
        hash.update(UNICODE_VERSION.as_bytes());
        hash.update((pattern.len() as u64).to_le_bytes());
        hash.update(pattern.as_bytes());
        hash.update([
            options.case_insensitive as u8,
            options.multi_line as u8,
            options.dot_matches_newline as u8,
            options.crlf as u8,
            options.ungreedy as u8,
        ]);
        for (_, value) in limits.fields() {
            hash.update((value as u64).to_le_bytes());
        }
        Ok(Self {
            pattern: copy_text(pattern, RegexPhase::Compile, 0)?,
            options,
            limits,
            names: Arc::new(names),
            program,
            fingerprint: hash.finalize().into(),
        })
    }
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
    /// Number of explicitly capturing groups, excluding capture zero.
    pub fn capture_count(&self) -> usize {
        self.names.len() - 1
    }
    /// Named captures in opening-parenthesis order, without copying their names.
    pub fn capture_names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().filter_map(|name| name.as_deref())
    }
    pub fn options(&self) -> RegexOptions {
        self.options
    }
    pub fn limits(&self) -> RegexLimits {
        self.limits
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
    pub fn program_states(&self) -> usize {
        self.program.state_count()
    }
    fn check_input(&self, input: &str, phase: RegexPhase) -> Result<(), RegexError> {
        if input.len() > self.limits.max_input_bytes {
            return Err(RegexError::limited(
                RegexErrorKind::InputLimitExceeded,
                phase,
                0,
                "max_input_bytes",
            ));
        }
        Ok(())
    }
    fn metadata_storage(pattern: &str, names: &[Option<String>]) -> usize {
        pattern
            .len()
            .saturating_add(
                names
                    .len()
                    .saturating_mul(std::mem::size_of::<Option<String>>()),
            )
            .saturating_add(
                names
                    .iter()
                    .filter_map(|name| name.as_ref())
                    .map(String::len)
                    .sum::<usize>(),
            )
    }
    fn search(
        &self,
        input: &str,
        mode: SearchMode,
        steps: &mut usize,
        phase: RegexPhase,
        reserved: usize,
    ) -> Result<Option<RegexMatch>, RegexError> {
        let mut limits = self.limits;
        limits.vm_heap = limits.vm_heap.saturating_sub(
            reserved.saturating_add(Self::metadata_storage(&self.pattern, &self.names)),
        );
        self.program
            .search(input, mode, self.names.clone(), limits, steps, phase)
    }
    pub fn is_match(&self, input: &str) -> Result<bool, RegexError> {
        Ok(self.find(input)?.is_some())
    }
    pub fn is_full_match(&self, input: &str) -> Result<bool, RegexError> {
        self.check_input(input, RegexPhase::Match)?;
        Ok(self
            .search(input, SearchMode::Full, &mut 0, RegexPhase::Match, 0)?
            .is_some())
    }
    pub fn find(&self, input: &str) -> Result<Option<RegexMatch>, RegexError> {
        self.check_input(input, RegexPhase::Match)?;
        self.search(input, SearchMode::At(0), &mut 0, RegexPhase::Match, 0)
    }
    pub fn find_all<'a>(&'a self, input: &'a str) -> Result<RegexFindIterator<'a>, RegexError> {
        self.check_input(input, RegexPhase::Match)?;
        Ok(RegexFindIterator {
            regex: self,
            input,
            cursor: Some(0),
            steps: 0,
            matches: 0,
        })
    }
    pub fn replace(&self, input: &str, replacement: &str) -> Result<String, RegexError> {
        self.replace_impl(input, replacement, false)
    }
    pub fn replace_all(&self, input: &str, replacement: &str) -> Result<String, RegexError> {
        self.replace_impl(input, replacement, true)
    }
}

/// A lazy cursor with a cumulative budget. One error makes it permanently fused.
pub struct RegexFindIterator<'a> {
    regex: &'a Regex,
    input: &'a str,
    cursor: Option<usize>,
    steps: usize,
    matches: usize,
}

impl Iterator for RegexFindIterator<'_> {
    type Item = Result<RegexMatch, RegexError>;
    fn next(&mut self) -> Option<Self::Item> {
        let cursor = self.cursor?;
        self.cursor = None;
        let result = self.regex.search(
            self.input,
            SearchMode::At(cursor),
            &mut self.steps,
            RegexPhase::Match,
            0,
        );
        match result {
            Err(error) => Some(Err(error)),
            Ok(None) => None,
            Ok(Some(found)) => {
                if self.matches == self.regex.limits.max_matches {
                    return Some(Err(RegexError::limited(
                        RegexErrorKind::MatchLimitExceeded,
                        RegexPhase::Match,
                        found.span.start,
                        "max_matches",
                    )));
                }
                self.matches += 1;
                self.cursor = if found.span.start == found.span.end {
                    self.input[found.span.end..]
                        .chars()
                        .next()
                        .map(|scalar| found.span.end + scalar.len_utf8())
                } else {
                    Some(found.span.end)
                };
                Some(Ok(found))
            }
        }
    }
}
impl std::iter::FusedIterator for RegexFindIterator<'_> {}

fn copy_text(value: &str, phase: RegexPhase, offset: usize) -> Result<String, RegexError> {
    let mut result = String::new();
    result
        .try_reserve_exact(value.len())
        .map_err(|_| RegexError::at(RegexErrorKind::OutOfMemory, phase, offset, offset))?;
    result.push_str(value);
    Ok(result)
}

fn reserve<T>(
    values: &mut Vec<T>,
    extra: usize,
    phase: RegexPhase,
    offset: usize,
) -> Result<(), RegexError> {
    values
        .try_reserve(extra)
        .map_err(|_| RegexError::at(RegexErrorKind::OutOfMemory, phase, offset, offset))
}

enum Replacement<'a> {
    Literal(&'a str),
    Capture(usize),
}

impl Regex {
    fn replacement<'a>(&self, text: &'a str) -> Result<Vec<Replacement<'a>>, RegexError> {
        let phase = RegexPhase::Replace;
        if text.len() > self.limits.max_replacement_bytes {
            return Err(RegexError::limited(
                RegexErrorKind::ReplacementLimitExceeded,
                phase,
                0,
                "max_replacement_bytes",
            ));
        }
        if text
            .len()
            .saturating_mul(std::mem::size_of::<Replacement<'_>>())
            > self.limits.vm_heap
        {
            return Err(RegexError::limited(
                RegexErrorKind::OutOfMemory,
                phase,
                0,
                "vm_heap",
            ));
        }
        let mut tokens = Vec::new();
        reserve(&mut tokens, text.len(), phase, 0)?;
        let mut cursor = 0;
        while cursor < text.len() {
            let Some(relative) = text[cursor..].find('$') else {
                tokens.push(Replacement::Literal(&text[cursor..]));
                break;
            };
            let dollar = cursor + relative;
            tokens.push(Replacement::Literal(&text[cursor..dollar]));
            cursor = dollar + 1;
            let invalid = || {
                RegexError::at(
                    RegexErrorKind::InvalidReplacement,
                    phase,
                    dollar,
                    dollar + 1,
                )
            };
            match text.as_bytes().get(cursor) {
                Some(b'$') => {
                    tokens.push(Replacement::Literal("$"));
                    cursor += 1;
                }
                Some(b'{') => {
                    let end = text[cursor + 1..]
                        .find('}')
                        .map(|at| cursor + 1 + at)
                        .ok_or_else(invalid)?;
                    let name = &text[cursor + 1..end];
                    let index = self
                        .names
                        .iter()
                        .position(|item| item.as_deref() == Some(name))
                        .ok_or_else(invalid)?;
                    tokens.push(Replacement::Capture(index));
                    cursor = end + 1;
                }
                Some(b'0'..=b'9') => {
                    let start = cursor;
                    while text.as_bytes().get(cursor).is_some_and(u8::is_ascii_digit) {
                        cursor += 1;
                    }
                    let index = text[start..cursor]
                        .parse::<usize>()
                        .map_err(|_| invalid())?;
                    if index >= self.names.len() {
                        return Err(invalid());
                    }
                    tokens.push(Replacement::Capture(index));
                }
                _ => return Err(invalid()),
            }
        }
        Ok(tokens)
    }
    fn replace_impl(
        &self,
        input: &str,
        replacement: &str,
        all: bool,
    ) -> Result<String, RegexError> {
        self.check_input(input, RegexPhase::Replace)?;
        let tokens = self.replacement(replacement)?;
        let token_storage = tokens
            .len()
            .saturating_mul(std::mem::size_of::<Replacement<'_>>());
        let mut output = String::new();
        let mut cursor = Some(0);
        let mut copied = 0;
        let mut steps = 0;
        let mut matches = 0;
        while let Some(start) = cursor {
            let Some(found) = self.search(
                input,
                SearchMode::At(start),
                &mut steps,
                RegexPhase::Replace,
                token_storage.saturating_add(output.len()),
            )?
            else {
                break;
            };
            if matches == self.limits.max_matches {
                return Err(RegexError::limited(
                    RegexErrorKind::MatchLimitExceeded,
                    RegexPhase::Replace,
                    found.span.start,
                    "max_matches",
                ));
            }
            matches += 1;
            let reserved = token_storage.saturating_add(
                found
                    .captures
                    .len()
                    .saturating_mul(std::mem::size_of::<Option<RegexSpan>>()),
            );
            self.append(
                &mut output,
                &input[copied..found.span.start],
                found.span.start,
                reserved,
            )?;
            for token in &tokens {
                let value = match *token {
                    Replacement::Literal(value) => value,
                    Replacement::Capture(index) => found
                        .capture(index)
                        .map(|span| &input[span.start..span.end])
                        .unwrap_or(""),
                };
                self.append(&mut output, value, found.span.start, reserved)?;
            }
            copied = found.span.end;
            if !all {
                break;
            }
            cursor = if found.span.start == found.span.end {
                input[found.span.end..]
                    .chars()
                    .next()
                    .map(|scalar| found.span.end + scalar.len_utf8())
            } else {
                Some(found.span.end)
            };
        }
        self.append(&mut output, &input[copied..], input.len(), token_storage)?;
        Ok(output)
    }
    fn append(
        &self,
        output: &mut String,
        value: &str,
        offset: usize,
        reserved: usize,
    ) -> Result<(), RegexError> {
        let length = output.len().checked_add(value.len()).ok_or_else(|| {
            RegexError::limited(
                RegexErrorKind::OutputLimitExceeded,
                RegexPhase::Replace,
                offset,
                "max_output_bytes",
            )
        })?;
        if length > self.limits.max_output_bytes {
            return Err(RegexError::limited(
                RegexErrorKind::OutputLimitExceeded,
                RegexPhase::Replace,
                offset,
                "max_output_bytes",
            ));
        }
        if length
            .saturating_add(reserved)
            .saturating_add(self.program.logical_storage())
            .saturating_add(Self::metadata_storage(&self.pattern, &self.names))
            > self.limits.vm_heap
        {
            return Err(RegexError::limited(
                RegexErrorKind::OutOfMemory,
                RegexPhase::Replace,
                offset,
                "vm_heap",
            ));
        }
        output.try_reserve_exact(value.len()).map_err(|_| {
            RegexError::at(
                RegexErrorKind::OutOfMemory,
                RegexPhase::Replace,
                offset,
                offset,
            )
        })?;
        output.push_str(value);
        Ok(())
    }
}
