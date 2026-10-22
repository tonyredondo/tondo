//! The Tondo compiler.

pub mod ast;
pub mod bind;
pub mod check;
pub mod codegen;
pub mod deps;
pub mod derive;
pub mod diag;
pub mod driver;
pub mod exhaust;
pub mod fmt;
pub mod hash;
pub mod hir;
pub mod lexer;
pub mod lower;
pub mod meta;
pub mod mir;
pub mod modules;
pub mod mono;
pub mod ownership;
pub mod parser;
pub mod project;
pub mod runner;
pub mod source;
pub mod stdlib;
pub mod toml;
pub mod types;
