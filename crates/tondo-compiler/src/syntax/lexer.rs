use std::error::Error;
use std::fmt;
use std::str;

use unicode_normalization::UnicodeNormalization;

use crate::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticError, PrimaryLocation, Severity};
use crate::source::{FileId, SourceDatabase, SourceError, TextRange};

use super::token::{Token, TokenKind};

/// Whether the root source is allowed to begin with a shebang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexMode {
    Module,
    Script,
    Fragment,
}

/// Explicit defensive budgets for one lexer invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexLimits {
    pub max_tokens: usize,
    pub max_diagnostics: usize,
    pub max_nesting_depth: u32,
}

impl LexLimits {
    pub const DEFAULT: Self = Self {
        max_tokens: 2_000_000,
        max_diagnostics: 10_000,
        max_nesting_depth: 256,
    };

    pub const UNLIMITED: Self = Self {
        max_tokens: usize::MAX,
        max_diagnostics: usize::MAX,
        max_nesting_depth: u32::MAX,
    };
}

impl Default for LexLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexResource {
    Tokens,
    Diagnostics,
    NestingDepth,
}

impl fmt::Display for LexResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tokens => formatter.write_str("syntax token count"),
            Self::Diagnostics => formatter.write_str("primary diagnostic count"),
            Self::NestingDepth => formatter.write_str("syntax nesting depth"),
        }
    }
}

impl LexMode {
    fn allows_shebang(self) -> bool {
        matches!(self, Self::Script)
    }
}

#[derive(Debug)]
pub enum LexError {
    Source(SourceError),
    Diagnostic(DiagnosticError),
    ResourceLimit { resource: LexResource, offset: u32 },
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::ResourceLimit { resource, offset } => {
                write!(formatter, "{resource} limit reached at byte {offset}")
            }
        }
    }
}

impl Error for LexError {}

impl From<SourceError> for LexError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<DiagnosticError> for LexError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

/// Complete lossless lexer output for one source file.
#[derive(Debug)]
pub struct Lexed {
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl Lexed {
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub(crate) fn into_parts(self) -> (Vec<Token>, Vec<Diagnostic>) {
        (self.tokens, self.diagnostics)
    }

    /// Reconstructs the exact physical input represented by the token stream.
    pub fn reconstruct(&self, source: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(source.len());
        for token in &self.tokens {
            if token.is_synthetic() {
                continue;
            }
            let range = token.range();
            result.extend_from_slice(&source[range.start() as usize..range.end() as usize]);
        }
        result
    }

    /// Checks the byte-ownership invariant required by the lossless CST.
    pub fn has_exact_physical_partition(&self, source_length: u32) -> bool {
        let mut cursor = 0;
        for token in &self.tokens {
            if token.is_synthetic() {
                if token.range().start() != token.range().end() {
                    return false;
                }
                continue;
            }
            if token.range().start() != cursor || token.range().end() < cursor {
                return false;
            }
            cursor = token.range().end();
        }
        cursor == source_length
    }
}

/// Lexes one immutable source snapshot.
pub fn lex(sources: &SourceDatabase, file: FileId, mode: LexMode) -> Result<Lexed, LexError> {
    lex_with_limits(sources, file, mode, LexLimits::DEFAULT)
}

pub fn lex_with_limits(
    sources: &SourceDatabase,
    file: FileId,
    mode: LexMode,
    limits: LexLimits,
) -> Result<Lexed, LexError> {
    let source = sources.get(file)?;
    let mut scanner = Scanner {
        sources,
        file,
        bytes: source.bytes(),
        mode,
        position: 0,
        raw_tokens: Vec::new(),
        diagnostics: Vec::new(),
        limits,
        nesting_depth: 0,
    };

    scanner.scan_root()?;
    let tokens = insert_logical_newlines(scanner.raw_tokens, scanner.bytes, limits.max_tokens)?;

    Ok(Lexed {
        tokens,
        diagnostics: scanner.diagnostics,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: usize,
    end: usize,
}

struct Scanner<'a> {
    sources: &'a SourceDatabase,
    file: FileId,
    bytes: &'a [u8],
    mode: LexMode,
    position: usize,
    raw_tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    limits: LexLimits,
    nesting_depth: u32,
}

impl Scanner<'_> {
    fn scan_root(&mut self) -> Result<(), LexError> {
        if self.bytes.starts_with(b"#!") {
            if self.mode.allows_shebang() {
                self.scan_shebang()?;
            } else {
                let end = self.line_content_end(0);
                self.diagnose_invalid_utf8_in_range(0, end)?;
                self.emit(TokenKind::InvalidToken, 0, end)?;
                self.diagnostic(
                    "E0002",
                    "a shebang is only valid at byte zero of a script root",
                    0,
                    end,
                )?;
                self.position = end;
            }
        }

        let closed = self.scan_code(false)?;
        debug_assert!(!closed, "root scanning cannot stop at interpolation end");
        Ok(())
    }

    /// Scans ordinary Tondo code. When `stop_at_interpolation_end` is true, a
    /// top-level `}` is left unconsumed for the surrounding string scanner.
    fn scan_code(&mut self, stop_at_interpolation_end: bool) -> Result<bool, LexError> {
        let mut brace_depth = 0_u32;
        while self.position < self.bytes.len() {
            if stop_at_interpolation_end && self.bytes[self.position] == b'}' {
                if brace_depth == 0 {
                    return Ok(true);
                }
                let start = self.position;
                self.position += 1;
                self.emit(TokenKind::RBrace, start, self.position)?;
                brace_depth -= 1;
                continue;
            }

            if stop_at_interpolation_end && self.bytes[self.position] == b'{' {
                let start = self.position;
                self.position += 1;
                self.emit(TokenKind::LBrace, start, self.position)?;
                brace_depth = brace_depth
                    .checked_add(1)
                    .ok_or_else(|| self.resource_limit(LexResource::NestingDepth, self.position))?;
                if self.nesting_depth.saturating_add(brace_depth) > self.limits.max_nesting_depth {
                    return Err(self.resource_limit(LexResource::NestingDepth, start));
                }
                continue;
            }

            self.scan_one()?;
        }
        Ok(false)
    }

    fn scan_one(&mut self) -> Result<(), LexError> {
        if let Some(range) = self.invalid_utf8_at(self.position) {
            let start = range.start;
            while let Some(range) = self.invalid_utf8_at(self.position) {
                self.diagnostic(
                    "E0001",
                    "source contains an invalid UTF-8 byte sequence",
                    range.start,
                    range.end,
                )?;
                self.position = range.end;
            }
            return self.emit(TokenKind::InvalidUtf8, start, self.position);
        }

        let start = self.position;
        let byte = self.bytes[start];
        match byte {
            b' ' | b'\t' => self.scan_whitespace(),
            b'\n' => {
                self.position += 1;
                self.emit(TokenKind::PhysicalNewline, start, self.position)
            }
            b'\r' if self.bytes.get(start + 1) == Some(&b'\n') => {
                self.position += 2;
                self.emit(TokenKind::PhysicalNewline, start, self.position)
            }
            b'\r' => {
                self.position += 1;
                self.emit(TokenKind::InvalidToken, start, self.position)?;
                self.diagnostic(
                    "E0002",
                    "a carriage return must be followed by a line feed",
                    start,
                    self.position,
                )
            }
            b'/' if self.bytes.get(start + 1) == Some(&b'/') => self.scan_line_comment(),
            b'/' if self.bytes.get(start + 1) == Some(&b'*') => self.scan_block_comment(),
            b'r' if self.bytes[start..].starts_with(b"r\"\"\"") => self.scan_raw_multiline_string(),
            b'r' if self.bytes[start..].starts_with(b"r\"") => self.scan_raw_string(),
            b'\"' if self.bytes[start..].starts_with(b"\"\"\"") => self.scan_string(true),
            b'\"' => self.scan_string(false),
            b'\'' => self.scan_char(),
            b'0'..=b'9' => self.scan_number(),
            b'_' | b'a'..=b'z' | b'A'..=b'Z' => self.scan_identifier(),
            _ if byte.is_ascii() => self.scan_operator_or_invalid(),
            _ => match self.char_at(start) {
                Some((character, _)) if unicode_ident::is_xid_start(character) => {
                    self.scan_identifier()
                }
                Some((_, width)) => {
                    self.position += width;
                    self.emit(TokenKind::InvalidToken, start, self.position)?;
                    self.diagnostic(
                        "E0002",
                        "character is neither whitespace nor part of a Tondo token",
                        start,
                        self.position,
                    )
                }
                None => {
                    // The invalid-sequence branch above normally handles this.
                    // Retain guaranteed progress if a future decoder changes.
                    self.position += 1;
                    self.emit(TokenKind::InvalidUtf8, start, self.position)
                }
            },
        }
    }

    fn scan_shebang(&mut self) -> Result<(), LexError> {
        let end = self.line_content_end(0);
        self.diagnose_invalid_utf8_in_range(0, end)?;
        self.position = end;
        self.emit(TokenKind::Shebang, 0, end)
    }

    fn line_content_end(&self, start: usize) -> usize {
        self.bytes[start..]
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
            .map_or(self.bytes.len(), |relative| start + relative)
    }

    fn scan_whitespace(&mut self) -> Result<(), LexError> {
        let start = self.position;
        while matches!(self.bytes.get(self.position), Some(b' ' | b'\t')) {
            self.position += 1;
        }
        self.emit(TokenKind::Whitespace, start, self.position)
    }

    fn scan_line_comment(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.position += 2;
        while self.position < self.bytes.len() {
            if self.bytes[self.position] == b'\n'
                || self.bytes[self.position..].starts_with(b"\r\n")
            {
                break;
            }
            if self.bytes[self.position] == b'\r' {
                self.diagnostic(
                    "E0002",
                    "a carriage return must be followed by a line feed",
                    self.position,
                    self.position + 1,
                )?;
            }
            if let Some(range) = self.invalid_utf8_at(self.position) {
                self.diagnostic(
                    "E0001",
                    "source contains an invalid UTF-8 byte sequence",
                    range.start,
                    range.end,
                )?;
                self.position = range.end;
            } else if let Some((_, width)) = self.char_at(self.position) {
                self.position += width;
            } else {
                self.position += 1;
            }
        }
        let kind = if self.bytes[start..self.position].starts_with(b"///") {
            TokenKind::DocComment
        } else {
            TokenKind::LineComment
        };
        self.emit(kind, start, self.position)
    }

    fn scan_block_comment(&mut self) -> Result<(), LexError> {
        let opening = self.position;
        let mut segment_start = opening;
        let mut depth = 1_u32;
        if depth > self.limits.max_nesting_depth {
            return Err(self.resource_limit(LexResource::NestingDepth, opening));
        }
        self.position += 2;

        while self.position < self.bytes.len() {
            if let Some(range) = self.invalid_utf8_at(self.position) {
                self.diagnostic(
                    "E0001",
                    "source contains an invalid UTF-8 byte sequence",
                    range.start,
                    range.end,
                )?;
                self.position = range.end;
                continue;
            }
            if self.bytes[self.position..].starts_with(b"/*") {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| self.resource_limit(LexResource::NestingDepth, self.position))?;
                if depth > self.limits.max_nesting_depth {
                    return Err(self.resource_limit(LexResource::NestingDepth, self.position));
                }
                self.position += 2;
                continue;
            }
            if self.bytes[self.position..].starts_with(b"*/") {
                self.position += 2;
                depth -= 1;
                if depth == 0 {
                    self.emit(TokenKind::BlockComment, segment_start, self.position)?;
                    return Ok(());
                }
                continue;
            }
            if self.bytes[self.position] == b'\n' {
                self.emit_nonempty(TokenKind::BlockComment, segment_start, self.position)?;
                let newline = self.position;
                self.position += 1;
                self.emit(TokenKind::PhysicalNewline, newline, self.position)?;
                segment_start = self.position;
                continue;
            }
            if self.bytes[self.position..].starts_with(b"\r\n") {
                self.emit_nonempty(TokenKind::BlockComment, segment_start, self.position)?;
                let newline = self.position;
                self.position += 2;
                self.emit(TokenKind::PhysicalNewline, newline, self.position)?;
                segment_start = self.position;
                continue;
            }
            if self.bytes[self.position] == b'\r' {
                let invalid = self.position;
                self.position += 1;
                self.diagnostic(
                    "E0002",
                    "a carriage return must be followed by a line feed",
                    invalid,
                    self.position,
                )?;
                continue;
            }
            if let Some((_, width)) = self.char_at(self.position) {
                self.position += width;
            } else {
                self.position += 1;
            }
        }

        self.emit_nonempty(TokenKind::BlockComment, segment_start, self.position)?;
        self.diagnostic(
            "E0002",
            "unterminated block comment",
            opening,
            self.position,
        )
    }

    fn scan_identifier(&mut self) -> Result<(), LexError> {
        let start = self.position;
        let Some((first, first_width)) = self.char_at(start) else {
            self.position += 1;
            return self.emit(TokenKind::InvalidUtf8, start, self.position);
        };
        debug_assert!(first == '_' || unicode_ident::is_xid_start(first));
        self.position += first_width;

        while let Some((character, width)) = self.char_at(self.position) {
            if character != '_' && !unicode_ident::is_xid_continue(character) {
                break;
            }
            self.position += width;
        }

        let spelling = str::from_utf8(&self.bytes[start..self.position])
            .expect("identifier scanning only crosses valid UTF-8 scalars");
        let normalized = spelling.nfc().collect::<String>();
        if let Some(keyword) = TokenKind::from_keyword(&normalized) {
            self.emit(keyword, start, self.position)
        } else {
            self.ensure_token_capacity(start)?;
            let range = text_range(start, self.position);
            self.raw_tokens
                .push(Token::identifier(range, normalized.into_boxed_str()));
            Ok(())
        }
    }

    fn scan_operator_or_invalid(&mut self) -> Result<(), LexError> {
        let start = self.position;
        if self.bytes[start..].starts_with(b"--") || self.bytes[start..].starts_with(b"??") {
            self.position += 2;
            self.emit(TokenKind::InvalidToken, start, self.position)?;
            return self.diagnostic(
                "E0002",
                "adjacent `--` and `??` are not Tondo operators",
                start,
                self.position,
            );
        }

        if let Some((length, kind)) = operator_at(&self.bytes[start..]) {
            self.position += length;
            return self.emit(kind, start, self.position);
        }

        self.position += 1;
        self.emit(TokenKind::InvalidToken, start, self.position)?;
        self.diagnostic(
            "E0002",
            "character or sequence does not form a Tondo token",
            start,
            self.position,
        )
    }

    fn invalid_utf8_at(&self, position: usize) -> Option<ByteRange> {
        let first = *self.bytes.get(position)?;
        if first.is_ascii() || self.char_at(position).is_some() {
            return None;
        }
        let error = str::from_utf8(&self.bytes[position..]).unwrap_err();
        debug_assert_eq!(error.valid_up_to(), 0);
        let length = error.error_len().unwrap_or(self.bytes.len() - position);
        Some(ByteRange {
            start: position,
            end: position + length.max(1),
        })
    }

    fn char_at(&self, position: usize) -> Option<(char, usize)> {
        let first = *self.bytes.get(position)?;
        let width = utf8_width(first)?;
        let end = position.checked_add(width)?;
        let text = str::from_utf8(self.bytes.get(position..end)?).ok()?;
        text.chars().next().map(|character| (character, width))
    }

    fn emit(&mut self, kind: TokenKind, start: usize, end: usize) -> Result<(), LexError> {
        self.ensure_token_capacity(start)?;
        self.raw_tokens
            .push(Token::physical(kind, text_range(start, end)));
        Ok(())
    }

    fn emit_nonempty(&mut self, kind: TokenKind, start: usize, end: usize) -> Result<(), LexError> {
        if start < end {
            self.emit(kind, start, end)?;
        }
        Ok(())
    }

    fn emit_synthetic(&mut self, kind: TokenKind, offset: usize) -> Result<(), LexError> {
        self.ensure_token_capacity(offset)?;
        self.raw_tokens.push(Token::synthetic(
            kind,
            u32::try_from(offset).expect("source length was validated on insertion"),
        ));
        Ok(())
    }

    fn diagnostic(
        &mut self,
        code: &str,
        message: &str,
        start: usize,
        end: usize,
    ) -> Result<(), LexError> {
        if self.diagnostics.len() >= self.limits.max_diagnostics {
            return Err(self.resource_limit(LexResource::Diagnostics, start));
        }
        let span = self.sources.span(self.file, text_range(start, end))?;
        self.diagnostics.push(Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(span),
        )?);
        Ok(())
    }

    fn ensure_token_capacity(&self, offset: usize) -> Result<(), LexError> {
        if self.raw_tokens.len() >= self.limits.max_tokens {
            Err(self.resource_limit(LexResource::Tokens, offset))
        } else {
            Ok(())
        }
    }

    fn resource_limit(&self, resource: LexResource, offset: usize) -> LexError {
        LexError::ResourceLimit {
            resource,
            offset: u32::try_from(offset).expect("source length was validated on insertion"),
        }
    }

    fn diagnose_invalid_utf8_in_range(&mut self, start: usize, end: usize) -> Result<(), LexError> {
        let mut cursor = start;
        while cursor < end {
            if let Some(range) = self.invalid_utf8_at(cursor) {
                self.diagnostic(
                    "E0001",
                    "source contains an invalid UTF-8 byte sequence",
                    range.start,
                    range.end,
                )?;
                cursor = range.end;
            } else if let Some((_, width)) = self.char_at(cursor) {
                cursor += width;
            } else {
                cursor += 1;
            }
        }
        Ok(())
    }
}

impl Scanner<'_> {
    fn scan_number(&mut self) -> Result<(), LexError> {
        let start = self.position;
        if self.bytes[start..].starts_with(b"0b")
            || self.bytes[start..].starts_with(b"0o")
            || self.bytes[start..].starts_with(b"0x")
        {
            self.position += 2;
            self.consume_number_tail();
        } else {
            self.consume_ascii_digits_and_underscores();

            if self.bytes.get(self.position) == Some(&b'.')
                && self
                    .bytes
                    .get(self.position + 1)
                    .is_some_and(u8::is_ascii_digit)
            {
                self.position += 1;
                self.consume_ascii_digits_and_underscores();
            }

            if matches!(self.bytes.get(self.position), Some(b'e' | b'E')) {
                self.position += 1;
                if matches!(self.bytes.get(self.position), Some(b'+' | b'-')) {
                    self.position += 1;
                }
                self.consume_ascii_digits_and_underscores();
            }

            self.consume_number_tail();
        }

        let spelling = str::from_utf8(&self.bytes[start..self.position])
            .expect("a numeric candidate never crosses invalid UTF-8");
        match validate_number(spelling) {
            Some(kind) => self.emit(kind, start, self.position),
            None => {
                self.emit(TokenKind::MalformedLiteral, start, self.position)?;
                self.diagnostic("E0003", "malformed numeric literal", start, self.position)
            }
        }
    }

    fn consume_ascii_digits_and_underscores(&mut self) {
        while matches!(self.bytes.get(self.position), Some(b'0'..=b'9' | b'_')) {
            self.position += 1;
        }
    }

    fn consume_number_tail(&mut self) {
        while let Some((character, width)) = self.char_at(self.position) {
            if character != '_' && !unicode_ident::is_xid_continue(character) {
                break;
            }
            self.position += width;
        }
    }

    fn scan_char(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.position += 1;
        let mut malformed = false;
        let mut invalid_utf8_scalar = false;
        let mut scalar_count = 0_u8;

        if self.position >= self.bytes.len()
            || matches!(self.bytes.get(self.position), Some(b'\n' | b'\r' | b'\''))
        {
            malformed = true;
        } else if self.bytes[self.position] == b'\\' {
            let escape = self.scan_escape(EscapeContext::Char)?;
            malformed |= !escape.valid;
            invalid_utf8_scalar |= escape.invalid_utf8;
            scalar_count = u8::from(escape.represents_scalar);
        } else if let Some(range) = self.invalid_utf8_at(self.position) {
            self.diagnostic(
                "E0001",
                "source contains an invalid UTF-8 byte sequence",
                range.start,
                range.end,
            )?;
            self.position = range.end;
            invalid_utf8_scalar = true;
        } else if let Some((character, width)) = self.char_at(self.position) {
            malformed |= character.is_ascii_control();
            scalar_count = 1;
            self.position += width;
        }

        if self.bytes.get(self.position) == Some(&b'\'') {
            self.position += 1;
        } else {
            malformed = true;
            while self.position < self.bytes.len() && self.bytes[self.position] != b'\'' {
                if self.bytes[self.position] == b'\n'
                    || self.bytes[self.position..].starts_with(b"\r\n")
                {
                    break;
                }
                if let Some(range) = self.invalid_utf8_at(self.position) {
                    self.diagnostic(
                        "E0001",
                        "source contains an invalid UTF-8 byte sequence",
                        range.start,
                        range.end,
                    )?;
                    self.position = range.end;
                } else if let Some((_, width)) = self.char_at(self.position) {
                    scalar_count = scalar_count.saturating_add(1);
                    self.position += width;
                } else {
                    self.position += 1;
                }
            }
            if self.bytes.get(self.position) == Some(&b'\'') {
                self.position += 1;
            }
        }

        if scalar_count != 1 && !invalid_utf8_scalar {
            malformed = true;
        }
        let kind = if malformed || invalid_utf8_scalar {
            TokenKind::MalformedLiteral
        } else {
            TokenKind::CharLiteral
        };
        self.emit(kind, start, self.position)?;
        if malformed {
            self.diagnostic(
                "E0003",
                "a character literal must contain exactly one valid Unicode scalar",
                start,
                self.position,
            )?;
        }
        Ok(())
    }

    fn scan_raw_string(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.position += 2;
        let mut malformed = false;
        let mut terminated = false;

        while self.position < self.bytes.len() {
            match self.bytes[self.position] {
                b'\"' => {
                    self.position += 1;
                    terminated = true;
                    break;
                }
                b'\n' => break,
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => break,
                b'\r' => {
                    malformed = true;
                    self.position += 1;
                }
                byte if byte.is_ascii_control() => {
                    malformed = true;
                    self.position += 1;
                }
                _ => self.advance_scalar_or_invalid()?,
            }
        }

        malformed |= !terminated;
        let kind = if malformed {
            TokenKind::MalformedLiteral
        } else {
            TokenKind::RawStringLiteral
        };
        self.emit(kind, start, self.position)?;
        if malformed {
            self.diagnostic(
                "E0003",
                "malformed or unterminated raw string literal",
                start,
                self.position,
            )?;
        }
        Ok(())
    }

    fn scan_raw_multiline_string(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.position += 4;
        let content_start = self.position;
        let mut malformed = false;
        let mut closing = None;

        while self.position < self.bytes.len() {
            if self.bytes[self.position..].starts_with(b"\"\"\"") {
                closing = Some(self.position);
                self.position += 3;
                break;
            }
            if self.bytes[self.position] == b'\r'
                && self.bytes.get(self.position + 1) != Some(&b'\n')
            {
                malformed = true;
            }
            self.advance_scalar_or_invalid()?;
        }

        if let Some(closing) = closing {
            malformed |= !valid_multiline_indentation(self.bytes, content_start, closing);
        } else {
            malformed = true;
        }

        let kind = if malformed {
            TokenKind::MalformedLiteral
        } else {
            TokenKind::RawMultilineStringLiteral
        };
        self.emit(kind, start, self.position)?;
        if malformed {
            self.diagnostic(
                "E0003",
                "malformed raw multiline string literal",
                start,
                self.position,
            )?;
        }
        Ok(())
    }

    fn scan_string(&mut self, multiline: bool) -> Result<(), LexError> {
        let opening = self.position;
        let delimiter_length = if multiline { 3 } else { 1 };
        self.position += delimiter_length;
        let content_start = self.position;
        self.emit(
            if multiline {
                TokenKind::MultilineStringStart
            } else {
                TokenKind::StringStart
            },
            opening,
            self.position,
        )?;

        let mut segment_start = self.position;
        let mut malformed = false;
        loop {
            if self.position >= self.bytes.len() {
                self.emit_nonempty(TokenKind::StringText, segment_start, self.position)?;
                self.emit_synthetic(
                    if multiline {
                        TokenKind::MultilineStringEnd
                    } else {
                        TokenKind::StringEnd
                    },
                    self.position,
                )?;
                self.diagnostic(
                    "E0003",
                    "unterminated string literal",
                    opening,
                    self.position,
                )?;
                return Ok(());
            }

            let closes = if multiline {
                self.bytes[self.position..].starts_with(b"\"\"\"")
            } else {
                self.bytes[self.position] == b'\"'
            };
            if closes {
                let closing = self.position;
                self.emit_nonempty(TokenKind::StringText, segment_start, closing)?;
                self.position += delimiter_length;
                self.emit(
                    if multiline {
                        TokenKind::MultilineStringEnd
                    } else {
                        TokenKind::StringEnd
                    },
                    closing,
                    self.position,
                )?;
                if multiline {
                    malformed |= !valid_multiline_indentation(self.bytes, content_start, closing);
                }
                if malformed {
                    self.diagnostic("E0003", "malformed string literal", opening, self.position)?;
                }
                return Ok(());
            }

            match self.bytes[self.position] {
                b'\\' => {
                    let escape = self.scan_escape(EscapeContext::String)?;
                    malformed |= !escape.valid;
                }
                b'{' if self.bytes.get(self.position + 1) == Some(&b'{') => {
                    self.position += 2;
                }
                b'}' if self.bytes.get(self.position + 1) == Some(&b'}') => {
                    self.position += 2;
                }
                b'{' => {
                    self.emit_nonempty(TokenKind::StringText, segment_start, self.position)?;
                    let interpolation = self.position;
                    self.position += 1;
                    self.emit(TokenKind::InterpolationStart, interpolation, self.position)?;
                    if !self.scan_interpolation(interpolation, multiline)? {
                        self.emit_synthetic(
                            if multiline {
                                TokenKind::MultilineStringEnd
                            } else {
                                TokenKind::StringEnd
                            },
                            self.position,
                        )?;
                        return Ok(());
                    }
                    segment_start = self.position;
                }
                b'}' => {
                    malformed = true;
                    self.position += 1;
                }
                b'\n' if !multiline => {
                    self.emit_nonempty(TokenKind::StringText, segment_start, self.position)?;
                    self.emit_synthetic(TokenKind::StringEnd, self.position)?;
                    self.diagnostic(
                        "E0003",
                        "a single-line string cannot contain a physical newline",
                        opening,
                        self.position,
                    )?;
                    return Ok(());
                }
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') && !multiline => {
                    self.emit_nonempty(TokenKind::StringText, segment_start, self.position)?;
                    self.emit_synthetic(TokenKind::StringEnd, self.position)?;
                    self.diagnostic(
                        "E0003",
                        "a single-line string cannot contain a physical newline",
                        opening,
                        self.position,
                    )?;
                    return Ok(());
                }
                b'\r' if self.bytes.get(self.position + 1) == Some(&b'\n') => {
                    self.position += 2;
                }
                b'\r' if self.bytes.get(self.position + 1) != Some(&b'\n') => {
                    malformed = true;
                    self.position += 1;
                }
                byte if byte.is_ascii_control() && byte != b'\n' => {
                    malformed = true;
                    self.position += 1;
                }
                _ => self.advance_scalar_or_invalid()?,
            }
        }
    }

    fn scan_interpolation(
        &mut self,
        opening: usize,
        multiline_string: bool,
    ) -> Result<bool, LexError> {
        let first_token = self.raw_tokens.len();
        self.enter_nesting(opening)?;
        let scan_result = self.scan_code(true);
        self.nesting_depth -= 1;
        let closed = scan_result?;
        if !closed {
            self.emit_synthetic(TokenKind::InterpolationEnd, self.position)?;
            self.diagnostic(
                "E0003",
                "unterminated string interpolation",
                opening,
                self.position,
            )?;
            return Ok(false);
        }

        let has_expression = self.raw_tokens[first_token..]
            .iter()
            .any(|token| !token.kind().is_trivia() && !token.is_synthetic());
        let closing = self.position;
        self.position += 1;
        self.emit(TokenKind::InterpolationEnd, closing, self.position)?;
        if !multiline_string
            && self.bytes[opening..self.position]
                .iter()
                .any(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.diagnostic(
                "E0003",
                "a single-line string interpolation cannot contain a physical newline",
                opening,
                self.position,
            )?;
        }
        if !has_expression {
            self.diagnostic(
                "E0003",
                "a string interpolation cannot be empty",
                opening,
                self.position,
            )?;
        }
        Ok(true)
    }

    fn scan_escape(&mut self, context: EscapeContext) -> Result<EscapeResult, LexError> {
        debug_assert_eq!(self.bytes.get(self.position), Some(&b'\\'));
        self.position += 1;
        let Some(&escaped) = self.bytes.get(self.position) else {
            return Ok(EscapeResult::invalid());
        };
        if let Some(range) = self.invalid_utf8_at(self.position) {
            self.diagnostic(
                "E0001",
                "source contains an invalid UTF-8 byte sequence",
                range.start,
                range.end,
            )?;
            self.position = range.end;
            return Ok(EscapeResult::invalid_utf8());
        }

        if matches!(escaped, b'n' | b'r' | b't' | b'\\' | b'0')
            || (context == EscapeContext::Char && escaped == b'\'')
            || (context == EscapeContext::String && escaped == b'\"')
        {
            self.position += 1;
            return Ok(EscapeResult::valid());
        }

        if escaped != b'u' {
            self.position += 1;
            return Ok(EscapeResult::invalid());
        }

        self.position += 1;
        if self.bytes.get(self.position) != Some(&b'{') {
            return Ok(EscapeResult::invalid());
        }
        self.position += 1;
        let digits_start = self.position;
        let mut value = 0_u32;
        let mut digits = 0_u8;
        while let Some(digit) = self
            .bytes
            .get(self.position)
            .and_then(|byte| hex_value(*byte))
        {
            digits = digits.saturating_add(1);
            value = value.saturating_mul(16).saturating_add(u32::from(digit));
            self.position += 1;
        }
        let closed = self.bytes.get(self.position) == Some(&b'}');
        if closed {
            self.position += 1;
        }
        let valid = closed
            && self.position > digits_start
            && (1..=6).contains(&digits)
            && value <= 0x10ffff
            && !(0xd800..=0xdfff).contains(&value);
        Ok(EscapeResult {
            valid,
            represents_scalar: valid,
            invalid_utf8: false,
        })
    }

    fn advance_scalar_or_invalid(&mut self) -> Result<(), LexError> {
        if let Some(range) = self.invalid_utf8_at(self.position) {
            self.diagnostic(
                "E0001",
                "source contains an invalid UTF-8 byte sequence",
                range.start,
                range.end,
            )?;
            self.position = range.end;
        } else if let Some((_, width)) = self.char_at(self.position) {
            self.position += width;
        } else {
            self.position += 1;
        }
        Ok(())
    }

    fn enter_nesting(&mut self, offset: usize) -> Result<(), LexError> {
        let next = self
            .nesting_depth
            .checked_add(1)
            .ok_or_else(|| self.resource_limit(LexResource::NestingDepth, offset))?;
        if next > self.limits.max_nesting_depth {
            return Err(self.resource_limit(LexResource::NestingDepth, offset));
        }
        self.nesting_depth = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeContext {
    Char,
    String,
}

#[derive(Debug, Clone, Copy)]
struct EscapeResult {
    valid: bool,
    represents_scalar: bool,
    invalid_utf8: bool,
}

impl EscapeResult {
    fn valid() -> Self {
        Self {
            valid: true,
            represents_scalar: true,
            invalid_utf8: false,
        }
    }

    fn invalid() -> Self {
        Self {
            valid: false,
            represents_scalar: false,
            invalid_utf8: false,
        }
    }

    fn invalid_utf8() -> Self {
        Self {
            valid: true,
            represents_scalar: false,
            invalid_utf8: true,
        }
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn validate_number(spelling: &str) -> Option<TokenKind> {
    if let Some(rest) = spelling
        .strip_prefix("0b")
        .or_else(|| spelling.strip_prefix("0o"))
        .or_else(|| spelling.strip_prefix("0x"))
    {
        let base = match spelling.as_bytes()[1] {
            b'b' => 2,
            b'o' => 8,
            b'x' => 16,
            _ => unreachable!("the prefix alternatives are exhaustive"),
        };
        let digits = strip_integer_suffix(rest)?;
        return valid_digit_sequence(digits, |byte| digit_in_base(byte, base))
            .then_some(TokenKind::IntegerLiteral);
    }

    let has_float_syntax = spelling.contains('.') || spelling.contains(['e', 'E']);
    if has_float_syntax {
        let body = strip_float_suffix(spelling)?;
        validate_decimal_float(body).then_some(TokenKind::FloatLiteral)
    } else {
        let body = strip_integer_suffix(spelling)?;
        valid_decimal_numeral(body).then_some(TokenKind::IntegerLiteral)
    }
}

fn strip_integer_suffix(spelling: &str) -> Option<&str> {
    const SUFFIXES: &[&str] = &["i16", "i32", "i64", "u16", "u32", "u64", "i8", "u8"];
    for suffix in SUFFIXES {
        if let Some(body) = spelling.strip_suffix(suffix) {
            return Some(body);
        }
    }
    if spelling
        .chars()
        .all(|character| character == '_' || character.is_ascii_hexdigit())
    {
        Some(spelling)
    } else {
        None
    }
}

fn strip_float_suffix(spelling: &str) -> Option<&str> {
    if let Some(body) = spelling.strip_suffix("f32") {
        Some(body)
    } else if let Some(body) = spelling.strip_suffix("f64") {
        Some(body)
    } else if spelling.chars().all(|character| {
        character.is_ascii_digit() || matches!(character, '_' | '.' | 'e' | 'E' | '+' | '-')
    }) {
        Some(spelling)
    } else {
        None
    }
}

fn validate_decimal_float(body: &str) -> bool {
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(index) => (&body[..index], Some(&body[index + 1..])),
        None => (body, None),
    };
    if body[index_after_first_exponent(body)..].contains(['e', 'E']) {
        return false;
    }

    let mantissa_valid = if let Some(dot) = mantissa.find('.') {
        !mantissa[dot + 1..].contains('.')
            && valid_decimal_numeral(&mantissa[..dot])
            && valid_digit_sequence(&mantissa[dot + 1..], |byte| byte.is_ascii_digit())
    } else {
        valid_decimal_numeral(mantissa)
    };
    if !mantissa_valid || (mantissa.find('.').is_none() && exponent.is_none()) {
        return false;
    }

    exponent.is_none_or(|value| {
        let digits = value
            .strip_prefix('+')
            .or_else(|| value.strip_prefix('-'))
            .unwrap_or(value);
        valid_digit_sequence(digits, |byte| byte.is_ascii_digit())
    })
}

fn index_after_first_exponent(value: &str) -> usize {
    value
        .find(['e', 'E'])
        .map_or(0, |index| index.saturating_add(1))
}

fn valid_decimal_numeral(value: &str) -> bool {
    if !valid_digit_sequence(value, |byte| byte.is_ascii_digit()) {
        return false;
    }
    value == "0" || !value.starts_with('0')
}

fn valid_digit_sequence<F>(value: &str, is_digit: F) -> bool
where
    F: Fn(u8) -> bool,
{
    let bytes = value.as_bytes();
    if bytes.is_empty() || !is_digit(bytes[0]) || !is_digit(*bytes.last().unwrap_or(&b'_')) {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        is_digit(*byte)
            || (*byte == b'_'
                && index > 0
                && index + 1 < bytes.len()
                && is_digit(bytes[index - 1])
                && is_digit(bytes[index + 1]))
    })
}

fn digit_in_base(byte: u8, base: u8) -> bool {
    match base {
        2 => matches!(byte, b'0' | b'1'),
        8 => matches!(byte, b'0'..=b'7'),
        16 => byte.is_ascii_hexdigit(),
        _ => false,
    }
}

fn valid_multiline_indentation(bytes: &[u8], content_start: usize, closing: usize) -> bool {
    let line_start = bytes[..closing]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(content_start, |newline| newline + 1);
    if line_start < content_start
        || !bytes[line_start..closing]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return true;
    }

    let prefix = &bytes[line_start..closing];
    let mut effective_start = content_start;
    if bytes.get(effective_start) == Some(&b'\n') {
        effective_start += 1;
    } else if bytes.get(effective_start..effective_start + 2) == Some(&b"\r\n"[..]) {
        effective_start += 2;
    }

    let effective_end = if line_start > effective_start {
        let lf = line_start - 1;
        if lf > effective_start && bytes[lf - 1] == b'\r' {
            lf - 1
        } else {
            lf
        }
    } else {
        line_start
    };

    let mut cursor = effective_start;
    while cursor < effective_end {
        let line_end = bytes[cursor..effective_end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(effective_end, |relative| cursor + relative);
        let line = if line_end > cursor && bytes[line_end - 1] == b'\r' {
            &bytes[cursor..line_end - 1]
        } else {
            &bytes[cursor..line_end]
        };
        let empty = line.iter().all(|byte| matches!(byte, b' ' | b'\t'));
        if !empty && !line.starts_with(prefix) {
            return false;
        }
        cursor = if line_end < effective_end {
            line_end + 1
        } else {
            effective_end
        };
    }
    true
}

fn text_range(start: usize, end: usize) -> TextRange {
    let start = u32::try_from(start).expect("source length was validated on insertion");
    let end = u32::try_from(end).expect("source length was validated on insertion");
    TextRange::new(start, end).expect("lexer ranges are monotonically increasing")
}

fn utf8_width(first: u8) -> Option<usize> {
    match first {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

fn operator_at(bytes: &[u8]) -> Option<(usize, TokenKind)> {
    const OPERATORS: &[(&[u8], TokenKind)] = &[
        (b"<<=", TokenKind::ShlEq),
        (b">>=", TokenKind::ShrEq),
        (b"...", TokenKind::Ellipsis),
        (b"..=", TokenKind::DotDotEq),
        (b"=>", TokenKind::FatArrow),
        (b"+=", TokenKind::PlusEq),
        (b"-=", TokenKind::MinusEq),
        (b"*=", TokenKind::StarEq),
        (b"/=", TokenKind::SlashEq),
        (b"%=", TokenKind::PercentEq),
        (b"&=", TokenKind::AmpEq),
        (b"^=", TokenKind::CaretEq),
        (b"|=", TokenKind::PipeEq),
        (b"<<", TokenKind::Shl),
        (b">>", TokenKind::Shr),
        (b"<=", TokenKind::LessEq),
        (b">=", TokenKind::GreaterEq),
        (b"==", TokenKind::EqEq),
        (b"!=", TokenKind::BangEq),
        (b"..", TokenKind::DotDot),
        (b"(", TokenKind::LParen),
        (b")", TokenKind::RParen),
        (b"[", TokenKind::LBracket),
        (b"]", TokenKind::RBracket),
        (b"{", TokenKind::LBrace),
        (b"}", TokenKind::RBrace),
        (b",", TokenKind::Comma),
        (b".", TokenKind::Dot),
        (b":", TokenKind::Colon),
        (b"?", TokenKind::Question),
        (b"!", TokenKind::Bang),
        (b"~", TokenKind::Tilde),
        (b"=", TokenKind::Eq),
        (b"+", TokenKind::Plus),
        (b"-", TokenKind::Minus),
        (b"*", TokenKind::Star),
        (b"/", TokenKind::Slash),
        (b"%", TokenKind::Percent),
        (b"&", TokenKind::Amp),
        (b"^", TokenKind::Caret),
        (b"|", TokenKind::Pipe),
        (b"<", TokenKind::Less),
        (b">", TokenKind::Greater),
    ];

    OPERATORS
        .iter()
        .find(|(spelling, _)| bytes.starts_with(spelling))
        .map(|(spelling, kind)| (spelling.len(), *kind))
}

fn insert_logical_newlines(
    raw_tokens: Vec<Token>,
    bytes: &[u8],
    max_tokens: usize,
) -> Result<Vec<Token>, LexError> {
    let mut result = Vec::with_capacity(raw_tokens.len().saturating_add(2));
    let mut index = 0;
    let mut previous_significant = None;
    let mut delimiters = Vec::new();

    while index < raw_tokens.len() {
        if raw_tokens[index].kind().is_trivia() {
            let gap_start = index;
            let mut has_physical_newline = false;
            while index < raw_tokens.len() && raw_tokens[index].kind().is_trivia() {
                has_physical_newline |= raw_tokens[index].kind() == TokenKind::PhysicalNewline;
                index += 1;
            }
            for token in &raw_tokens[gap_start..index] {
                push_bounded_token(&mut result, token.clone(), max_tokens)?;
            }

            let next_significant = raw_tokens.get(index).map(Token::kind);
            // Parentheses and brackets suppress logical newlines only while
            // they are the innermost delimiter. A nested brace starts a body
            // whose fields or statements still need significant newlines.
            let delimited = matches!(
                delimiters.last().copied(),
                Some(TokenKind::LParen | TokenKind::LBracket)
            );
            let suppressed_by_previous =
                previous_significant.is_some_and(TokenKind::suppresses_newline_after);
            let suppressed_by_next =
                next_significant.is_some_and(TokenKind::suppresses_newline_before);
            if has_physical_newline && !delimited && !suppressed_by_previous && !suppressed_by_next
            {
                let offset = raw_tokens
                    .get(index)
                    .map_or(bytes.len() as u32, |token| token.range().start());
                push_bounded_token(
                    &mut result,
                    Token::synthetic(TokenKind::Nl, offset),
                    max_tokens,
                )?;
            }
            continue;
        }

        let token = raw_tokens[index].clone();
        let kind = token.kind();
        push_bounded_token(&mut result, token, max_tokens)?;
        match kind {
            TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => delimiters.push(kind),
            TokenKind::RParen if delimiters.last() == Some(&TokenKind::LParen) => {
                delimiters.pop();
            }
            TokenKind::RBracket if delimiters.last() == Some(&TokenKind::LBracket) => {
                delimiters.pop();
            }
            TokenKind::RBrace if delimiters.last() == Some(&TokenKind::LBrace) => {
                delimiters.pop();
            }
            _ => {}
        }
        previous_significant = Some(kind);
        index += 1;
    }

    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        push_bounded_token(
            &mut result,
            Token::synthetic(
                TokenKind::Nl,
                u32::try_from(bytes.len()).expect("source length was validated on insertion"),
            ),
            max_tokens,
        )?;
    }
    push_bounded_token(
        &mut result,
        Token::synthetic(
            TokenKind::Eof,
            u32::try_from(bytes.len()).expect("source length was validated on insertion"),
        ),
        max_tokens,
    )?;
    Ok(result)
}

fn push_bounded_token(
    tokens: &mut Vec<Token>,
    token: Token,
    max_tokens: usize,
) -> Result<(), LexError> {
    if tokens.len() >= max_tokens {
        return Err(LexError::ResourceLimit {
            resource: LexResource::Tokens,
            offset: token.range().start(),
        });
    }
    tokens.push(token);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::diagnostics::PrimaryLocation;
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};

    fn lex_bytes(bytes: impl Into<Arc<[u8]>>, mode: LexMode) -> (SourceDatabase, FileId, Lexed) {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:lexer-test").unwrap(),
                ModulePath::new("lexer").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                bytes,
            ))
            .unwrap();
        let lexed = lex(&sources, file, mode).unwrap();
        (sources, file, lexed)
    }

    fn physical_significant_kinds(lexed: &Lexed) -> Vec<TokenKind> {
        lexed
            .tokens()
            .iter()
            .filter(|token| !token.is_synthetic() && !token.kind().is_trivia())
            .map(Token::kind)
            .collect()
    }

    fn logical_kinds(lexed: &Lexed) -> Vec<TokenKind> {
        lexed.tokens().iter().map(Token::kind).collect()
    }

    fn codes(lexed: &Lexed) -> Vec<&str> {
        lexed
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    fn assert_lossless(sources: &SourceDatabase, file: FileId, lexed: &Lexed) {
        let source = sources.get(file).unwrap();
        assert!(lexed.has_exact_physical_partition(source.length()));
        assert_eq!(lexed.reconstruct(source.bytes()), source.bytes());
    }

    #[test]
    fn normalization_tables_are_pinned_to_unicode_16() {
        assert_eq!(unicode_normalization::UNICODE_VERSION, (16, 0, 0));
    }

    #[test]
    fn identifiers_compare_as_nfc_but_preserve_original_bytes() {
        let input = "let cafe\u{301} = café\n";
        let (sources, file, lexed) = lex_bytes(input.as_bytes(), LexMode::Module);

        let identifiers = lexed
            .tokens()
            .iter()
            .filter_map(Token::normalized_identifier)
            .collect::<Vec<_>>();
        assert_eq!(identifiers, ["café", "café"]);
        assert_lossless(&sources, file, &lexed);
        assert!(lexed.diagnostics().is_empty());
    }

    #[test]
    fn every_reserved_word_has_a_distinct_keyword_token() {
        let spellings = [
            "alias", "and", "as", "async", "await", "break", "const", "continue", "defer", "else",
            "enum", "err", "fail", "false", "fn", "for", "if", "impl", "import", "in", "let",
            "match", "mut", "none", "not", "ok", "or", "priv", "pub", "ref", "return", "scope",
            "self", "some", "spawn", "trait", "true", "type", "unsafe", "var", "with",
        ];
        let input = spellings.join(" ");
        let (_, _, lexed) = lex_bytes(input.as_bytes(), LexMode::Module);
        let tokens = physical_significant_kinds(&lexed);

        assert_eq!(tokens.len(), spellings.len());
        assert!(tokens.iter().all(|kind| kind.is_keyword()));
        assert_eq!(
            tokens
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            spellings.len()
        );
    }

    #[test]
    fn maximal_munch_covers_the_complete_operator_inventory() {
        let input = "<<= >>= ... ..= => += -= *= /= %= &= ^= |= << >> <= >= == != .. ( ) [ ] { } , . : ? ! ~ = + - * / % & ^ | < >";
        let (_, _, lexed) = lex_bytes(input.as_bytes(), LexMode::Module);
        assert!(lexed.diagnostics().is_empty());
        assert_eq!(
            physical_significant_kinds(&lexed),
            [
                TokenKind::ShlEq,
                TokenKind::ShrEq,
                TokenKind::Ellipsis,
                TokenKind::DotDotEq,
                TokenKind::FatArrow,
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarEq,
                TokenKind::SlashEq,
                TokenKind::PercentEq,
                TokenKind::AmpEq,
                TokenKind::CaretEq,
                TokenKind::PipeEq,
                TokenKind::Shl,
                TokenKind::Shr,
                TokenKind::LessEq,
                TokenKind::GreaterEq,
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::DotDot,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::Comma,
                TokenKind::Dot,
                TokenKind::Colon,
                TokenKind::Question,
                TokenKind::Bang,
                TokenKind::Tilde,
                TokenKind::Eq,
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Amp,
                TokenKind::Caret,
                TokenKind::Pipe,
                TokenKind::Less,
                TokenKind::Greater,
            ]
        );
    }

    #[test]
    fn comments_are_nested_lossless_trivia_and_expose_physical_newlines() {
        let input = b"/* outer\n /* inner */\r\n end */\nlet value = 1\n";
        let (sources, file, lexed) = lex_bytes(&input[..], LexMode::Module);
        let newline_count = lexed
            .tokens()
            .iter()
            .filter(|token| token.kind() == TokenKind::PhysicalNewline)
            .count();

        assert_eq!(newline_count, 4);
        assert!(lexed.diagnostics().is_empty());
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn logical_newline_rules_are_closed_and_delimiter_aware() {
        let input =
            b"let a =\n  1 +\n  2\nlet b = (\n  a\n  + 3\n)\nlet c = {\n  a\n}\nreturn\nvalue\n";
        let (_, _, lexed) = lex_bytes(&input[..], LexMode::Module);
        let nl_count = lexed
            .tokens()
            .iter()
            .filter(|token| token.kind() == TokenKind::Nl)
            .count();

        // After `2`, `)`, `{`, `a`, `}`, `return`, and `value`.
        assert_eq!(nl_count, 7);
        assert!(lexed.diagnostics().is_empty());
    }

    #[test]
    fn nested_braces_restore_logical_newlines_inside_soft_delimiters() {
        let input = b"let values = [Point {\nx: 1\ny: (\n2 +\n3\n)\n}]\n";
        let (_, _, lexed) = lex_bytes(&input[..], LexMode::Module);
        let nl_count = lexed
            .tokens()
            .iter()
            .filter(|token| token.kind() == TokenKind::Nl)
            .count();

        // After `{`, `1`, `)`, and the outer `]`; the newlines inside the
        // nested parentheses remain suppressed.
        assert_eq!(nl_count, 4);
        assert!(lexed.diagnostics().is_empty());
    }

    #[test]
    fn comments_and_blank_lines_collapse_to_one_logical_newline_per_gap() {
        let input = b"let first = 1\n\n// note\n/* block\ncomment */\n\nlet second = 2";
        let (_, _, lexed) = lex_bytes(&input[..], LexMode::Module);
        let nl_offsets = lexed
            .tokens()
            .iter()
            .filter(|token| token.kind() == TokenKind::Nl)
            .map(|token| token.range().start())
            .collect::<Vec<_>>();

        assert_eq!(nl_offsets.len(), 2);
        assert_eq!(nl_offsets.last().copied(), Some(input.len() as u32));
    }

    #[test]
    fn missing_final_line_feed_gets_zero_width_nl_and_eof() {
        let input = b"let answer = 42";
        let (sources, file, lexed) = lex_bytes(&input[..], LexMode::Module);
        let tail = &lexed.tokens()[lexed.tokens().len() - 2..];

        assert_eq!(tail[0].kind(), TokenKind::Nl);
        assert_eq!(tail[1].kind(), TokenKind::Eof);
        assert!(tail.iter().all(Token::is_synthetic));
        assert_eq!(tail[0].range(), TextRange::empty(input.len() as u32));
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn invalid_utf8_is_diagnosed_at_original_bytes_without_replacement() {
        let input = [b'a', 0xf0, 0x28, 0x8c, 0x28, b'b'];
        let (sources, file, lexed) = lex_bytes(Arc::<[u8]>::from(input), LexMode::Module);

        assert_eq!(codes(&lexed), ["E0001", "E0001"]);
        let ranges = lexed
            .diagnostics()
            .iter()
            .map(|diagnostic| match diagnostic.location() {
                PrimaryLocation::Source(span) => span.range(),
                PrimaryLocation::Target(_) => {
                    panic!("lexer diagnostics must be source diagnostics")
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(ranges, [text_range(1, 2), text_range(3, 4)]);
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn invalid_tokens_recover_locally() {
        let input = "let a = 1;\rlet b = 2 -- 1 ?? 0\u{00a0}let c = 3";
        let (sources, file, lexed) = lex_bytes(input.as_bytes(), LexMode::Module);

        assert_eq!(codes(&lexed), ["E0002", "E0002", "E0002", "E0002", "E0002"]);
        assert_eq!(
            physical_significant_kinds(&lexed)
                .iter()
                .filter(|kind| **kind == TokenKind::Let)
                .count(),
            3
        );
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn integer_and_float_literals_accept_only_the_normative_forms() {
        let valid_integers = [
            "0",
            "42",
            "1_000_000",
            "0b1010_0110",
            "0o755",
            "0xFF_A0",
            "42i8",
            "42i16",
            "42i32",
            "42i64",
            "42u8",
            "42u16",
            "42u32",
            "42u64",
        ];
        let valid_floats = [
            "3.14", "1_000.25", "1.0e-9", "6.022e23", "3.14f32", "3.14f64",
        ];
        let malformed = [
            "00",
            "01",
            "0_1",
            "00.5",
            "1_",
            "1__0",
            "0b",
            "0b2",
            "0o8",
            "0xG",
            "42i32extra",
            "1e",
            "1e+",
            "1.0e_2",
            "1f32",
            "3.14f16",
        ];

        for spelling in valid_integers {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert_eq!(
                physical_significant_kinds(&lexed),
                [TokenKind::IntegerLiteral],
                "{spelling}"
            );
            assert!(lexed.diagnostics().is_empty(), "{spelling}");
        }
        for spelling in valid_floats {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert_eq!(
                physical_significant_kinds(&lexed),
                [TokenKind::FloatLiteral],
                "{spelling}"
            );
            assert!(lexed.diagnostics().is_empty(), "{spelling}");
        }
        for spelling in malformed {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert_eq!(
                physical_significant_kinds(&lexed),
                [TokenKind::MalformedLiteral],
                "{spelling}"
            );
            assert_eq!(codes(&lexed), ["E0003"], "{spelling}");
        }
    }

    #[test]
    fn character_literals_validate_scalars_and_escapes() {
        for spelling in ["'a'", "'ñ'", "'λ'", "'\\n'", "'\\u{1F642}'", "'\\0'"] {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert_eq!(
                physical_significant_kinds(&lexed),
                [TokenKind::CharLiteral],
                "{spelling}"
            );
            assert!(lexed.diagnostics().is_empty(), "{spelling}");
        }
        for spelling in [
            "''",
            "'ab'",
            "'\\x'",
            "'\\u{}'",
            "'\\u{110000}'",
            "'\\u{D800}'",
        ] {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert_eq!(
                physical_significant_kinds(&lexed),
                [TokenKind::MalformedLiteral],
                "{spelling}"
            );
            assert_eq!(codes(&lexed), ["E0003"], "{spelling}");
        }
    }

    #[test]
    fn strings_preserve_segments_and_lex_interpolated_expressions() {
        let input = b"\"hello {{ {user.name + nested(\"x\")} }}\"";
        let (sources, file, lexed) = lex_bytes(&input[..], LexMode::Fragment);
        let kinds = physical_significant_kinds(&lexed);

        assert_eq!(kinds.first(), Some(&TokenKind::StringStart));
        assert_eq!(kinds.last(), Some(&TokenKind::StringEnd));
        assert!(kinds.contains(&TokenKind::InterpolationStart));
        assert!(kinds.contains(&TokenKind::InterpolationEnd));
        assert!(kinds.contains(&TokenKind::Plus));
        assert!(lexed.diagnostics().is_empty());
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn raw_and_multiline_strings_follow_their_distinct_rules() {
        let valid = [
            r#"r"C:\users\name""#,
            "r\"\"\"\n    raw \\ { text }\n    \"\"\"",
            "r\"\"\"\r\n    raw \\ { text }\r\n    \"\"\"",
            "\"\"\"\n    first\n    second: {value}\n    \"\"\"",
            "\"\"\"\r\n    first\r\n    second: {value}\r\n    \"\"\"",
        ];
        for spelling in valid {
            let (sources, file, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert!(lexed.diagnostics().is_empty(), "{spelling:?}");
            assert_lossless(&sources, file, &lexed);
        }

        let malformed = [
            "r\"unterminated\n",
            "\"bad\\q\"",
            "\"single\nline\"",
            "\"\"\"\n    good\n  bad\n    \"\"\"",
            "r\"\"\"\n    good\n  bad\n    \"\"\"",
        ];
        for spelling in malformed {
            let (_, _, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert!(codes(&lexed).contains(&"E0003"), "{spelling:?}");
        }
    }

    #[test]
    fn empty_and_unterminated_interpolations_are_malformed_but_lossless() {
        for spelling in ["\"{}\"", "\"{   }\"", "\"{value\""] {
            let (sources, file, lexed) = lex_bytes(spelling.as_bytes(), LexMode::Fragment);
            assert!(codes(&lexed).contains(&"E0003"), "{spelling}");
            assert_lossless(&sources, file, &lexed);
        }
    }

    #[test]
    fn shebang_is_trivia_only_at_byte_zero_of_a_script() {
        let input = b"#!/usr/bin/env tondo\nlet answer = 42\n";
        let (_, _, script) = lex_bytes(&input[..], LexMode::Script);
        let (_, _, module) = lex_bytes(&input[..], LexMode::Module);

        assert_eq!(script.tokens()[0].kind(), TokenKind::Shebang);
        assert!(script.diagnostics().is_empty());
        assert_eq!(module.tokens()[0].kind(), TokenKind::InvalidToken);
        assert_eq!(codes(&module), ["E0002"]);
    }

    #[test]
    fn unterminated_nested_comment_reports_one_local_error() {
        let input = b"let x = 1 /* outer /* inner */";
        let (sources, file, lexed) = lex_bytes(&input[..], LexMode::Module);
        assert_eq!(codes(&lexed), ["E0002"]);
        assert_lossless(&sources, file, &lexed);
    }

    #[test]
    fn arbitrary_bytes_never_break_partition_or_progress() {
        let mut samples = (0_u8..=u8::MAX).map(|byte| vec![byte]).collect::<Vec<_>>();
        let mut state = 0x9e37_79b9_u32;
        for length in 0..512_usize {
            let mut sample = Vec::with_capacity(length % 31);
            for _ in 0..sample.capacity() {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                sample.push((state >> 24) as u8);
            }
            samples.push(sample);
        }

        for sample in samples {
            let (sources, file, lexed) = lex_bytes(Arc::<[u8]>::from(sample), LexMode::Script);
            assert_lossless(&sources, file, &lexed);
            assert_eq!(logical_kinds(&lexed).last(), Some(&TokenKind::Eof));
        }
    }

    #[test]
    fn lexer_limits_fail_explicitly_before_unbounded_growth() {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:limit-test").unwrap(),
                ModulePath::new("limit").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                Arc::<[u8]>::from(&b"a"[..]),
            ))
            .unwrap();
        let token_error = lex_with_limits(
            &sources,
            file,
            LexMode::Module,
            LexLimits {
                max_tokens: 2,
                ..LexLimits::UNLIMITED
            },
        )
        .unwrap_err();
        assert!(matches!(
            token_error,
            LexError::ResourceLimit {
                resource: LexResource::Tokens,
                ..
            }
        ));

        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:limit-test").unwrap(),
                ModulePath::new("limit").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                Arc::<[u8]>::from(&b";;"[..]),
            ))
            .unwrap();
        let diagnostic_error = lex_with_limits(
            &sources,
            file,
            LexMode::Module,
            LexLimits {
                max_diagnostics: 1,
                ..LexLimits::UNLIMITED
            },
        )
        .unwrap_err();
        assert!(matches!(
            diagnostic_error,
            LexError::ResourceLimit {
                resource: LexResource::Diagnostics,
                ..
            }
        ));

        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:limit-test").unwrap(),
                ModulePath::new("limit").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                Arc::<[u8]>::from(&b"/* /* nested */ */"[..]),
            ))
            .unwrap();
        let depth_error = lex_with_limits(
            &sources,
            file,
            LexMode::Module,
            LexLimits {
                max_nesting_depth: 1,
                ..LexLimits::UNLIMITED
            },
        )
        .unwrap_err();
        assert!(matches!(
            depth_error,
            LexError::ResourceLimit {
                resource: LexResource::NestingDepth,
                ..
            }
        ));
    }
}
