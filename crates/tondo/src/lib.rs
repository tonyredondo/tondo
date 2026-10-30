//! The Tondo compiler.

pub mod api;
pub mod ast;
pub mod bind;
pub mod built;
pub mod check;
pub mod cli;
pub mod codegen;
pub mod compact;
pub mod dap;
pub mod debug;
pub mod deps;
pub mod derive;
pub mod diag;
pub mod doctest;
pub mod driver;
pub mod elide;
pub mod embed;
pub mod exhaust;
pub mod explain;
pub mod fmt;
pub mod graph;
pub mod guide;
pub mod hash;
pub mod header;
pub mod hir;
pub mod index;
pub mod inline;
pub mod lexer;
pub mod link;
pub mod live;
pub mod lower;
pub mod lsp;
pub mod manage;
pub mod mcp;
pub mod meta;
pub mod mir;
pub mod modules;
pub mod mono;
pub mod moves;
pub mod numbers;
pub mod observe;
pub mod ownership;
pub mod parallel;
pub mod parser;
#[path = "../../tondo-runtime/src/pprof.rs"]
pub mod pprof;
pub mod profile;
pub mod project;
pub mod prove;
// Its kinds of error are the runtime's; the compiler reads only their names.
#[allow(dead_code)]
#[path = "../../tondo-runtime/src/regex_dialect.rs"]
mod regex_dialect;
pub mod runner;
pub mod setup;
pub mod source;
pub mod stdlib;
pub mod target;
#[path = "../../tondo-runtime/src/template_syntax.rs"]
pub mod template_syntax;
pub mod templates;
pub mod thread;
/// The patterns of `std.time`, which the checker checks in a literal
/// (standard library 14.3.5).
mod time_pattern;
pub mod toml;
pub mod types;
pub mod unroll;
