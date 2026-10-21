//! The Tondo compiler.

pub mod ast;
pub mod check;
pub mod codegen;
pub mod diag;
pub mod driver;
pub mod exhaust;
pub mod hir;
pub mod lexer;
pub mod lower;
pub mod mir;
pub mod modules;
pub mod mono;
pub mod ownership;
pub mod parser;
pub mod project;
pub mod source;
pub mod types;
