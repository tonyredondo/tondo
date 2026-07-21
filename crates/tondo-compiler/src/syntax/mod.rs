//! Lossless lexical and concrete-syntax support.

pub mod ast;
mod cst;
mod format;
mod lexer;
mod parser;
mod token;

pub use cst::{
    Cst, DescendantTokens, NodeId, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxNodeRef,
    SyntaxTokenRef, TokenId,
};
pub use format::{FormatError, FormattedSource, format_parsed};
pub use lexer::{LexError, LexLimits, LexMode, LexResource, Lexed, lex, lex_with_limits};
pub use parser::{ParseError, ParseLimits, ParseMode, ParseResource, Parsed, parse};
pub use token::{Token, TokenData, TokenKind};
