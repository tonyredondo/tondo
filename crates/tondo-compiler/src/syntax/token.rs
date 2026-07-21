use crate::source::TextRange;

/// A lexical token kind in the lossless Tondo token stream.
///
/// Physical tokens partition the input bytes. `Nl`, `Eof`, and parser recovery
/// tokens are synthetic and therefore own no bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenKind {
    // Trivia.
    Whitespace,
    PhysicalNewline,
    LineComment,
    DocComment,
    BlockComment,
    Shebang,

    // Recovery tokens.
    InvalidUtf8,
    InvalidToken,
    MalformedLiteral,

    // Names and literals.
    Identifier,
    IntegerLiteral,
    FloatLiteral,
    CharLiteral,
    RawStringLiteral,
    RawMultilineStringLiteral,
    StringStart,
    MultilineStringStart,
    StringText,
    InterpolationStart,
    InterpolationEnd,
    StringEnd,
    MultilineStringEnd,

    // Keywords.
    Alias,
    And,
    As,
    Async,
    Await,
    Break,
    Const,
    Continue,
    Defer,
    Else,
    Enum,
    Err,
    Fail,
    False,
    Fn,
    For,
    If,
    Impl,
    Import,
    In,
    Let,
    Match,
    Mut,
    None,
    Not,
    Ok,
    Or,
    Priv,
    Pub,
    Ref,
    Return,
    Scope,
    SelfKw,
    Some,
    Spawn,
    Trait,
    True,
    Type,
    Unsafe,
    Var,
    With,

    // Delimiters and punctuation.
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    Colon,
    Question,
    Bang,
    Tilde,

    // Operators.
    Ellipsis,
    DotDot,
    DotDotEq,
    FatArrow,
    Eq,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Caret,
    Pipe,
    Shl,
    Shr,
    Less,
    LessEq,
    Greater,
    GreaterEq,
    EqEq,
    BangEq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    CaretEq,
    PipeEq,
    ShlEq,
    ShrEq,

    // Synthetic lexer/parser structure.
    Nl,
    Eof,
}

impl TokenKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace
                | Self::PhysicalNewline
                | Self::LineComment
                | Self::DocComment
                | Self::BlockComment
                | Self::Shebang
        )
    }

    pub fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::Alias
                | Self::And
                | Self::As
                | Self::Async
                | Self::Await
                | Self::Break
                | Self::Const
                | Self::Continue
                | Self::Defer
                | Self::Else
                | Self::Enum
                | Self::Err
                | Self::Fail
                | Self::False
                | Self::Fn
                | Self::For
                | Self::If
                | Self::Impl
                | Self::Import
                | Self::In
                | Self::Let
                | Self::Match
                | Self::Mut
                | Self::None
                | Self::Not
                | Self::Ok
                | Self::Or
                | Self::Priv
                | Self::Pub
                | Self::Ref
                | Self::Return
                | Self::Scope
                | Self::SelfKw
                | Self::Some
                | Self::Spawn
                | Self::Trait
                | Self::True
                | Self::Type
                | Self::Unsafe
                | Self::Var
                | Self::With
        )
    }

    pub(crate) fn from_keyword(normalized: &str) -> Option<Self> {
        Some(match normalized {
            "alias" => Self::Alias,
            "and" => Self::And,
            "as" => Self::As,
            "async" => Self::Async,
            "await" => Self::Await,
            "break" => Self::Break,
            "const" => Self::Const,
            "continue" => Self::Continue,
            "defer" => Self::Defer,
            "else" => Self::Else,
            "enum" => Self::Enum,
            "err" => Self::Err,
            "fail" => Self::Fail,
            "false" => Self::False,
            "fn" => Self::Fn,
            "for" => Self::For,
            "if" => Self::If,
            "impl" => Self::Impl,
            "import" => Self::Import,
            "in" => Self::In,
            "let" => Self::Let,
            "match" => Self::Match,
            "mut" => Self::Mut,
            "none" => Self::None,
            "not" => Self::Not,
            "ok" => Self::Ok,
            "or" => Self::Or,
            "priv" => Self::Priv,
            "pub" => Self::Pub,
            "ref" => Self::Ref,
            "return" => Self::Return,
            "scope" => Self::Scope,
            "self" => Self::SelfKw,
            "some" => Self::Some,
            "spawn" => Self::Spawn,
            "trait" => Self::Trait,
            "true" => Self::True,
            "type" => Self::Type,
            "unsafe" => Self::Unsafe,
            "var" => Self::Var,
            "with" => Self::With,
            _ => return None,
        })
    }

    pub(crate) fn suppresses_newline_after(self) -> bool {
        matches!(
            self,
            Self::Comma
                | Self::Dot
                | Self::Colon
                | Self::FatArrow
                | Self::Bang
                | Self::Eq
                | Self::PlusEq
                | Self::MinusEq
                | Self::StarEq
                | Self::SlashEq
                | Self::PercentEq
                | Self::AmpEq
                | Self::CaretEq
                | Self::PipeEq
                | Self::ShlEq
                | Self::ShrEq
                | Self::Plus
                | Self::Minus
                | Self::Star
                | Self::Slash
                | Self::Percent
                | Self::Shl
                | Self::Shr
                | Self::Amp
                | Self::Caret
                | Self::Pipe
                | Self::DotDot
                | Self::DotDotEq
                | Self::Less
                | Self::LessEq
                | Self::Greater
                | Self::GreaterEq
                | Self::EqEq
                | Self::BangEq
                | Self::In
                | Self::And
                | Self::Or
                | Self::With
                | Self::Not
                | Self::Tilde
                | Self::Await
                | Self::Spawn
                | Self::Fail
                | Self::Defer
        )
    }

    pub(crate) fn suppresses_newline_before(self) -> bool {
        matches!(
            self,
            Self::Dot
                | Self::Plus
                | Self::Minus
                | Self::Star
                | Self::Slash
                | Self::Percent
                | Self::Shl
                | Self::Shr
                | Self::Amp
                | Self::Caret
                | Self::Pipe
                | Self::DotDot
                | Self::DotDotEq
                | Self::Less
                | Self::LessEq
                | Self::Greater
                | Self::GreaterEq
                | Self::EqEq
                | Self::BangEq
                | Self::In
                | Self::And
                | Self::Or
                | Self::With
        )
    }
}

/// Additional normalized data carried by selected token kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenData {
    None,
    Identifier { normalized: Box<str> },
}

/// One token in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    kind: TokenKind,
    range: TextRange,
    synthetic: bool,
    data: TokenData,
}

impl Token {
    pub(crate) fn physical(kind: TokenKind, range: TextRange) -> Self {
        Self {
            kind,
            range,
            synthetic: false,
            data: TokenData::None,
        }
    }

    pub(crate) fn identifier(range: TextRange, normalized: Box<str>) -> Self {
        Self {
            kind: TokenKind::Identifier,
            range,
            synthetic: false,
            data: TokenData::Identifier { normalized },
        }
    }

    pub(crate) fn synthetic(kind: TokenKind, offset: u32) -> Self {
        Self {
            kind,
            range: TextRange::empty(offset),
            synthetic: true,
            data: TokenData::None,
        }
    }

    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }

    pub fn data(&self) -> &TokenData {
        &self.data
    }

    pub fn normalized_identifier(&self) -> Option<&str> {
        match &self.data {
            TokenData::Identifier { normalized } => Some(normalized),
            TokenData::None => None,
        }
    }
}
