use std::error::Error;
use std::fmt;

use crate::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticError, PrimaryLocation, Severity};
use crate::source::{FileId, SourceDatabase, SourceError};

use super::cst::{Checkpoint, CstBuilder, TokenId};
use super::{Cst, Lexed, SyntaxKind, Token, TokenKind};

const DEFAULT_MAX_NESTING_DEPTH: u32 = 256;
// The ordinary recursive-descent path is deliberately kept shallow. Once this
// fixed implementation detail is reached, expression continuations live in a
// heap-backed frame stack instead of consuming the host thread stack. This is
// not a language limit: `ParseLimits.max_nesting_depth` remains the only
// user-visible nesting budget.
const RECURSIVE_SPILL_DEPTH: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Module,
    ImportedModule,
    Script,
    Fragment,
    SyntaxSequence,
    StandaloneBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseLimits {
    pub max_nodes: u32,
    pub max_nesting_depth: u32,
    pub max_diagnostics: u32,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_nodes: 4_000_000,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
            max_diagnostics: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseResource {
    Nodes,
    NestingDepth,
    Diagnostics,
}

impl fmt::Display for ParseResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nodes => formatter.write_str("syntax node count"),
            Self::NestingDepth => formatter.write_str("parser nesting depth"),
            Self::Diagnostics => formatter.write_str("primary diagnostic count"),
        }
    }
}

#[derive(Debug)]
pub enum ParseError {
    Source(SourceError),
    Diagnostic(DiagnosticError),
    ResourceLimit {
        resource: ParseResource,
        offset: u32,
    },
}

impl fmt::Display for ParseError {
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

impl Error for ParseError {}

impl From<SourceError> for ParseError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<DiagnosticError> for ParseError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

#[derive(Debug)]
pub struct Parsed {
    cst: Cst,
    diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    pub fn cst(&self) -> &Cst {
        &self.cst
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_parts(self) -> (Cst, Vec<Diagnostic>) {
        (self.cst, self.diagnostics)
    }
}

pub fn parse(
    sources: &SourceDatabase,
    file: FileId,
    lexed: Lexed,
    mode: ParseMode,
    limits: ParseLimits,
) -> Result<Parsed, ParseError> {
    let (tokens, mut diagnostics) = lexed.into_parts();
    let original_token_count = tokens.len();
    let significant_indices = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| (!token.kind().is_trivia()).then_some(index))
        .collect();
    let mut parser = Parser {
        sources,
        file,
        mode,
        limits,
        builder: CstBuilder::new(tokens),
        original_token_count,
        significant_indices,
        significant_cursor: 0,
        cursor: 0,
        diagnostics: Vec::new(),
        nodes_started: 0,
        depth: 0,
        recursion_depth: 0,
        type_recursion_depth: 0,
        pattern_recursion_depth: 0,
        if_recursion_depth: 0,
        for_recursion_depth: 0,
        assignment_recursion_depth: 0,
        header_expression_depth: 0,
        spill_active: false,
        type_spill_active: false,
        pattern_spill_active: false,
        suppress_syntax_errors: false,
        logical_newlines_consumed: 0,
    };
    parser.parse_program()?;
    diagnostics.extend(parser.diagnostics);
    Ok(Parsed {
        cst: parser.builder.build(),
        diagnostics,
    })
}

type ParseResult<T = ()> = Result<T, ParseError>;

struct Parser<'a> {
    sources: &'a SourceDatabase,
    file: FileId,
    mode: ParseMode,
    limits: ParseLimits,
    builder: CstBuilder,
    original_token_count: usize,
    significant_indices: Vec<usize>,
    significant_cursor: usize,
    cursor: usize,
    diagnostics: Vec<Diagnostic>,
    nodes_started: u32,
    depth: u32,
    recursion_depth: u32,
    type_recursion_depth: u32,
    pattern_recursion_depth: u32,
    if_recursion_depth: u32,
    for_recursion_depth: u32,
    assignment_recursion_depth: u32,
    header_expression_depth: u32,
    spill_active: bool,
    type_spill_active: bool,
    pattern_spill_active: bool,
    suppress_syntax_errors: bool,
    logical_newlines_consumed: u32,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> ParseResult {
        if self.mode == ParseMode::SyntaxSequence {
            return self.parse_syntax_sequence();
        }
        if self.mode == ParseMode::StandaloneBlock {
            return self.parse_standalone_block();
        }
        let root = match self.mode {
            ParseMode::Module | ParseMode::ImportedModule => SyntaxKind::Module,
            ParseMode::Script => SyntaxKind::Script,
            ParseMode::Fragment => SyntaxKind::Fragment,
            ParseMode::SyntaxSequence | ParseMode::StandaloneBlock => unreachable!(),
        };
        self.start(root)?;

        while !self.at(TokenKind::Eof) {
            if self.at(TokenKind::Nl) {
                self.bump();
                continue;
            }
            if self.at(TokenKind::Import) {
                self.parse_import_decl()?;
                continue;
            }
            if self.at_top_decl_start() {
                self.parse_top_decl()?;
                continue;
            }

            let range = self.current_token().range();
            let actual = self.current();
            let diagnostics_before = self.diagnostics.len();
            let newlines_before = self.logical_newlines_consumed;
            self.parse_statement()?;
            let had_syntax_error = self.diagnostics[diagnostics_before..]
                .iter()
                .any(|diagnostic| diagnostic.code().as_str().starts_with("E000"));
            if had_syntax_error && self.logical_newlines_consumed == newlines_before {
                self.recover_to_statement_boundary()?;
            }
            if matches!(self.mode, ParseMode::Module | ParseMode::ImportedModule)
                && !had_syntax_error
            {
                let (code, message) = if self.mode == ParseMode::ImportedModule {
                    (
                        "E1801",
                        "a file with script statements cannot be imported as a module",
                    )
                } else {
                    (
                        "E1804",
                        "top-level statements are only allowed in a script root",
                    )
                };
                self.push_diagnostic_at(code, message, None, range, actual)?;
            }
        }
        self.expect(TokenKind::Eof)?;
        self.finish();
        Ok(())
    }

    fn parse_syntax_sequence(&mut self) -> ParseResult {
        self.start(SyntaxKind::SyntaxSequence)?;
        while !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            if self.at_function_signature_start() && !self.function_item_has_body() {
                self.parse_function_signature()?;
            } else if self.line_requires_type_production() {
                self.parse_type_expr()?;
                self.expect_line_end()?;
            } else if self.at_top_decl_start() {
                self.parse_top_decl()?;
            } else if self.line_requires_pattern_production() {
                self.parse_pattern()?;
                self.expect_line_end()?;
            } else {
                self.parse_statement()?;
            }
        }
        self.expect(TokenKind::Eof)?;
        self.finish();
        Ok(())
    }

    fn parse_standalone_block(&mut self) -> ParseResult {
        self.start(SyntaxKind::StandaloneBlock)?;
        self.eat_newlines();
        self.parse_block()?;
        self.eat_newlines();
        self.expect(TokenKind::Eof)?;
        self.finish();
        Ok(())
    }

    fn parse_function_signature(&mut self) -> ParseResult {
        self.start(SyntaxKind::FunctionSignature)?;
        self.parse_visibility()?;
        self.parse_function_modifiers();
        self.expect(TokenKind::Fn)?;
        self.start(SyntaxKind::FunctionHead)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        if self.eat(TokenKind::Dot) {
            self.expect_identifier()?;
            if self.at(TokenKind::LBracket) {
                self.parse_generic_params()?;
            }
        }
        self.finish();
        self.parse_parameter_list()?;
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(true)?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_import_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::ImportDecl)?;
        self.expect(TokenKind::Import)?;
        self.parse_module_path()?;
        if self.eat(TokenKind::As) {
            self.expect_identifier()?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_top_decl(&mut self) -> ParseResult {
        match self.top_decl_discriminator() {
            Some(TokenKind::Const) => self.parse_const_decl(),
            Some(TokenKind::Type) => self.parse_type_decl(),
            Some(TokenKind::Alias) => self.parse_alias_decl(),
            Some(TokenKind::Enum) => self.parse_enum_decl(),
            Some(TokenKind::Trait) => self.parse_trait_decl(),
            Some(TokenKind::Impl) => self.parse_impl_decl(),
            Some(TokenKind::Fn | TokenKind::Async | TokenKind::Unsafe) => {
                self.parse_function_decl()
            }
            Some(TokenKind::Test) => self.parse_test_decl(),
            Some(TokenKind::Suite) => self.parse_suite_decl(),
            _ => {
                self.syntax_error("expected a top-level declaration")?;
                self.recover_one()
            }
        }
    }

    fn parse_visibility(&mut self) -> ParseResult {
        if self.at(TokenKind::Pub) {
            self.start(SyntaxKind::Visibility)?;
            self.bump();
            self.finish();
        }
        Ok(())
    }

    fn parse_const_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::ConstDecl)?;
        self.parse_visibility()?;
        self.expect(TokenKind::Const)?;
        self.expect_identifier()?;
        if self.eat(TokenKind::Colon) {
            self.parse_type_expr()?;
        }
        self.expect(TokenKind::Eq)?;
        self.parse_expression()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_type_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::TypeDecl)?;
        self.parse_visibility()?;
        self.expect(TokenKind::Type)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.expect(TokenKind::Eq)?;
        if self.at(TokenKind::LBrace) {
            self.parse_record_body(true)?;
        } else {
            self.parse_type_expr()?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_alias_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::AliasDecl)?;
        self.parse_visibility()?;
        self.expect(TokenKind::Alias)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.expect(TokenKind::Eq)?;
        self.parse_type_expr()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_enum_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::EnumDecl)?;
        self.parse_visibility()?;
        self.expect(TokenKind::Enum)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        if self.at(TokenKind::RBrace) {
            self.syntax_error("an enum requires at least one variant")?;
        } else {
            loop {
                self.start(SyntaxKind::EnumVariant)?;
                self.expect_identifier()?;
                if self.at(TokenKind::LParen) {
                    self.parse_tuple_payload()?;
                } else if self.at(TokenKind::LBrace) {
                    self.parse_record_body(false)?;
                }
                self.finish();
                if !self.parse_field_separator()? {
                    break;
                }
                if self.at(TokenKind::RBrace) {
                    break;
                }
            }
        }
        self.eat_newlines();
        self.expect(TokenKind::RBrace)?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_trait_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::TraitDecl)?;
        self.parse_visibility()?;
        self.expect(TokenKind::Trait)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.expect(TokenKind::LBrace)?;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            if !self.at_method_start() {
                self.syntax_error("expected a trait method")?;
                self.recover_to_member_boundary()?;
                continue;
            }
            self.parse_trait_method()?;
        }
        self.expect(TokenKind::RBrace)?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_trait_method(&mut self) -> ParseResult {
        self.start(SyntaxKind::TraitMethod)?;
        self.parse_function_modifiers();
        self.expect(TokenKind::Fn)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.parse_parameter_list()?;
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(false)?;
        }
        if self.at(TokenKind::LBrace) {
            self.parse_block()?;
            self.expect_line_end()?;
        } else {
            self.expect_line_end()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_impl_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::ImplDecl)?;
        self.expect(TokenKind::Impl)?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.parse_type_path()?;
        self.expect(TokenKind::For)?;
        self.parse_type_expr()?;
        self.expect(TokenKind::LBrace)?;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            if !self.at_method_start() {
                self.syntax_error("expected an implementation method")?;
                self.recover_to_member_boundary()?;
                continue;
            }
            self.parse_implementation_method()?;
        }
        self.expect(TokenKind::RBrace)?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_implementation_method(&mut self) -> ParseResult {
        self.start(SyntaxKind::ImplementationMethod)?;
        self.parse_function_modifiers();
        self.expect(TokenKind::Fn)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        self.parse_parameter_list()?;
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(false)?;
        }
        self.parse_block()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_function_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::FunctionDecl)?;
        self.parse_visibility()?;
        self.parse_function_modifiers();
        self.expect(TokenKind::Fn)?;
        self.start(SyntaxKind::FunctionHead)?;
        self.expect_identifier()?;
        if self.at(TokenKind::LBracket) {
            self.parse_generic_params()?;
        }
        if self.eat(TokenKind::Dot) {
            self.expect_identifier()?;
            if self.at(TokenKind::LBracket) {
                self.parse_generic_params()?;
            }
        }
        self.finish();
        self.parse_parameter_list()?;
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(true)?;
        }
        self.parse_block()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_test_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::TestDecl)?;
        self.parse_test_suite_modifiers()?;
        self.expect(TokenKind::Test)?;
        self.expect_identifier()?;
        self.parse_block()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_suite_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::SuiteDecl)?;
        self.parse_test_suite_modifiers()?;
        self.expect(TokenKind::Suite)?;
        self.expect_identifier()?;
        self.parse_suite_block()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_test_suite_modifiers(&mut self) -> ParseResult {
        if !self.at_any(&[
            TokenKind::Pub,
            TokenKind::Priv,
            TokenKind::Async,
            TokenKind::Unsafe,
        ]) {
            return Ok(());
        }
        self.syntax_error("test and suite declarations do not accept modifiers")?;
        while self.at_any(&[
            TokenKind::Pub,
            TokenKind::Priv,
            TokenKind::Async,
            TokenKind::Unsafe,
        ]) {
            self.bump();
        }
        Ok(())
    }

    fn parse_suite_block(&mut self) -> ParseResult {
        self.start(SyntaxKind::SuiteBlock)?;
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        let mut has_member = false;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            if let Some(member) = self.suite_member_discriminator() {
                has_member = true;
                self.parse_suite_member(member)?;
                continue;
            }
            if has_member {
                self.syntax_error(
                    "only test or suite declarations may follow the first suite member",
                )?;
                self.recover_to_suite_boundary()?;
            } else {
                self.parse_statement()?;
            }
        }
        self.expect(TokenKind::RBrace)?;
        self.finish();
        Ok(())
    }

    fn parse_suite_member(&mut self, member: TokenKind) -> ParseResult {
        match member {
            TokenKind::Test => self.parse_test_decl(),
            TokenKind::Suite => self.parse_suite_decl(),
            _ => {
                self.syntax_error("expected a test or suite member")?;
                self.recover_to_suite_boundary()
            }
        }
    }

    fn parse_function_modifiers(&mut self) {
        self.eat(TokenKind::Async);
        self.eat(TokenKind::Unsafe);
    }

    fn parse_generic_params(&mut self) -> ParseResult {
        self.start(SyntaxKind::GenericParams)?;
        self.expect(TokenKind::LBracket)?;
        loop {
            self.start(SyntaxKind::GenericParam)?;
            self.expect_identifier()?;
            if self.eat(TokenKind::Colon) {
                self.parse_generic_bound()?;
            }
            self.finish();
            if !self.eat(TokenKind::Comma) {
                break;
            }
            if self.at(TokenKind::RBracket) {
                break;
            }
        }
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_generic_bound(&mut self) -> ParseResult {
        self.start(SyntaxKind::GenericBound)?;
        self.parse_type_path()?;
        while self.eat(TokenKind::Plus) {
            self.parse_type_path()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_generic_args(&mut self) -> ParseResult {
        self.start(SyntaxKind::GenericArgs)?;
        self.expect(TokenKind::LBracket)?;
        self.parse_type_expr()?;
        while self.eat(TokenKind::Comma) {
            if self.at(TokenKind::RBracket) {
                break;
            }
            self.parse_type_expr()?;
        }
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_module_path(&mut self) -> ParseResult {
        self.start(SyntaxKind::ModulePath)?;
        self.expect_identifier()?;
        while self.eat(TokenKind::Dot) {
            self.expect_identifier()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_type_path(&mut self) -> ParseResult {
        self.start(SyntaxKind::TypePath)?;
        self.expect_identifier()?;
        while self.eat(TokenKind::Dot) {
            self.expect_identifier()?;
        }
        if self.at(TokenKind::LBracket) {
            self.parse_generic_args()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_type_expr(&mut self) -> ParseResult {
        if self.type_recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        if self.type_spill_active
            || self.type_recursion_depth >= RECURSIVE_SPILL_DEPTH
            || self.deep_generic_type_chain_len() >= 4
            || self.deep_parenthesized_type_chain_len() >= 4
        {
            return self.parse_type_expr_spilled();
        }
        self.type_recursion_depth += 1;
        let result = self.parse_type_expr_recursive();
        self.type_recursion_depth -= 1;
        result
    }

    fn parse_type_expr_recursive(&mut self) -> ParseResult {
        self.start(SyntaxKind::TypeExpr)?;
        self.start(SyntaxKind::UnionType)?;
        self.parse_result_type()?;
        while self.eat(TokenKind::Pipe) {
            self.parse_result_type()?;
        }
        self.finish();
        self.finish();
        Ok(())
    }

    fn parse_type_expr_spilled(&mut self) -> ParseResult {
        let was_spilling = self.type_spill_active;
        self.type_spill_active = true;
        let result = if self.deep_generic_type_chain_len() >= 4 {
            self.parse_deep_generic_type_chain()
        } else {
            let layers = self.deep_parenthesized_type_chain_len();
            if layers >= 4 {
                self.parse_deep_parenthesized_type_chain(layers)
            } else {
                self.type_spill_active = false;
                let result = self.parse_type_expr_recursive();
                self.type_spill_active = true;
                result
            }
        };
        self.type_spill_active = was_spilling;
        result
    }

    fn parse_deep_parenthesized_type_chain(&mut self, layers: usize) -> ParseResult {
        if layers as u32 >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        for _ in 0..layers {
            self.start(SyntaxKind::GroupType)?;
            self.expect(TokenKind::LParen)?;
        }
        self.parse_type_expr()?;
        for _ in 0..layers {
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_deep_generic_type_chain(&mut self) -> ParseResult {
        let estimated_layers = self.deep_generic_type_chain_len();
        if estimated_layers as u32 >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        let mut layers = 0_usize;
        loop {
            if self.nth(1) != TokenKind::LBracket {
                break;
            }
            self.start(SyntaxKind::TypeExpr)?;
            self.start(SyntaxKind::UnionType)?;
            self.start(SyntaxKind::ResultType)?;
            self.start(SyntaxKind::OptionalType)?;
            self.start(SyntaxKind::PathType)?;
            self.start(SyntaxKind::TypePath)?;
            self.expect_identifier()?;
            if !self.at(TokenKind::LBracket) {
                self.finish();
                self.finish();
                if self.eat(TokenKind::Question) {
                    // The question belongs to this optional layer.
                }
                self.finish();
                self.finish();
                self.finish();
                self.finish();
                break;
            }
            self.start(SyntaxKind::GenericArgs)?;
            self.expect(TokenKind::LBracket)?;
            layers += 1;
        }

        self.parse_type_expr()?;
        for _ in 0..layers {
            while self.eat(TokenKind::Comma) {
                self.parse_type_expr()?;
            }
            self.expect(TokenKind::RBracket)?;
            self.finish();
            self.finish();
            self.finish();
            if self.eat(TokenKind::Question) {
                // The question belongs to the optional layer being closed.
            }
            self.finish();
            self.finish();
            self.finish();
            self.finish();
        }
        Ok(())
    }

    fn deep_generic_type_chain_len(&self) -> usize {
        let mut offset = 0;
        let mut layers = 0;
        loop {
            if self.nth(offset) != TokenKind::Identifier
                || self.nth(offset + 1) != TokenKind::LBracket
            {
                break;
            }
            layers += 1;
            offset += 2;
            if self.nth(offset) != TokenKind::Identifier {
                break;
            }
        }
        layers
    }

    fn deep_parenthesized_type_chain_len(&self) -> usize {
        let mut offset = 0;
        let mut layers = 0;
        while self.nth(offset) == TokenKind::LParen {
            layers += 1;
            offset += 1;
        }
        if layers < 4 {
            return layers;
        }
        let mut depth = layers;
        while depth > 0 {
            match self.nth(offset) {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                TokenKind::Comma => return 0,
                TokenKind::Eof | TokenKind::Nl => break,
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                break;
            }
        }
        layers
    }

    fn parse_result_type(&mut self) -> ParseResult {
        self.start(SyntaxKind::ResultType)?;
        if self.eat(TokenKind::Bang) {
            self.parse_error_type_operand()?;
        } else {
            self.parse_optional_type()?;
            if self.eat(TokenKind::Bang) {
                self.parse_error_type_operand()?;
            }
        }
        self.finish();
        Ok(())
    }

    fn parse_error_type_operand(&mut self) -> ParseResult {
        if self.at(TokenKind::LParen) {
            self.start(SyntaxKind::GroupType)?;
            self.bump();
            self.parse_type_expr()?;
            self.expect(TokenKind::RParen)?;
            self.finish();
        } else {
            self.parse_optional_type()?;
        }
        Ok(())
    }

    fn parse_optional_type(&mut self) -> ParseResult {
        self.start(SyntaxKind::OptionalType)?;
        self.parse_primary_type()?;
        self.eat(TokenKind::Question);
        self.finish();
        Ok(())
    }

    fn parse_primary_type(&mut self) -> ParseResult {
        if self.at(TokenKind::Impl) {
            self.start(SyntaxKind::Error)?;
            self.syntax_error("`impl Bound` is only valid as an admitted declaration outcome")?;
            self.bump();
            if self.at(TokenKind::Identifier) {
                self.parse_generic_bound()?;
                if self.eat(TokenKind::Bang) {
                    self.parse_error_type_operand()?;
                }
            }
            self.finish();
            return Ok(());
        }
        if self.at_function_type_start() {
            return self.parse_function_type();
        }
        if self.at(TokenKind::LParen) {
            let checkpoint = self.checkpoint();
            self.bump();
            if self.at(TokenKind::RParen) {
                self.start_at(checkpoint, SyntaxKind::TupleType)?;
                self.syntax_error("the unit value is not a type")?;
                self.bump();
                self.finish();
                return Ok(());
            }
            self.parse_type_expr()?;
            if self.eat(TokenKind::Comma) {
                self.start_at(checkpoint, SyntaxKind::TupleType)?;
                self.parse_type_expr()?;
                while self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    self.parse_type_expr()?;
                }
                self.expect(TokenKind::RParen)?;
                self.finish();
            } else {
                self.start_at(checkpoint, SyntaxKind::GroupType)?;
                self.expect(TokenKind::RParen)?;
                self.finish();
            }
            return Ok(());
        }
        self.start(SyntaxKind::PathType)?;
        self.parse_type_path()?;
        self.finish();
        Ok(())
    }

    fn parse_function_type(&mut self) -> ParseResult {
        self.start(SyntaxKind::FunctionType)?;
        self.parse_function_modifiers();
        self.expect(TokenKind::Fn)?;
        self.expect(TokenKind::LParen)?;
        if !self.at(TokenKind::RParen) {
            self.start(SyntaxKind::FunctionTypeList)?;
            loop {
                self.start(SyntaxKind::FunctionTypeItem)?;
                if self.eat(TokenKind::Ellipsis) {
                    self.parse_type_expr()?;
                } else {
                    self.eat_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var]);
                    self.parse_type_expr()?;
                }
                self.finish();
                if !self.eat(TokenKind::Comma) || self.at(TokenKind::RParen) {
                    break;
                }
            }
            self.finish();
        }
        self.expect(TokenKind::RParen)?;
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(false)?;
        }
        self.finish();
        Ok(())
    }

    fn parse_outcome_annotation(&mut self, allow_opaque: bool) -> ParseResult {
        self.start(SyntaxKind::OutcomeAnnotation)?;
        self.expect(TokenKind::Colon)?;
        if allow_opaque && self.at(TokenKind::Impl) {
            self.start(SyntaxKind::OpaqueOutcome)?;
            self.bump();
            self.parse_generic_bound()?;
            if self.eat(TokenKind::Bang) {
                self.parse_error_type_operand()?;
            }
            self.finish();
        } else {
            self.parse_type_expr()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_record_body(&mut self, allow_priv: bool) -> ParseResult {
        self.start(SyntaxKind::RecordBody)?;
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        if !allow_priv && self.at(TokenKind::RBrace) {
            self.syntax_error("an enum record variant requires at least one field")?;
        }
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            self.start(SyntaxKind::RecordField)?;
            if allow_priv && self.at(TokenKind::Priv) && self.nth(1) != TokenKind::Colon {
                self.bump();
            }
            self.expect_field_name()?;
            self.expect(TokenKind::Colon)?;
            self.parse_type_expr()?;
            self.finish();
            if !self.parse_field_separator()? {
                break;
            }
        }
        self.eat_newlines();
        self.expect(TokenKind::RBrace)?;
        self.finish();
        Ok(())
    }

    fn parse_tuple_payload(&mut self) -> ParseResult {
        self.start(SyntaxKind::TuplePayload)?;
        self.expect(TokenKind::LParen)?;
        self.parse_type_expr()?;
        while self.eat(TokenKind::Comma) {
            if self.at(TokenKind::RParen) {
                break;
            }
            self.parse_type_expr()?;
        }
        self.expect(TokenKind::RParen)?;
        self.finish();
        Ok(())
    }

    fn parse_parameter_list(&mut self) -> ParseResult {
        self.start(SyntaxKind::ParameterList)?;
        self.expect(TokenKind::LParen)?;
        if !self.at(TokenKind::RParen) {
            loop {
                self.start(SyntaxKind::Parameter)?;
                if self.at(TokenKind::SelfKw) {
                    self.bump();
                } else if self.at_any(&[TokenKind::Mut, TokenKind::Var])
                    && self.nth(1) == TokenKind::SelfKw
                {
                    self.bump();
                    self.bump();
                } else {
                    self.expect_identifier_or_discard()?;
                    self.expect(TokenKind::Colon)?;
                    if !self.eat(TokenKind::Ellipsis) {
                        self.eat_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var]);
                    }
                    self.parse_type_expr()?;
                }
                self.finish();
                if !self.eat(TokenKind::Comma) || self.at(TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen)?;
        self.finish();
        Ok(())
    }

    // Statements and expressions are implemented below this declaration layer.
    fn parse_statement(&mut self) -> ParseResult {
        match self.current() {
            TokenKind::Let | TokenKind::Var => self.parse_binding_decl(),
            TokenKind::Return => self.parse_return_stmt(),
            TokenKind::Fail => self.parse_fail_stmt(),
            TokenKind::Break => self.parse_simple_statement(SyntaxKind::BreakStmt),
            TokenKind::Continue => self.parse_simple_statement(SyntaxKind::ContinueStmt),
            TokenKind::Defer => self.parse_defer_stmt(),
            TokenKind::For => self.parse_for_stmt(),
            _ => self.parse_expression_or_assignment_statement(false),
        }
    }

    fn parse_binding_decl(&mut self) -> ParseResult {
        self.start(SyntaxKind::BindingDecl)?;
        self.bump();
        self.parse_pattern()?;
        if self.eat(TokenKind::Colon) {
            self.parse_type_expr()?;
        }
        if self.eat(TokenKind::Eq) {
            self.parse_expression()?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_return_stmt(&mut self) -> ParseResult {
        self.start(SyntaxKind::ReturnStmt)?;
        self.bump();
        if !self.at(TokenKind::Nl) {
            self.parse_expression()?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_fail_stmt(&mut self) -> ParseResult {
        self.start(SyntaxKind::FailStmt)?;
        self.bump();
        self.parse_expression()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_simple_statement(&mut self, kind: SyntaxKind) -> ParseResult {
        self.start(kind)?;
        self.bump();
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_defer_stmt(&mut self) -> ParseResult {
        self.start(SyntaxKind::DeferStmt)?;
        self.bump();
        if self.at(TokenKind::LBrace) {
            self.parse_block()?;
        } else {
            self.parse_expression()?;
        }
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_for_stmt(&mut self) -> ParseResult {
        if self.for_recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        if self.for_recursion_depth >= RECURSIVE_SPILL_DEPTH {
            return self.parse_for_stmt_spilled();
        }
        self.for_recursion_depth += 1;
        let result = self.parse_for_stmt_recursive();
        self.for_recursion_depth -= 1;
        result
    }

    fn parse_for_stmt_recursive(&mut self) -> ParseResult {
        self.start(SyntaxKind::ForStmt)?;
        self.expect(TokenKind::For)?;
        self.start(SyntaxKind::ForHeader)?;
        if !self.at(TokenKind::LBrace) {
            if self.header_has_top_level_in() {
                self.parse_pattern()?;
                self.expect(TokenKind::In)?;
                self.parse_header_expression()?;
            } else {
                self.parse_header_expression()?;
            }
        }
        self.finish();
        self.parse_block()?;
        self.expect_line_end()?;
        self.finish();
        Ok(())
    }

    fn parse_for_stmt_spilled(&mut self) -> ParseResult {
        let mut layers = 0_usize;
        loop {
            self.start(SyntaxKind::ForStmt)?;
            self.expect(TokenKind::For)?;
            self.start(SyntaxKind::ForHeader)?;
            if !self.at(TokenKind::LBrace) {
                if self.header_has_top_level_in() {
                    self.parse_pattern()?;
                    self.expect(TokenKind::In)?;
                    self.parse_header_expression()?;
                } else {
                    self.parse_header_expression()?;
                }
            }
            self.finish();
            self.start(SyntaxKind::Block)?;
            self.expect(TokenKind::LBrace)?;
            self.eat_newlines();
            layers += 1;
            if layers as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            if !self.at(TokenKind::For) {
                break;
            }
        }

        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            match self.current() {
                TokenKind::Let
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For => self.parse_statement()?,
                _ => self.parse_expression_or_assignment_statement(true)?,
            }
        }

        for _ in 0..layers {
            self.expect(TokenKind::RBrace)?;
            self.finish();
            self.expect_line_end()?;
            self.finish();
        }
        Ok(())
    }

    fn parse_expression_or_assignment_statement(&mut self, allow_tail: bool) -> ParseResult {
        if self.test_suite_decl_discriminator().is_some() {
            self.syntax_error("test and suite declarations are not allowed in this block")?;
            self.recover_to_statement_boundary()?;
            return Ok(());
        }
        if self.has_top_level_assignment_before_line_end() {
            self.start(SyntaxKind::Assignment)?;
            self.parse_assignment_pattern()?;
            if is_assignment_operator(self.current()) {
                self.bump();
            } else {
                self.expect(TokenKind::Eq)?;
            }
            self.parse_expression()?;
            self.expect_line_end()?;
            self.finish();
            return Ok(());
        }

        let checkpoint = self.checkpoint();
        self.parse_expression()?;
        if allow_tail && self.at_block_tail_boundary() {
            self.start_at(checkpoint, SyntaxKind::TailExpression)?;
            self.finish();
        } else {
            self.start_at(checkpoint, SyntaxKind::ExpressionStmt)?;
            self.expect_line_end()?;
            self.finish();
        }
        Ok(())
    }

    fn parse_assignment_pattern(&mut self) -> ParseResult {
        if self.assignment_recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        if self.assignment_recursion_depth >= RECURSIVE_SPILL_DEPTH
            || self.deep_assignment_tuple_chain_len() >= 4
        {
            return self.parse_assignment_pattern_spilled();
        }
        self.assignment_recursion_depth += 1;
        let result = self.parse_assignment_pattern_recursive();
        self.assignment_recursion_depth -= 1;
        result
    }

    fn parse_assignment_pattern_recursive(&mut self) -> ParseResult {
        if self.at(TokenKind::LParen) {
            self.start(SyntaxKind::TupleAssignmentPattern)?;
            self.bump();
            self.parse_assignment_pattern()?;
            self.expect(TokenKind::Comma)?;
            self.parse_assignment_pattern()?;
            while self.eat(TokenKind::Comma) {
                if self.at(TokenKind::RParen) {
                    break;
                }
                self.parse_assignment_pattern()?;
            }
            self.expect(TokenKind::RParen)?;
            self.finish();
            return Ok(());
        }
        if self.at_discard() {
            self.start(SyntaxKind::WildcardPattern)?;
            self.bump();
            self.finish();
            return Ok(());
        }
        self.start(SyntaxKind::Lvalue)?;
        if self.at_any(&[TokenKind::Identifier, TokenKind::SelfKw]) {
            self.bump();
        } else {
            self.expect(TokenKind::Identifier)?;
        }
        while self.at_any(&[TokenKind::Dot, TokenKind::LBracket]) {
            if self.eat(TokenKind::Dot) {
                if self.at(TokenKind::IntegerLiteral) {
                    self.bump();
                } else {
                    self.expect_field_name()?;
                }
            } else {
                self.parse_bracket_postfix()?;
            }
        }
        self.finish();
        Ok(())
    }

    fn parse_assignment_pattern_spilled(&mut self) -> ParseResult {
        let count = self.deep_assignment_tuple_chain_len();
        if count < 4 {
            self.assignment_recursion_depth += 1;
            let result = self.parse_assignment_pattern_recursive();
            self.assignment_recursion_depth -= 1;
            return result;
        }
        if count as u32 >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        for _ in 0..count {
            self.start(SyntaxKind::TupleAssignmentPattern)?;
            self.expect(TokenKind::LParen)?;
        }
        self.parse_assignment_leaf()?;
        for _ in 0..count {
            self.expect(TokenKind::Comma)?;
            self.parse_assignment_leaf()?;
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_assignment_leaf(&mut self) -> ParseResult {
        if self.at_discard() {
            self.start(SyntaxKind::WildcardPattern)?;
            self.bump();
            self.finish();
            return Ok(());
        }
        self.start(SyntaxKind::Lvalue)?;
        if self.at_any(&[TokenKind::Identifier, TokenKind::SelfKw]) {
            self.bump();
        } else {
            self.expect(TokenKind::Identifier)?;
        }
        while self.at_any(&[TokenKind::Dot, TokenKind::LBracket]) {
            if self.eat(TokenKind::Dot) {
                if self.at(TokenKind::IntegerLiteral) {
                    self.bump();
                } else {
                    self.expect_field_name()?;
                }
            } else {
                self.parse_bracket_postfix()?;
            }
        }
        self.finish();
        Ok(())
    }

    fn deep_assignment_tuple_chain_len(&self) -> usize {
        let mut offset = 0;
        let mut count = 0;
        while self.nth(offset) == TokenKind::LParen {
            count += 1;
            offset += 1;
        }
        count
    }

    fn parse_expression(&mut self) -> ParseResult {
        self.parse_expression_bp(0)
    }

    fn parse_header_expression(&mut self) -> ParseResult {
        self.header_expression_depth = self.header_expression_depth.saturating_add(1);
        let result = self.parse_expression();
        self.header_expression_depth -= 1;
        result
    }

    fn parse_expression_bp(&mut self, minimum_binding_power: u8) -> ParseResult {
        if self.recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        if self.spill_active || self.recursion_depth >= RECURSIVE_SPILL_DEPTH {
            return self.parse_expression_spilled(minimum_binding_power);
        }
        self.recursion_depth += 1;
        let result = self.parse_expression_bp_inner(minimum_binding_power);
        self.recursion_depth -= 1;
        result
    }

    /// Parse the Pratt continuation using an explicit heap-backed stack.
    ///
    /// The recursive path above is useful for the common shallow case because
    /// it keeps the grammar readable. It must never be allowed to grow with
    /// user input, though: once it reaches `RECURSIVE_SPILL_DEPTH`
    /// this method owns all prefix, postfix and infix continuations. Nested
    /// delimiters are consumed in batches, so a source containing millions of
    /// groups does not turn into millions of Rust call frames.
    fn parse_expression_spilled(&mut self, minimum_binding_power: u8) -> ParseResult {
        let was_spilling = self.spill_active;
        self.spill_active = true;
        let result = self.parse_expression_spilled_inner(minimum_binding_power);
        self.spill_active = was_spilling;
        result
    }

    fn parse_expression_spilled_inner(&mut self, minimum_binding_power: u8) -> ParseResult {
        let mut frames: Vec<SpilledBinaryFrame> = Vec::new();
        let mut checkpoint = self.checkpoint();
        let mut shape = self.parse_spilled_operand()?;
        let mut last_non_associative = None;
        let mut minimum_binding_power = minimum_binding_power;

        loop {
            if shape.postfix != PostfixPolicy::None && is_postfix_start(self.current()) {
                if shape.postfix == PostfixPolicy::AwaitBoundary && !self.at(TokenKind::Question) {
                    break;
                }
                self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
                let was_question = self.at(TokenKind::Question);
                self.parse_postfix_suffix()?;
                self.finish();
                if shape.postfix == PostfixPolicy::AwaitBoundary && was_question {
                    shape.postfix = PostfixPolicy::All;
                }
                continue;
            }

            let Some(operator) = binary_operator(self.current()) else {
                if let Some(frame) = frames.pop() {
                    self.finish();
                    checkpoint = frame.left_checkpoint;
                    minimum_binding_power = frame.minimum_binding_power;
                    last_non_associative = frame.last_non_associative;
                    shape = ExprShape::ordinary();
                    continue;
                }
                break;
            };
            if operator.left_binding_power < minimum_binding_power {
                if let Some(frame) = frames.pop() {
                    self.finish();
                    checkpoint = frame.left_checkpoint;
                    minimum_binding_power = frame.minimum_binding_power;
                    last_non_associative = frame.last_non_associative;
                    shape = ExprShape::ordinary();
                    continue;
                }
                break;
            }
            if let Some(family) = operator.non_associative_family {
                if last_non_associative == Some(family) {
                    self.invalid_operator_chain()?;
                }
                last_non_associative = Some(family);
            }

            self.start_at(checkpoint, SyntaxKind::BinaryExpr)?;
            let kind = self.current();
            self.bump();
            frames.push(SpilledBinaryFrame {
                left_checkpoint: checkpoint,
                minimum_binding_power,
                last_non_associative,
            });
            minimum_binding_power = operator.right_binding_power;
            checkpoint = self.checkpoint();
            if kind == TokenKind::With {
                self.parse_record_update_body()?;
                shape = ExprShape::ordinary();
            } else {
                shape = self.parse_spilled_operand()?;
            }
        }
        Ok(())
    }

    fn parse_spilled_operand(&mut self) -> ParseResult<ExprShape> {
        let mut prefix_count = 0_u32;
        while matches!(
            self.current(),
            TokenKind::Minus | TokenKind::Not | TokenKind::Tilde
        ) {
            self.start(SyntaxKind::PrefixExpr)?;
            self.bump();
            prefix_count += 1;
        }
        if prefix_count >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }

        let shape = match self.current() {
            TokenKind::Await => {
                self.start(SyntaxKind::AwaitExpr)?;
                self.bump();
                let checkpoint = self.checkpoint();
                self.parse_spilled_atom()?;
                while is_plain_postfix_start(self.current()) {
                    self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
                    self.parse_postfix_suffix()?;
                    self.finish();
                }
                self.finish();
                ExprShape {
                    postfix: PostfixPolicy::AwaitBoundary,
                    binary: true,
                }
            }
            TokenKind::Spawn => {
                self.start(SyntaxKind::SpawnExpr)?;
                self.bump();
                let checkpoint = self.checkpoint();
                self.parse_spilled_atom()?;
                while is_plain_postfix_start(self.current()) {
                    self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
                    self.parse_postfix_suffix()?;
                    self.finish();
                }
                self.finish();
                ExprShape::closed()
            }
            TokenKind::If => {
                self.parse_if_expression()?;
                ExprShape::closed()
            }
            TokenKind::Match => {
                self.parse_match_expression()?;
                ExprShape::closed()
            }
            TokenKind::Async => {
                self.parse_closure_expression()?;
                ExprShape::closed()
            }
            TokenKind::Unsafe if self.nth(1) == TokenKind::LParen => {
                self.parse_closure_expression()?;
                ExprShape::closed()
            }
            TokenKind::LParen if self.looks_like_closure() => {
                self.parse_closure_expression()?;
                ExprShape::closed()
            }
            _ => {
                let checkpoint = self.checkpoint();
                self.parse_spilled_atom()?;
                let mut shape = ExprShape::ordinary();
                while is_postfix_start(self.current()) {
                    self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
                    let was_question = self.at(TokenKind::Question);
                    self.parse_postfix_suffix()?;
                    self.finish();
                    if shape.postfix == PostfixPolicy::AwaitBoundary && was_question {
                        shape.postfix = PostfixPolicy::All;
                    }
                }
                shape
            }
        };

        for _ in 0..prefix_count {
            self.finish();
        }
        if prefix_count != 0 {
            Ok(ExprShape {
                postfix: PostfixPolicy::None,
                binary: true,
            })
        } else {
            Ok(shape)
        }
    }

    fn parse_spilled_atom(&mut self) -> ParseResult {
        if self.at(TokenKind::LBrace) {
            return self.parse_block_spilled();
        }
        if self.at(TokenKind::Scope) {
            self.start(SyntaxKind::ScopeExpr)?;
            self.bump();
            self.parse_block_spilled()?;
            self.finish();
            return Ok(());
        }
        if self.at(TokenKind::Unsafe) && self.nth(1) == TokenKind::LBrace {
            self.start(SyntaxKind::UnsafeExpr)?;
            self.bump();
            self.parse_block_spilled()?;
            self.finish();
            return Ok(());
        }
        if let Some(count) = self.deep_constructor_expression_chain_len() {
            if count as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            return self.parse_deep_constructor_expression(count);
        }
        if self.at(TokenKind::LParen)
            && let Some(tuple_layers) = self.parenthesized_chain_layers()
        {
            if tuple_layers.len() as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            for &is_tuple in &tuple_layers {
                self.start(if is_tuple {
                    SyntaxKind::TupleExpr
                } else {
                    SyntaxKind::GroupExpr
                })?;
                self.bump();
            }
            self.parse_expression_spilled_inner(0)?;
            for &is_tuple in tuple_layers.iter().rev() {
                if is_tuple {
                    self.expect(TokenKind::Comma)?;
                    if self.at(TokenKind::RParen) {
                        self.syntax_error("a tuple requires at least two items")?;
                    } else {
                        self.parse_expression_spilled_inner(0)?;
                        while self.eat(TokenKind::Comma) {
                            if self.at(TokenKind::RParen) {
                                break;
                            }
                            self.parse_expression_spilled_inner(0)?;
                        }
                    }
                }
                self.expect(TokenKind::RParen)?;
                self.finish();
            }
            return Ok(());
        }
        if self.at(TokenKind::LBracket) {
            let count = self.plain_bracket_literal_chain_len();
            if count > 1 {
                if count as u32 >= self.limits.max_nesting_depth {
                    return Err(ParseError::ResourceLimit {
                        resource: ParseResource::NestingDepth,
                        offset: self.current_offset(),
                    });
                }
                for _ in 0..count {
                    self.start(SyntaxKind::BracketLiteralExpr)?;
                    self.bump();
                }
                self.parse_expression_spilled_inner(0)?;
                for _ in 0..count {
                    self.expect(TokenKind::RBracket)?;
                    self.finish();
                }
                return Ok(());
            }
        }
        self.parse_primary_expression()
    }

    fn parse_block_spilled(&mut self) -> ParseResult {
        let mut layers = 0_usize;
        loop {
            self.start(SyntaxKind::Block)?;
            self.expect(TokenKind::LBrace)?;
            self.eat_newlines();
            layers += 1;
            if layers as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            if !self.at(TokenKind::LBrace) {
                break;
            }
        }

        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            match self.current() {
                TokenKind::Let
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For => self.parse_statement()?,
                _ => self.parse_expression_or_assignment_statement(true)?,
            }
        }

        for layer in 0..layers {
            if layer > 0 {
                self.eat_newlines();
            }
            self.expect(TokenKind::RBrace)?;
            self.finish();
        }
        Ok(())
    }

    fn deep_block_chain_len(&self) -> usize {
        let mut offset = 0;
        while self.nth(offset) == TokenKind::LBrace {
            offset += 1;
        }
        offset
    }

    fn deep_call_argument_chain_len(&self) -> Option<usize> {
        let mut offset = 0;
        let mut count = 0;
        loop {
            if self.nth(offset) != TokenKind::Identifier
                || self.nth(offset + 1) != TokenKind::LParen
            {
                break;
            }
            count += 1;
            offset += 2;
            if self.nth(offset) != TokenKind::Identifier {
                return None;
            }
        }
        if count < 4 {
            return None;
        }
        let mut close_offset = offset + 1;
        for _ in 0..count {
            if self.nth(close_offset) != TokenKind::RParen {
                return None;
            }
            close_offset += 1;
        }
        Some(count)
    }

    fn parse_deep_call_expression(&mut self) -> ParseResult {
        let mut frames = 0_usize;
        loop {
            let expression_checkpoint = self.checkpoint();
            self.parse_path_or_record_expression()?;
            if !self.at(TokenKind::LParen) {
                break;
            }
            self.start_at(expression_checkpoint, SyntaxKind::PostfixExpr)?;
            self.start(SyntaxKind::CallSuffix)?;
            self.expect(TokenKind::LParen)?;
            if self.at(TokenKind::RParen) {
                self.expect(TokenKind::RParen)?;
                self.finish();
                self.finish();
                break;
            }
            self.start(SyntaxKind::CallArgument)?;
            frames += 1;
        }

        for _ in 0..frames {
            self.expect(TokenKind::RParen)?;
            self.finish();
            self.finish();
            self.finish();
        }
        Ok(())
    }

    fn deep_record_expression_chain_len(&self) -> Option<usize> {
        let mut offset = 0;
        let mut count = 0;
        loop {
            if self.nth(offset) != TokenKind::Identifier
                || self.nth(offset + 1) != TokenKind::LBrace
                || self.nth(offset + 2) != TokenKind::Identifier
                || self.nth(offset + 3) != TokenKind::Colon
            {
                break;
            }
            count += 1;
            offset += 4;
            if self.nth(offset) != TokenKind::Identifier
                || self.nth(offset + 1) != TokenKind::LBrace
            {
                break;
            }
        }
        (count >= 4).then_some(count)
    }

    fn parse_deep_record_expression(&mut self, count: usize) -> ParseResult {
        let mut layers = 0_usize;
        loop {
            let checkpoint = self.checkpoint();
            self.start(SyntaxKind::PathExpr)?;
            self.expect_identifier()?;
            self.finish();
            self.start_at(checkpoint, SyntaxKind::RecordLikeExpr)?;
            self.expect(TokenKind::LBrace)?;
            self.eat_newlines();
            self.start(SyntaxKind::RecordInitializer)?;
            self.expect_field_name()?;
            self.expect(TokenKind::Colon)?;
            layers += 1;
            if layers >= count
                || self.nth(0) != TokenKind::Identifier
                || self.nth(1) != TokenKind::LBrace
            {
                break;
            }
        }

        self.parse_expression_spilled_inner(0)?;
        for layer in 0..layers {
            self.finish();
            if layer > 0 {
                self.eat_newlines();
            }
            self.expect(TokenKind::RBrace)?;
            self.finish();
        }
        Ok(())
    }

    fn deep_constructor_expression_chain_len(&self) -> Option<usize> {
        let mut offset = 0;
        let mut count = 0;
        while matches!(
            self.nth(offset),
            TokenKind::Some | TokenKind::Ok | TokenKind::Err
        ) && self.nth(offset + 1) == TokenKind::LParen
        {
            count += 1;
            offset += 2;
        }
        (count >= 4).then_some(count)
    }

    fn parse_deep_constructor_expression(&mut self, count: usize) -> ParseResult {
        for _ in 0..count {
            self.start(SyntaxKind::OptionResultConstructor)?;
            self.bump();
            self.expect(TokenKind::LParen)?;
        }
        self.parse_expression_spilled_inner(0)?;
        for _ in 0..count {
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parenthesized_chain_layers(&self) -> Option<Vec<bool>> {
        let mut offset = 0;
        let mut layers = Vec::new();
        while self.nth(offset) == TokenKind::LParen {
            layers.push(false);
            offset += 1;
        }
        if layers.is_empty() {
            return None;
        }
        let initial_layers = layers.len();
        let mut depth = initial_layers;
        let mut closing_initial_layers = false;
        while depth > 0 {
            let kind = self.nth(offset);
            if closing_initial_layers && !matches!(kind, TokenKind::RParen | TokenKind::Eof) {
                // The batch representation is only valid when the initial
                // parentheses are wrappers around one complete expression.
                // For `(a) + b` inside `((...))`, falling back to the ordinary
                // path preserves the operator boundary instead of treating
                // the first inner close as a missing delimiter.
                return None;
            }
            match kind {
                TokenKind::LParen => {
                    if closing_initial_layers {
                        return None;
                    }
                    depth += 1;
                }
                TokenKind::RParen => {
                    depth -= 1;
                    if depth < initial_layers {
                        closing_initial_layers = true;
                    }
                }
                TokenKind::Comma if !closing_initial_layers => {
                    if depth <= layers.len() {
                        layers[depth - 1] = true;
                    }
                }
                TokenKind::Eof => break,
                TokenKind::Nl => {
                    if closing_initial_layers {
                        return None;
                    }
                }
                _ => {}
            }
            if depth == 0 {
                break;
            }
            offset += 1;
            if offset > self.original_token_count {
                break;
            }
        }
        Some(layers)
    }

    fn plain_bracket_literal_chain_len(&self) -> usize {
        let mut offset = 0;
        let mut count = 0;
        while self.nth(offset) == TokenKind::LBracket {
            count += 1;
            offset += 1;
        }
        if count < 2 {
            return count;
        }
        let mut depth = count as i32;
        let mut saw_separator = false;
        while depth > 0 {
            match self.nth(offset) {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => depth -= 1,
                TokenKind::Comma | TokenKind::Colon if depth == count as i32 => {
                    saw_separator = true
                }
                TokenKind::Eof | TokenKind::Nl if depth > 0 => {
                    return if saw_separator { 0 } else { count };
                }
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return if saw_separator { 0 } else { count };
            }
        }
        if saw_separator { 0 } else { count }
    }

    fn parse_expression_bp_inner(&mut self, minimum_binding_power: u8) -> ParseResult {
        let checkpoint = self.checkpoint();
        let mut shape = self.parse_prefix_expression()?;
        let mut last_non_associative = None;

        loop {
            if shape.postfix != PostfixPolicy::None && is_postfix_start(self.current()) {
                if shape.postfix == PostfixPolicy::AwaitBoundary && !self.at(TokenKind::Question) {
                    break;
                }
                self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
                let was_question = self.at(TokenKind::Question);
                self.parse_postfix_suffix()?;
                self.finish();
                if shape.postfix == PostfixPolicy::AwaitBoundary && was_question {
                    shape.postfix = PostfixPolicy::All;
                }
                continue;
            }

            if !shape.binary {
                break;
            }
            let Some(operator) = binary_operator(self.current()) else {
                break;
            };
            if operator.left_binding_power < minimum_binding_power {
                break;
            }
            if let Some(family) = operator.non_associative_family {
                if last_non_associative == Some(family) {
                    self.invalid_operator_chain()?;
                }
                last_non_associative = Some(family);
            }

            self.start_at(checkpoint, SyntaxKind::BinaryExpr)?;
            let kind = self.current();
            self.bump();
            if kind == TokenKind::With {
                self.parse_record_update_body()?;
            } else {
                self.parse_expression_bp(operator.right_binding_power)?;
            }
            self.finish();
            shape = ExprShape::ordinary();
        }
        Ok(())
    }

    fn parse_prefix_expression(&mut self) -> ParseResult<ExprShape> {
        match self.current() {
            TokenKind::Minus | TokenKind::Not | TokenKind::Tilde => {
                self.start(SyntaxKind::PrefixExpr)?;
                self.bump();
                self.parse_expression_bp(PREFIX_BINDING_POWER)?;
                self.finish();
                Ok(ExprShape {
                    postfix: PostfixPolicy::None,
                    binary: true,
                })
            }
            TokenKind::Await => {
                self.start(SyntaxKind::AwaitExpr)?;
                self.bump();
                self.parse_plain_postfix_expression()?;
                self.finish();
                Ok(ExprShape {
                    postfix: PostfixPolicy::AwaitBoundary,
                    binary: true,
                })
            }
            TokenKind::Spawn => {
                self.start(SyntaxKind::SpawnExpr)?;
                self.bump();
                self.parse_plain_postfix_expression()?;
                self.finish();
                Ok(ExprShape::closed())
            }
            TokenKind::If => {
                self.parse_if_expression()?;
                Ok(ExprShape::closed())
            }
            TokenKind::Match => {
                self.parse_match_expression()?;
                Ok(ExprShape::closed())
            }
            TokenKind::Async => {
                self.parse_closure_expression()?;
                Ok(ExprShape::closed())
            }
            TokenKind::Unsafe if self.nth(1) == TokenKind::LParen => {
                self.parse_closure_expression()?;
                Ok(ExprShape::closed())
            }
            TokenKind::LParen if self.looks_like_closure() => {
                self.parse_closure_expression()?;
                Ok(ExprShape::closed())
            }
            _ => {
                self.parse_primary_expression()?;
                Ok(ExprShape::ordinary())
            }
        }
    }

    fn parse_plain_postfix_expression(&mut self) -> ParseResult {
        let checkpoint = self.checkpoint();
        self.parse_primary_expression()?;
        while is_plain_postfix_start(self.current()) {
            self.start_at(checkpoint, SyntaxKind::PostfixExpr)?;
            self.parse_postfix_suffix()?;
            self.finish();
        }
        Ok(())
    }

    fn parse_primary_expression(&mut self) -> ParseResult {
        match self.current() {
            TokenKind::IntegerLiteral
            | TokenKind::FloatLiteral
            | TokenKind::CharLiteral
            | TokenKind::RawStringLiteral
            | TokenKind::RawMultilineStringLiteral
            | TokenKind::True
            | TokenKind::False
            | TokenKind::None => {
                self.start(SyntaxKind::LiteralExpr)?;
                self.bump();
                self.finish();
            }
            TokenKind::StringStart | TokenKind::MultilineStringStart => {
                self.parse_string_literal_expression()?;
            }
            TokenKind::Identifier
                if self.at_intrinsic_set() && self.nth(1) == TokenKind::LBracket =>
            {
                self.parse_set_literal()?;
            }
            TokenKind::Identifier => {
                if let Some(count) = self.deep_call_argument_chain_len() {
                    if count as u32 >= self.limits.max_nesting_depth {
                        return Err(ParseError::ResourceLimit {
                            resource: ParseResource::NestingDepth,
                            offset: self.current_offset(),
                        });
                    }
                    self.parse_deep_call_expression()?;
                } else if let Some(count) = self.deep_record_expression_chain_len() {
                    if count as u32 >= self.limits.max_nesting_depth {
                        return Err(ParseError::ResourceLimit {
                            resource: ParseResource::NestingDepth,
                            offset: self.current_offset(),
                        });
                    }
                    self.parse_deep_record_expression(count)?;
                } else {
                    self.parse_path_or_record_expression()?;
                }
            }
            TokenKind::SelfKw => {
                self.start(SyntaxKind::SelfExpr)?;
                self.bump();
                self.finish();
            }
            TokenKind::LParen => {
                if self
                    .parenthesized_chain_layers()
                    .is_some_and(|layers| layers.len() >= 4)
                {
                    self.parse_spilled_atom()?;
                } else {
                    self.parse_tuple_or_group_expression()?;
                }
            }
            TokenKind::LBracket => {
                if self.plain_bracket_literal_chain_len() >= 4 {
                    self.parse_spilled_atom()?;
                } else {
                    self.parse_bracket_literal()?;
                }
            }
            TokenKind::LBrace => {
                if self.spill_active || self.deep_block_chain_len() >= 4 {
                    self.parse_block_spilled()?
                } else {
                    self.parse_block()?
                }
            }
            TokenKind::Scope => {
                self.start(SyntaxKind::ScopeExpr)?;
                self.bump();
                if self.spill_active || self.deep_block_chain_len() >= 4 {
                    self.parse_block_spilled()?;
                } else {
                    self.parse_block()?;
                }
                self.finish();
            }
            TokenKind::Unsafe if self.nth(1) == TokenKind::LBrace => {
                self.start(SyntaxKind::UnsafeExpr)?;
                self.bump();
                if self.spill_active || self.deep_block_chain_len() >= 4 {
                    self.parse_block_spilled()?;
                } else {
                    self.parse_block()?;
                }
                self.finish();
            }
            TokenKind::Some | TokenKind::Ok | TokenKind::Err
                if self.deep_constructor_expression_chain_len().is_some() =>
            {
                let count = self
                    .deep_constructor_expression_chain_len()
                    .expect("constructor chain was checked above");
                if count as u32 >= self.limits.max_nesting_depth {
                    return Err(ParseError::ResourceLimit {
                        resource: ParseResource::NestingDepth,
                        offset: self.current_offset(),
                    });
                }
                self.parse_deep_constructor_expression(count)?;
            }
            TokenKind::Some | TokenKind::Ok | TokenKind::Err => {
                self.start(SyntaxKind::OptionResultConstructor)?;
                self.bump();
                self.expect(TokenKind::LParen)?;
                self.parse_expression()?;
                self.expect(TokenKind::RParen)?;
                self.finish();
            }
            _ => {
                self.syntax_error("expected an expression")?;
                self.recover_one()?;
            }
        }
        Ok(())
    }

    fn parse_path_or_record_expression(&mut self) -> ParseResult {
        let checkpoint = self.checkpoint();
        self.start(SyntaxKind::PathExpr)?;
        self.expect_identifier()?;
        loop {
            while self.at(TokenKind::LBracket) {
                self.parse_bracket_postfix()?;
            }
            if self.at(TokenKind::Dot) && self.nth(1) == TokenKind::Identifier {
                self.bump();
                self.bump();
            } else {
                break;
            }
        }
        self.finish();
        if self.at(TokenKind::LBrace) && self.brace_belongs_to_record_expression() {
            self.start_at(checkpoint, SyntaxKind::RecordLikeExpr)?;
            self.parse_record_initializer_body()?;
            self.finish();
        }
        Ok(())
    }

    fn parse_tuple_or_group_expression(&mut self) -> ParseResult {
        let checkpoint = self.checkpoint();
        self.bump();
        if self.eat(TokenKind::RParen) {
            self.start_at(checkpoint, SyntaxKind::LiteralExpr)?;
            self.finish();
            return Ok(());
        }
        self.parse_expression()?;
        if self.eat(TokenKind::Comma) {
            self.start_at(checkpoint, SyntaxKind::TupleExpr)?;
            if self.at(TokenKind::RParen) {
                self.syntax_error("a tuple requires at least two items")?;
            } else {
                self.parse_expression()?;
                while self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    self.parse_expression()?;
                }
            }
            self.expect(TokenKind::RParen)?;
            self.finish();
        } else {
            self.start_at(checkpoint, SyntaxKind::GroupExpr)?;
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_postfix_suffix(&mut self) -> ParseResult {
        match self.current() {
            TokenKind::LParen => self.parse_call_suffix(),
            TokenKind::LBracket => self.parse_bracket_postfix(),
            TokenKind::Dot => {
                self.start(SyntaxKind::MemberSuffix)?;
                self.bump();
                if self.at(TokenKind::IntegerLiteral) {
                    self.bump();
                } else {
                    self.expect_field_name()?;
                }
                self.finish();
                Ok(())
            }
            TokenKind::Question => {
                self.start(SyntaxKind::PropagateSuffix)?;
                self.bump();
                self.finish();
                Ok(())
            }
            _ => {
                self.syntax_error("expected a postfix suffix")?;
                self.recover_one()
            }
        }
    }

    fn parse_call_suffix(&mut self) -> ParseResult {
        self.start(SyntaxKind::CallSuffix)?;
        self.expect(TokenKind::LParen)?;
        if !self.at(TokenKind::RParen) {
            loop {
                self.start(SyntaxKind::CallArgument)?;
                if self.at(TokenKind::Identifier) && self.nth(1) == TokenKind::Colon {
                    self.bump();
                    self.bump();
                }
                if self.eat(TokenKind::Ellipsis) {
                    self.parse_expression()?;
                } else {
                    self.eat_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var]);
                    self.parse_expression()?;
                }
                self.finish();
                if !self.eat(TokenKind::Comma) || self.at(TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen)?;
        self.finish();
        Ok(())
    }

    fn parse_bracket_postfix(&mut self) -> ParseResult {
        self.start(SyntaxKind::BracketPostfix)?;
        self.expect(TokenKind::LBracket)?;
        if self.at(TokenKind::Colon) {
            self.parse_slice_spec(None)?;
        } else if !self.at(TokenKind::RBracket) {
            self.parse_bracket_item()?;
            if self.at(TokenKind::Colon) {
                self.parse_slice_spec(Some(()))?;
            } else {
                while self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RBracket) {
                        break;
                    }
                    self.parse_bracket_item()?;
                }
            }
        } else {
            self.syntax_error("an index or generic argument list cannot be empty")?;
        }
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_bracket_item(&mut self) -> ParseResult {
        self.start(SyntaxKind::BracketItem)?;
        if self.bracket_item_requires_type_production() {
            self.parse_type_expr()?;
        } else {
            self.parse_expression()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_slice_spec(&mut self, _start_was_parsed: Option<()>) -> ParseResult {
        self.start(SyntaxKind::SliceSpec)?;
        self.expect(TokenKind::Colon)?;
        if !self.at_any(&[TokenKind::Colon, TokenKind::RBracket]) {
            self.parse_expression()?;
        }
        if self.eat(TokenKind::Colon) {
            if self.at(TokenKind::RBracket) {
                self.syntax_error("a second slice colon requires an explicit step")?;
            } else {
                self.parse_expression()?;
            }
        }
        self.finish();
        Ok(())
    }

    fn parse_string_literal_expression(&mut self) -> ParseResult {
        self.start(SyntaxKind::StringLiteralExpr)?;
        let end = if self.at(TokenKind::StringStart) {
            TokenKind::StringEnd
        } else {
            TokenKind::MultilineStringEnd
        };
        self.bump();
        while !self.at_any(&[end, TokenKind::Eof]) {
            if self.at(TokenKind::InterpolationStart) {
                self.start(SyntaxKind::Interpolation)?;
                self.bump();
                self.eat_newlines();
                if !self.at(TokenKind::InterpolationEnd) {
                    self.parse_expression()?;
                }
                self.eat_newlines();
                self.expect(TokenKind::InterpolationEnd)?;
                self.finish();
            } else {
                self.bump();
            }
        }
        self.expect(end)?;
        self.finish();
        Ok(())
    }

    fn parse_set_literal(&mut self) -> ParseResult {
        self.start(SyntaxKind::SetLiteralExpr)?;
        self.expect_identifier()?;
        self.expect(TokenKind::LBracket)?;
        if !self.at(TokenKind::RBracket) {
            self.parse_expression()?;
            while self.eat(TokenKind::Comma) {
                if self.at(TokenKind::RBracket) {
                    break;
                }
                self.parse_expression()?;
            }
        }
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_closure_expression(&mut self) -> ParseResult {
        self.start(SyntaxKind::ClosureExpr)?;
        self.parse_function_modifiers();
        self.start(SyntaxKind::ClosureParameterList)?;
        self.expect(TokenKind::LParen)?;
        if !self.at(TokenKind::RParen) {
            loop {
                self.start(SyntaxKind::ClosureParameter)?;
                self.expect_identifier_or_discard()?;
                if self.eat(TokenKind::Colon) {
                    if !self.eat(TokenKind::Ellipsis) {
                        self.eat_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var]);
                    }
                    self.parse_type_expr()?;
                }
                self.finish();
                if !self.eat(TokenKind::Comma) || self.at(TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen)?;
        self.finish();
        if self.at(TokenKind::Colon) {
            self.parse_outcome_annotation(false)?;
        }
        self.parse_block()?;
        self.finish();
        Ok(())
    }

    fn parse_record_initializer_body(&mut self) -> ParseResult {
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            self.start(SyntaxKind::RecordInitializer)?;
            self.expect_field_name()?;
            if self.eat(TokenKind::Colon) {
                self.parse_expression()?;
            }
            self.finish();
            if !self.parse_field_separator()? {
                break;
            }
        }
        self.eat_newlines();
        self.expect(TokenKind::RBrace)?;
        Ok(())
    }

    fn parse_record_update_body(&mut self) -> ParseResult {
        self.start(SyntaxKind::RecordUpdateBody)?;
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        let mut count = 0;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            count += 1;
            self.start(SyntaxKind::RecordUpdate)?;
            self.expect_field_name()?;
            self.expect(TokenKind::Colon)?;
            self.parse_expression()?;
            self.finish();
            if !self.parse_field_separator()? {
                break;
            }
        }
        if count == 0 {
            self.syntax_error("a record update requires at least one field")?;
        }
        self.eat_newlines();
        self.expect(TokenKind::RBrace)?;
        self.finish();
        Ok(())
    }

    fn parse_bracket_literal(&mut self) -> ParseResult {
        self.start(SyntaxKind::BracketLiteralExpr)?;
        self.bump();
        if self.eat(TokenKind::Colon) {
            self.expect(TokenKind::RBracket)?;
            self.finish();
            return Ok(());
        }
        if !self.at(TokenKind::RBracket) {
            self.parse_expression()?;
            let is_map = self.eat(TokenKind::Colon);
            if is_map {
                self.parse_expression()?;
            }
            while self.eat(TokenKind::Comma) {
                if self.at(TokenKind::RBracket) {
                    break;
                }
                self.parse_expression()?;
                if is_map {
                    if self.eat(TokenKind::Colon) {
                        self.parse_expression()?;
                    } else {
                        self.syntax_error("every map entry requires a value")?;
                    }
                } else if self.eat(TokenKind::Colon) {
                    self.syntax_error("array and map entries cannot be mixed")?;
                    self.parse_expression()?;
                }
            }
        }
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_if_expression(&mut self) -> ParseResult {
        if self.if_recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        // A nested `if` otherwise keeps every surrounding block on the host
        // stack. Spill at the first recursive continuation so the complete
        // chain is represented by the parser's heap-backed layer stack.
        if self.if_recursion_depth > 0 {
            return self.parse_if_expression_spilled();
        }
        self.if_recursion_depth += 1;
        let result = self.parse_if_expression_recursive();
        self.if_recursion_depth -= 1;
        result
    }

    fn parse_if_expression_recursive(&mut self) -> ParseResult {
        self.start(SyntaxKind::IfExpr)?;
        self.bump();
        self.parse_header_expression()?;
        self.parse_block()?;
        if self.eat(TokenKind::Else) {
            if self.at(TokenKind::If) {
                self.parse_if_expression()?;
            } else {
                self.parse_block()?;
            }
        }
        self.finish();
        Ok(())
    }

    fn parse_if_expression_spilled(&mut self) -> ParseResult {
        let mut layers = 0_usize;
        loop {
            self.start(SyntaxKind::IfExpr)?;
            self.expect(TokenKind::If)?;
            self.parse_header_expression()?;
            self.start(SyntaxKind::Block)?;
            self.expect(TokenKind::LBrace)?;
            self.eat_newlines();
            layers += 1;
            if layers as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            if !self.at(TokenKind::If) {
                break;
            }
        }

        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            match self.current() {
                TokenKind::Let
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For => self.parse_statement()?,
                _ => self.parse_expression_or_assignment_statement(true)?,
            }
        }

        for layer in 0..layers {
            if layer > 0 {
                self.eat_newlines();
            }
            self.expect(TokenKind::RBrace)?;
            self.finish();
            if self.eat(TokenKind::Else) {
                if self.at(TokenKind::If) {
                    self.parse_else_if_chain_spilled()?;
                } else {
                    self.parse_block_spilled()?;
                }
            }
            self.finish();
        }
        Ok(())
    }

    /// Parse an `else if` chain without putting one Rust frame per arm on the
    /// host stack. Each open `IfExpr` remains on the CST builder stack until
    /// the chain has been consumed, preserving the ordinary parent/child
    /// shape while the chain itself lives in a small counter.
    fn parse_else_if_chain_spilled(&mut self) -> ParseResult {
        let mut layers = 0_usize;
        loop {
            self.start(SyntaxKind::IfExpr)?;
            self.expect(TokenKind::If)?;
            self.parse_header_expression()?;
            self.parse_block_spilled()?;
            layers += 1;
            if layers as u32 >= self.limits.max_nesting_depth {
                return Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                });
            }
            if !self.eat(TokenKind::Else) {
                break;
            }
            if self.at(TokenKind::If) {
                continue;
            }
            if self.at(TokenKind::LBrace) {
                self.parse_block_spilled()?;
            } else {
                self.parse_block()?;
            }
            break;
        }
        for _ in 0..layers {
            self.finish();
        }
        Ok(())
    }

    fn parse_match_expression(&mut self) -> ParseResult {
        self.start(SyntaxKind::MatchExpr)?;
        self.bump();
        self.parse_header_expression()?;
        self.expect(TokenKind::LBrace)?;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            self.start(SyntaxKind::MatchArm)?;
            self.parse_pattern()?;
            if self.eat(TokenKind::If) {
                self.parse_expression()?;
            }
            self.expect(TokenKind::FatArrow)?;
            match self.current() {
                TokenKind::Return => self.parse_control_transfer(SyntaxKind::ReturnStmt, true)?,
                TokenKind::Fail => self.parse_control_transfer(SyntaxKind::FailStmt, false)?,
                TokenKind::Break => self.parse_control_transfer(SyntaxKind::BreakStmt, true)?,
                TokenKind::Continue => {
                    self.parse_control_transfer(SyntaxKind::ContinueStmt, true)?
                }
                _ => self.parse_expression()?,
            }
            if !self.eat(TokenKind::Comma) {
                self.expect_line_end()?;
            }
            self.finish();
        }
        self.expect(TokenKind::RBrace)?;
        self.finish();
        Ok(())
    }

    fn parse_control_transfer(
        &mut self,
        kind: SyntaxKind,
        expression_optional: bool,
    ) -> ParseResult {
        self.start(kind)?;
        self.bump();
        if (!expression_optional || !self.at_any(&[TokenKind::Nl, TokenKind::Comma]))
            && !matches!(kind, SyntaxKind::BreakStmt | SyntaxKind::ContinueStmt)
        {
            self.parse_expression()?;
        }
        self.finish();
        Ok(())
    }

    fn parse_block(&mut self) -> ParseResult {
        self.start(SyntaxKind::Block)?;
        self.expect(TokenKind::LBrace)?;
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.eat(TokenKind::Nl) {
                continue;
            }
            match self.current() {
                TokenKind::Let
                | TokenKind::Var
                | TokenKind::Return
                | TokenKind::Fail
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Defer
                | TokenKind::For => self.parse_statement()?,
                _ => self.parse_expression_or_assignment_statement(true)?,
            }
        }
        self.expect(TokenKind::RBrace)?;
        self.finish();
        Ok(())
    }

    fn parse_pattern(&mut self) -> ParseResult {
        if self.pattern_recursion_depth >= self.limits.max_nesting_depth {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        if self.pattern_spill_active
            || self.pattern_recursion_depth >= RECURSIVE_SPILL_DEPTH
            || self.deep_pattern_chain_len() >= 4
        {
            return self.parse_pattern_spilled();
        }
        self.pattern_recursion_depth += 1;
        let result = self.parse_pattern_recursive();
        self.pattern_recursion_depth -= 1;
        result
    }

    fn parse_pattern_recursive(&mut self) -> ParseResult {
        match self.current() {
            TokenKind::Identifier if self.at_discard() => {
                self.start(SyntaxKind::WildcardPattern)?;
                self.bump();
                self.finish();
            }
            TokenKind::Identifier => self.parse_named_pattern()?,
            TokenKind::Ref | TokenKind::Mut | TokenKind::Var => {
                self.start(SyntaxKind::BorrowBindingPattern)?;
                self.bump();
                self.expect_identifier()?;
                self.finish();
            }
            TokenKind::LParen => self.parse_tuple_or_unit_pattern()?,
            TokenKind::LBracket => self.parse_array_pattern()?,
            TokenKind::Minus
                if matches!(
                    self.nth(1),
                    TokenKind::IntegerLiteral | TokenKind::FloatLiteral
                ) =>
            {
                self.start(SyntaxKind::LiteralPattern)?;
                self.bump();
                self.bump();
                self.finish();
            }
            TokenKind::IntegerLiteral
            | TokenKind::FloatLiteral
            | TokenKind::CharLiteral
            | TokenKind::RawStringLiteral
            | TokenKind::RawMultilineStringLiteral
            | TokenKind::StringStart
            | TokenKind::MultilineStringStart
            | TokenKind::True
            | TokenKind::False => {
                self.start(SyntaxKind::LiteralPattern)?;
                if self.at_any(&[TokenKind::StringStart, TokenKind::MultilineStringStart]) {
                    self.parse_string_literal_expression()?;
                } else {
                    self.bump();
                }
                self.finish();
            }
            TokenKind::Some | TokenKind::Ok | TokenKind::Err => {
                self.start(SyntaxKind::OptionResultPattern)?;
                self.bump();
                self.expect(TokenKind::LParen)?;
                self.parse_pattern()?;
                self.expect(TokenKind::RParen)?;
                self.finish();
            }
            TokenKind::None => {
                self.start(SyntaxKind::OptionResultPattern)?;
                self.bump();
                self.finish();
            }
            _ => {
                self.syntax_error("expected a pattern")?;
                self.recover_one()?;
            }
        }
        Ok(())
    }

    fn parse_pattern_spilled(&mut self) -> ParseResult {
        let was_spilling = self.pattern_spill_active;
        self.pattern_spill_active = true;
        let result = if let Some(tuple_layers) = self.parenthesized_pattern_layers() {
            if tuple_layers.len() as u32 >= self.limits.max_nesting_depth {
                Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                })
            } else {
                self.parse_deep_tuple_pattern(tuple_layers)
            }
        } else if let Some(count) = self.deep_array_pattern_chain_len() {
            if count as u32 >= self.limits.max_nesting_depth {
                Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                })
            } else {
                self.parse_deep_array_pattern(count)
            }
        } else if let Some(count) = self.deep_constructor_pattern_chain_len() {
            if count as u32 >= self.limits.max_nesting_depth {
                Err(ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    offset: self.current_offset(),
                })
            } else {
                self.parse_deep_constructor_pattern(count)
            }
        } else {
            self.pattern_spill_active = false;
            let result = self.parse_pattern_recursive();
            self.pattern_spill_active = true;
            result
        };
        self.pattern_spill_active = was_spilling;
        result
    }

    fn parse_deep_tuple_pattern(&mut self, tuple_layers: Vec<bool>) -> ParseResult {
        for _ in &tuple_layers {
            self.start(SyntaxKind::TuplePattern)?;
            self.bump();
        }
        if self.at(TokenKind::RParen) {
            self.syntax_error("a tuple requires at least one item")?;
        } else {
            self.parse_pattern_spilled_leaf()?;
        }
        for _ in tuple_layers.iter().rev() {
            self.expect(TokenKind::Comma)?;
            if self.at(TokenKind::RParen) {
                self.syntax_error("a tuple requires at least two items")?;
            } else {
                self.parse_pattern_spilled_leaf()?;
                while self.eat(TokenKind::Comma) {
                    if self.at(TokenKind::RParen) {
                        break;
                    }
                    self.parse_pattern_spilled_leaf()?;
                }
            }
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_deep_array_pattern(&mut self, count: usize) -> ParseResult {
        for _ in 0..count {
            self.start(SyntaxKind::ArrayPattern)?;
            self.bump();
        }
        self.parse_pattern_spilled_leaf()?;
        for _ in 0..count {
            self.expect(TokenKind::RBracket)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_deep_constructor_pattern(&mut self, count: usize) -> ParseResult {
        for _ in 0..count {
            self.start(SyntaxKind::ConstructorPattern)?;
            self.expect_identifier()?;
            self.expect(TokenKind::LParen)?;
        }
        self.parse_pattern_spilled_leaf()?;
        for _ in 0..count {
            self.expect(TokenKind::RParen)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_pattern_spilled_leaf(&mut self) -> ParseResult {
        self.pattern_spill_active = false;
        let result = self.parse_pattern();
        self.pattern_spill_active = true;
        result
    }

    fn deep_pattern_chain_len(&self) -> usize {
        self.parenthesized_pattern_layers()
            .map(|layers| layers.len())
            .or_else(|| self.deep_array_pattern_chain_len())
            .or_else(|| self.deep_constructor_pattern_chain_len())
            .unwrap_or(0)
    }

    fn deep_array_pattern_chain_len(&self) -> Option<usize> {
        let mut offset = 0;
        let mut count = 0;
        while self.nth(offset) == TokenKind::LBracket {
            count += 1;
            offset += 1;
        }
        (count >= 4).then_some(count)
    }

    fn deep_constructor_pattern_chain_len(&self) -> Option<usize> {
        let mut offset = 0;
        let mut count = 0;
        while self.nth(offset) == TokenKind::Identifier && self.nth(offset + 1) == TokenKind::LParen
        {
            count += 1;
            offset += 2;
        }
        (count >= 4).then_some(count)
    }

    fn parenthesized_pattern_layers(&self) -> Option<Vec<bool>> {
        let mut offset = 0;
        let mut layers = Vec::new();
        while self.nth(offset) == TokenKind::LParen {
            layers.push(false);
            offset += 1;
        }
        if layers.len() < 4 {
            return None;
        }
        let mut depth = layers.len();
        while depth > 0 {
            match self.nth(offset) {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                TokenKind::Comma if depth <= layers.len() => {
                    layers[depth - 1] = true;
                }
                TokenKind::Eof => break,
                TokenKind::Nl => {}
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                break;
            }
        }
        Some(layers)
    }

    fn parse_named_pattern(&mut self) -> ParseResult {
        let checkpoint = self.checkpoint();
        let mut qualified = false;
        self.start(SyntaxKind::BindingPattern)?;
        self.bump();
        loop {
            while self.at(TokenKind::LBracket) {
                qualified = true;
                self.parse_bracket_postfix()?;
            }
            if self.at(TokenKind::Dot) && self.nth(1) == TokenKind::Identifier {
                qualified = true;
                self.bump();
                self.bump();
            } else {
                break;
            }
        }
        self.finish();
        if self.at(TokenKind::LParen) {
            self.start_at(checkpoint, SyntaxKind::ConstructorPattern)?;
            self.bump();
            self.parse_pattern()?;
            while self.eat(TokenKind::Comma) {
                if self.at(TokenKind::RParen) {
                    break;
                }
                self.parse_pattern()?;
            }
            self.expect(TokenKind::RParen)?;
            self.finish();
        } else if self.at(TokenKind::LBrace) {
            self.start_at(checkpoint, SyntaxKind::RecordPattern)?;
            self.parse_record_pattern_body()?;
            self.finish();
        } else if qualified {
            self.start_at(checkpoint, SyntaxKind::QualifiedValuePattern)?;
            self.finish();
        }
        Ok(())
    }

    fn parse_tuple_or_unit_pattern(&mut self) -> ParseResult {
        let checkpoint = self.checkpoint();
        self.bump();
        if self.eat(TokenKind::RParen) {
            self.start_at(checkpoint, SyntaxKind::UnitPattern)?;
            self.finish();
            return Ok(());
        }
        self.parse_pattern()?;
        self.expect(TokenKind::Comma)?;
        self.start_at(checkpoint, SyntaxKind::TuplePattern)?;
        self.parse_pattern()?;
        while self.eat(TokenKind::Comma) {
            if self.at(TokenKind::RParen) {
                break;
            }
            self.parse_pattern()?;
        }
        self.expect(TokenKind::RParen)?;
        self.finish();
        Ok(())
    }

    fn parse_array_pattern(&mut self) -> ParseResult {
        self.start(SyntaxKind::ArrayPattern)?;
        self.bump();
        let mut fixed_count = 0;
        while !self.at_any(&[TokenKind::RBracket, TokenKind::Eof]) {
            if self.at(TokenKind::DotDot) {
                self.start(SyntaxKind::ArrayRestPattern)?;
                self.bump();
                if self.eat_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var]) {
                    self.expect_identifier()?;
                } else if self.at(TokenKind::Identifier) {
                    self.bump();
                }
                self.finish();
                if fixed_count == 0 {
                    self.syntax_error("an array rest pattern requires a fixed prefix")?;
                }
                break;
            }
            self.parse_pattern()?;
            fixed_count += 1;
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        self.eat(TokenKind::Comma);
        self.expect(TokenKind::RBracket)?;
        self.finish();
        Ok(())
    }

    fn parse_record_pattern_body(&mut self) -> ParseResult {
        self.expect(TokenKind::LBrace)?;
        self.eat_newlines();
        while !self.at_any(&[TokenKind::RBrace, TokenKind::Eof]) {
            if self.at(TokenKind::DotDot) {
                self.start(SyntaxKind::RecordRestPattern)?;
                self.bump();
                self.finish();
                self.eat(TokenKind::Comma);
                break;
            }
            self.start(SyntaxKind::RecordPatternField)?;
            if self.at_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var])
                && self.nth(1) != TokenKind::Colon
            {
                self.bump();
                self.expect_identifier()?;
            } else {
                self.expect_field_name()?;
                if self.eat(TokenKind::Colon) {
                    self.parse_pattern()?;
                }
            }
            self.finish();
            if !self.parse_field_separator()? {
                break;
            }
        }
        self.eat_newlines();
        self.expect(TokenKind::RBrace)?;
        Ok(())
    }

    fn parse_field_separator(&mut self) -> ParseResult<bool> {
        if self.eat(TokenKind::Comma) || self.eat(TokenKind::Nl) {
            self.eat_newlines();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn header_has_top_level_in(&self) -> bool {
        self.find_top_level_before_boundary(|kind| kind == TokenKind::In)
    }

    fn has_top_level_assignment_before_line_end(&self) -> bool {
        self.find_top_level_before_boundary(is_assignment_operator)
    }

    fn find_top_level_before_boundary(&self, predicate: impl Fn(TokenKind) -> bool) -> bool {
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        let mut offset = 0;
        loop {
            let kind = self.nth(offset);
            if kind == TokenKind::Eof {
                return false;
            }
            if parentheses == 0 && brackets == 0 {
                if matches!(kind, TokenKind::Nl | TokenKind::RBrace | TokenKind::LBrace) {
                    return false;
                }
                if predicate(kind) {
                    return true;
                }
            }
            match kind {
                TokenKind::LParen => parentheses = parentheses.saturating_add(1),
                TokenKind::RParen => parentheses = parentheses.saturating_sub(1),
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket => brackets = brackets.saturating_sub(1),
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn looks_like_closure(&self) -> bool {
        if !self.at(TokenKind::LParen) {
            return false;
        }
        let mut depth = 0_u32;
        let mut offset = 0;
        let after_parameters = loop {
            let kind = self.nth(offset);
            match kind {
                TokenKind::LParen => depth = depth.saturating_add(1),
                TokenKind::RParen => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break offset + 1;
                    }
                }
                TokenKind::Eof | TokenKind::Nl => return false,
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        };

        if self.nth(after_parameters) == TokenKind::LBrace {
            return true;
        }
        if self.nth(after_parameters) != TokenKind::Colon {
            return false;
        }
        let mut offset = after_parameters + 1;
        let mut delimiters = 0_i32;
        loop {
            match self.nth(offset) {
                TokenKind::LParen | TokenKind::LBracket => delimiters += 1,
                TokenKind::RParen | TokenKind::RBracket => delimiters -= 1,
                TokenKind::LBrace if delimiters == 0 => return true,
                TokenKind::Nl | TokenKind::Eof if delimiters == 0 => return false,
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn at_discard(&self) -> bool {
        self.at(TokenKind::Identifier) && self.current_token().normalized_identifier() == Some("_")
    }

    fn at_intrinsic_set(&self) -> bool {
        self.at(TokenKind::Identifier)
            && self.current_token().normalized_identifier() == Some("Set")
    }

    fn at_block_tail_boundary(&self) -> bool {
        if self.at(TokenKind::RBrace) {
            return true;
        }
        if !self.at(TokenKind::Nl) {
            return false;
        }
        let mut offset = 0;
        while self.nth(offset) == TokenKind::Nl {
            offset += 1;
        }
        self.nth(offset) == TokenKind::RBrace
    }

    fn brace_belongs_to_record_expression(&self) -> bool {
        if self.header_expression_depth == 0 || !self.at(TokenKind::LBrace) {
            return true;
        }
        let mut depth = 0_u32;
        let mut offset = 0;
        loop {
            match self.nth(offset) {
                TokenKind::LBrace => depth = depth.saturating_add(1),
                TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        let next = self.nth(offset + 1);
                        return next == TokenKind::LBrace
                            || is_postfix_start(next)
                            || binary_operator(next).is_some();
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn invalid_operator_chain(&mut self) -> ParseResult {
        self.push_diagnostic(
            "E0005",
            "non-associative operators cannot be chained without parentheses",
            None,
        )
    }

    fn at_top_decl_start(&self) -> bool {
        self.top_decl_discriminator().is_some()
    }

    fn suite_member_discriminator(&self) -> Option<TokenKind> {
        self.test_suite_decl_discriminator()
    }

    fn test_suite_decl_discriminator(&self) -> Option<TokenKind> {
        if !matches!(self.mode, ParseMode::Module | ParseMode::ImportedModule) {
            return None;
        }
        let mut offset = 0;
        while matches!(
            self.nth(offset),
            TokenKind::Pub | TokenKind::Priv | TokenKind::Async | TokenKind::Unsafe
        ) {
            offset += 1;
        }
        let kind = self.nth(offset);
        matches!(kind, TokenKind::Test | TokenKind::Suite).then_some(kind)
    }

    fn at_function_signature_start(&self) -> bool {
        let mut offset = usize::from(self.nth(0) == TokenKind::Pub);
        if self.nth(offset) == TokenKind::Async {
            offset += 1;
            if self.nth(offset) == TokenKind::Unsafe {
                offset += 1;
            }
        } else if self.nth(offset) == TokenKind::Unsafe {
            offset += 1;
        }
        self.nth(offset) == TokenKind::Fn && self.nth(offset + 1) == TokenKind::Identifier
    }

    fn function_item_has_body(&self) -> bool {
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        let mut offset = 0;
        loop {
            match self.nth(offset) {
                TokenKind::LParen => parentheses = parentheses.saturating_add(1),
                TokenKind::RParen => parentheses = parentheses.saturating_sub(1),
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket => brackets = brackets.saturating_sub(1),
                TokenKind::LBrace if parentheses == 0 && brackets == 0 => return true,
                TokenKind::Nl | TokenKind::Eof if parentheses == 0 && brackets == 0 => {
                    return false;
                }
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn line_requires_type_production(&self) -> bool {
        if self.at(TokenKind::Bang)
            || (self.at_function_type_start() && !self.at_function_signature_start())
        {
            return true;
        }
        matches!(self.current(), TokenKind::Identifier | TokenKind::LParen)
            && (self.line_has_top_level_token(TokenKind::Bang)
                || self.line_contains_token(TokenKind::Fn))
    }

    fn line_requires_pattern_production(&self) -> bool {
        (self.at_any(&[TokenKind::Ref, TokenKind::Mut, TokenKind::Var])
            && self.nth(1) == TokenKind::Identifier
            && matches!(self.nth(2), TokenKind::Nl | TokenKind::Eof))
            || self.line_has_pattern_rest()
    }

    fn line_has_top_level_token(&self, target: TokenKind) -> bool {
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        let mut offset = 0;
        loop {
            let kind = self.nth(offset);
            if matches!(kind, TokenKind::Nl | TokenKind::Eof) && parentheses == 0 && brackets == 0 {
                return false;
            }
            if kind == target && parentheses == 0 && brackets == 0 {
                return true;
            }
            match kind {
                TokenKind::LParen => parentheses = parentheses.saturating_add(1),
                TokenKind::RParen => parentheses = parentheses.saturating_sub(1),
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket => brackets = brackets.saturating_sub(1),
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn line_has_pattern_rest(&self) -> bool {
        let mut offset = 0;
        let mut brackets = 0_u32;
        let mut braces = 0_u32;
        loop {
            let kind = self.nth(offset);
            if matches!(kind, TokenKind::Nl | TokenKind::Eof) {
                return false;
            }
            if kind == TokenKind::DotDot && (brackets > 0 || braces > 0) {
                return true;
            }
            match kind {
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket => brackets = brackets.saturating_sub(1),
                TokenKind::LBrace => braces = braces.saturating_add(1),
                TokenKind::RBrace => braces = braces.saturating_sub(1),
                _ => {}
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn line_contains_token(&self, target: TokenKind) -> bool {
        let mut offset = 0;
        loop {
            let kind = self.nth(offset);
            if matches!(kind, TokenKind::Nl | TokenKind::Eof) {
                return false;
            }
            if kind == target {
                return true;
            }
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn bracket_item_requires_type_production(&self) -> bool {
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        let mut braces = 0_u32;
        let mut previous = None;
        let mut offset = 0;
        loop {
            let kind = self.nth(offset);
            if parentheses == 0 && brackets == 0 && braces == 0 {
                if matches!(
                    kind,
                    TokenKind::Comma | TokenKind::Colon | TokenKind::RBracket
                ) {
                    return false;
                }
                if matches!(kind, TokenKind::Nl | TokenKind::Eof) {
                    return false;
                }
            }
            if kind == TokenKind::Bang
                || (kind == TokenKind::Fn && braces == 0 && previous != Some(TokenKind::Dot))
            {
                return true;
            }
            match kind {
                TokenKind::LParen => parentheses = parentheses.saturating_add(1),
                TokenKind::RParen => parentheses = parentheses.saturating_sub(1),
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket => {
                    if brackets == 0 {
                        return false;
                    }
                    brackets -= 1;
                }
                TokenKind::LBrace => braces = braces.saturating_add(1),
                TokenKind::RBrace => braces = braces.saturating_sub(1),
                _ => {}
            }
            previous = Some(kind);
            offset += 1;
            if offset > self.original_token_count {
                return false;
            }
        }
    }

    fn top_decl_discriminator(&self) -> Option<TokenKind> {
        if let Some(kind) = self.test_suite_decl_discriminator() {
            return Some(kind);
        }
        let mut offset = 0;
        if self.nth(offset) == TokenKind::Pub {
            offset += 1;
        }
        if self.nth(offset) == TokenKind::Async {
            return Some(TokenKind::Async);
        }
        if self.nth(offset) == TokenKind::Unsafe && self.nth(offset + 1) == TokenKind::Fn {
            return Some(TokenKind::Unsafe);
        }
        let kind = self.nth(offset);
        matches!(
            kind,
            TokenKind::Const
                | TokenKind::Type
                | TokenKind::Alias
                | TokenKind::Enum
                | TokenKind::Trait
                | TokenKind::Impl
                | TokenKind::Fn
        )
        .then_some(kind)
    }

    fn at_function_type_start(&self) -> bool {
        self.at(TokenKind::Fn)
            || (self.at_any(&[TokenKind::Async, TokenKind::Unsafe])
                && matches!(self.nth(1), TokenKind::Fn | TokenKind::Unsafe))
    }

    fn at_method_start(&self) -> bool {
        self.at_any(&[TokenKind::Fn, TokenKind::Async, TokenKind::Unsafe])
    }

    fn at_recovery_construct_start(&self) -> bool {
        self.at(TokenKind::Import)
            || self.at_top_decl_start()
            || self.at_any(&[
                TokenKind::Let,
                TokenKind::Var,
                TokenKind::Return,
                TokenKind::Fail,
                TokenKind::Break,
                TokenKind::Continue,
                TokenKind::Defer,
                TokenKind::For,
            ])
    }

    fn has_physical_newline_before_current(&self) -> bool {
        let mut index = self.cursor;
        let mut found = false;
        while index < self.original_token_count {
            let kind = self.builder.original_token(index).kind();
            if !kind.is_trivia() {
                return found;
            }
            found |= kind == TokenKind::PhysicalNewline;
            index += 1;
        }
        found
    }

    fn expect_identifier(&mut self) -> ParseResult {
        self.expect(TokenKind::Identifier)
    }

    fn expect_identifier_or_discard(&mut self) -> ParseResult {
        self.expect(TokenKind::Identifier)
    }

    fn expect_field_name(&mut self) -> ParseResult {
        if self.current() == TokenKind::Identifier || self.current().is_keyword() {
            self.bump();
            Ok(())
        } else {
            self.expect(TokenKind::Identifier)
        }
    }

    fn expect_line_end(&mut self) -> ParseResult {
        if self.eat(TokenKind::Nl) {
            return Ok(());
        }
        let recovered_physical_boundary = self.has_physical_newline_before_current();
        self.expect(TokenKind::Nl)?;
        if recovered_physical_boundary {
            self.suppress_syntax_errors = false;
            self.logical_newlines_consumed = self.logical_newlines_consumed.saturating_add(1);
        }
        Ok(())
    }

    fn eat_newlines(&mut self) {
        while self.eat(TokenKind::Nl) {}
    }

    fn expect(&mut self, kind: TokenKind) -> ParseResult {
        if self.at(kind) {
            self.bump();
            return Ok(());
        }
        let offset = self.current_offset();
        self.syntax_error_expected(kind)?;
        self.builder.missing_token(kind, offset);
        Ok(())
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_any(&mut self, kinds: &[TokenKind]) -> bool {
        if self.at_any(kinds) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.current() == kind
    }

    fn at_any(&self, kinds: &[TokenKind]) -> bool {
        kinds.contains(&self.current())
    }

    fn current(&self) -> TokenKind {
        self.nth(0)
    }

    fn nth(&self, significant_offset: usize) -> TokenKind {
        self.significant_indices
            .get(self.significant_cursor + significant_offset)
            .map(|&index| self.builder.original_token(index).kind())
            .unwrap_or(TokenKind::Eof)
    }

    fn current_offset(&self) -> u32 {
        if let Some(&index) = self.significant_indices.get(self.significant_cursor) {
            return self.builder.original_token(index).range().start();
        }
        self.builder
            .original_token(self.original_token_count - 1)
            .range()
            .end()
    }

    fn bump(&mut self) {
        while self.cursor < self.original_token_count {
            let id = TokenId::from_original_index(self.cursor);
            let kind = self.builder.original_token(self.cursor).kind();
            self.builder.token(id);
            self.cursor += 1;
            if !kind.is_trivia() {
                if kind == TokenKind::Nl {
                    self.suppress_syntax_errors = false;
                    self.logical_newlines_consumed =
                        self.logical_newlines_consumed.saturating_add(1);
                }
                self.significant_cursor += 1;
                break;
            }
        }
    }

    fn recover_to_member_boundary(&mut self) -> ParseResult {
        self.start(SyntaxKind::Error)?;
        while !self.at_any(&[TokenKind::Nl, TokenKind::RBrace, TokenKind::Eof]) {
            self.bump();
        }
        self.finish();
        self.eat(TokenKind::Nl);
        Ok(())
    }

    fn recover_to_suite_boundary(&mut self) -> ParseResult {
        self.start(SyntaxKind::Error)?;
        let mut braces = 0_u32;
        let mut parentheses = 0_u32;
        let mut brackets = 0_u32;
        while !self.at(TokenKind::Eof) {
            if braces == 0
                && parentheses == 0
                && brackets == 0
                && (self.at_any(&[TokenKind::Nl, TokenKind::RBrace])
                    || self.suite_member_discriminator().is_some())
            {
                break;
            }
            match self.current() {
                TokenKind::LBrace => braces = braces.saturating_add(1),
                TokenKind::RBrace if braces > 0 => braces -= 1,
                TokenKind::LParen => parentheses = parentheses.saturating_add(1),
                TokenKind::RParen if parentheses > 0 => parentheses -= 1,
                TokenKind::LBracket => brackets = brackets.saturating_add(1),
                TokenKind::RBracket if brackets > 0 => brackets -= 1,
                _ => {}
            }
            self.bump();
        }
        self.finish();
        self.eat(TokenKind::Nl);
        Ok(())
    }

    fn recover_to_statement_boundary(&mut self) -> ParseResult {
        if self.at(TokenKind::Eof) {
            return Ok(());
        }
        self.start(SyntaxKind::Error)?;
        while !self.at_any(&[TokenKind::Nl, TokenKind::Eof]) {
            self.bump();
        }
        self.finish();
        self.eat(TokenKind::Nl);
        Ok(())
    }

    fn recover_one(&mut self) -> ParseResult {
        self.start(SyntaxKind::Error)?;
        let begins_recovered_line =
            self.has_physical_newline_before_current() && self.at_recovery_construct_start();
        if !begins_recovered_line
            && !self.at_any(&[
                TokenKind::Nl,
                TokenKind::Eof,
                TokenKind::Comma,
                TokenKind::RParen,
                TokenKind::RBracket,
                TokenKind::RBrace,
                TokenKind::FatArrow,
            ])
        {
            self.bump();
        }
        self.finish();
        Ok(())
    }

    fn syntax_error(&mut self, message: &str) -> ParseResult {
        self.push_diagnostic("E0004", message, None)
    }

    fn syntax_error_expected(&mut self, expected: TokenKind) -> ParseResult {
        self.push_diagnostic(
            "E0004",
            "tokens do not form the required syntax",
            Some(expected),
        )
    }

    fn push_diagnostic(
        &mut self,
        code: &str,
        message: &str,
        expected: Option<TokenKind>,
    ) -> ParseResult {
        self.push_diagnostic_at(
            code,
            message,
            expected,
            self.current_token().range(),
            self.current(),
        )
    }

    fn push_diagnostic_at(
        &mut self,
        code: &str,
        message: &str,
        expected: Option<TokenKind>,
        range: crate::source::TextRange,
        actual: TokenKind,
    ) -> ParseResult {
        let offset = range.start();
        if code == "E0004" && self.suppress_syntax_errors {
            return Ok(());
        }
        if self.diagnostics.len() >= self.limits.max_diagnostics as usize {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::Diagnostics,
                offset,
            });
        }
        let span = self.sources.span(self.file, range)?;
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(span),
        )?
        .with_expected_actual(
            expected.map(|kind| format!("{kind:?}")),
            Some(format!("{actual:?}")),
        );
        self.diagnostics.push(diagnostic);
        if code == "E0004" {
            self.suppress_syntax_errors = true;
        }
        Ok(())
    }

    fn current_token(&self) -> &Token {
        if let Some(&index) = self.significant_indices.get(self.significant_cursor) {
            return self.builder.original_token(index);
        }
        self.builder.original_token(self.original_token_count - 1)
    }

    fn checkpoint(&self) -> Checkpoint {
        self.builder.checkpoint()
    }

    fn start(&mut self, kind: SyntaxKind) -> ParseResult {
        self.check_node_budget()?;
        self.builder.start(kind, self.current_offset());
        self.depth += 1;
        Ok(())
    }

    fn start_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) -> ParseResult {
        self.check_node_budget()?;
        self.builder
            .start_at(checkpoint, kind, self.current_offset());
        self.depth += 1;
        Ok(())
    }

    fn finish(&mut self) {
        self.builder.finish();
        self.depth -= 1;
    }

    fn check_node_budget(&mut self) -> ParseResult {
        if self.nodes_started >= self.limits.max_nodes {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::Nodes,
                offset: self.current_offset(),
            });
        }
        self.nodes_started += 1;
        Ok(())
    }
}

const PREFIX_BINDING_POWER: u8 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostfixPolicy {
    None,
    AwaitBoundary,
    All,
}

#[derive(Debug, Clone, Copy)]
struct ExprShape {
    postfix: PostfixPolicy,
    binary: bool,
}

#[derive(Debug, Clone, Copy)]
struct SpilledBinaryFrame {
    left_checkpoint: Checkpoint,
    minimum_binding_power: u8,
    last_non_associative: Option<NonAssociativeFamily>,
}

impl ExprShape {
    fn ordinary() -> Self {
        Self {
            postfix: PostfixPolicy::All,
            binary: true,
        }
    }

    fn closed() -> Self {
        Self {
            postfix: PostfixPolicy::None,
            binary: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonAssociativeFamily {
    Equality,
    Comparison,
    Range,
}

#[derive(Debug, Clone, Copy)]
struct BinaryOperator {
    left_binding_power: u8,
    right_binding_power: u8,
    non_associative_family: Option<NonAssociativeFamily>,
}

fn binary_operator(kind: TokenKind) -> Option<BinaryOperator> {
    let (binding_power, non_associative_family) = match kind {
        TokenKind::With => (1, None),
        TokenKind::Or => (2, None),
        TokenKind::And => (3, None),
        TokenKind::EqEq | TokenKind::BangEq => (4, Some(NonAssociativeFamily::Equality)),
        TokenKind::Less
        | TokenKind::LessEq
        | TokenKind::Greater
        | TokenKind::GreaterEq
        | TokenKind::In => (5, Some(NonAssociativeFamily::Comparison)),
        TokenKind::DotDot | TokenKind::DotDotEq => (6, Some(NonAssociativeFamily::Range)),
        TokenKind::Pipe => (7, None),
        TokenKind::Caret => (8, None),
        TokenKind::Amp => (9, None),
        TokenKind::Shl | TokenKind::Shr => (10, None),
        TokenKind::Plus | TokenKind::Minus => (11, None),
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => (12, None),
        _ => return None,
    };
    Some(BinaryOperator {
        left_binding_power: binding_power,
        right_binding_power: binding_power + 1,
        non_associative_family,
    })
}

fn is_postfix_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot | TokenKind::Question
    )
}

fn is_plain_postfix_start(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot
    )
}

fn is_assignment_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::AmpEq
            | TokenKind::CaretEq
            | TokenKind::PipeEq
            | TokenKind::ShlEq
            | TokenKind::ShrEq
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};
    use crate::syntax::ast::{Declaration, SourceFile};
    use crate::syntax::format::format_parsed;
    use crate::syntax::{LexMode, lex};

    fn parse_source(source: &[u8], mode: ParseMode) -> (SourceDatabase, FileId, Parsed) {
        parse_source_with_limits(source, mode, ParseLimits::default())
    }

    fn parse_source_with_limits(
        source: &[u8],
        mode: ParseMode,
        limits: ParseLimits,
    ) -> (SourceDatabase, FileId, Parsed) {
        parse_source_result(source, mode, limits).expect("parser test source should parse")
    }

    fn parse_source_result(
        source: &[u8],
        mode: ParseMode,
        limits: ParseLimits,
    ) -> Result<(SourceDatabase, FileId, Parsed), ParseError> {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:parser-test").unwrap(),
                ModulePath::new("parser").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                Arc::<[u8]>::from(source),
            ))
            .unwrap();
        let lex_mode = match mode {
            ParseMode::Module => LexMode::Module,
            ParseMode::ImportedModule => LexMode::ImportedModule,
            ParseMode::Script => LexMode::Script,
            ParseMode::Fragment | ParseMode::SyntaxSequence | ParseMode::StandaloneBlock => {
                LexMode::Fragment
            }
        };
        let lexed = lex(&sources, file, lex_mode).unwrap();
        let parsed = parse(&sources, file, lexed, mode, limits)?;
        Ok((sources, file, parsed))
    }

    fn codes(parsed: &Parsed) -> Vec<&str> {
        parsed
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().as_str())
            .collect()
    }

    fn assert_lossless(sources: &SourceDatabase, file: FileId, parsed: &Parsed, expected: &[u8]) {
        let source = sources.get(file).unwrap();
        assert!(parsed.cst().has_exact_physical_partition(source.length()));
        assert_eq!(parsed.cst().reconstruct(source.bytes()), expected);
    }

    #[test]
    fn minimal_module_builds_a_lossless_cst() {
        let source = b"fn main() {}\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed.cst().node(parsed.cst().root()).kind(),
            SyntaxKind::Module
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn test_and_suite_declarations_are_lossless_and_typed() {
        let source = br#"test top_level {
    assert(true)
}

suite arithmetic {
    let offset = 20

    test subtracts_offset {
        assert(offset == 20)
    }

    suite nested {
        test child {
            assert(true)
        }
    }
}
"#;

        for mode in [ParseMode::Module, ParseMode::ImportedModule] {
            let (sources, file, parsed) = parse_source(source, mode);
            assert!(
                parsed.diagnostics().is_empty(),
                "{mode:?}: {:#?}",
                parsed.diagnostics()
            );
            assert_eq!(
                parsed
                    .cst()
                    .nodes()
                    .iter()
                    .filter(|node| node.kind() == SyntaxKind::TestDecl)
                    .count(),
                3,
                "{mode:?}"
            );
            assert_eq!(
                parsed
                    .cst()
                    .nodes()
                    .iter()
                    .filter(|node| node.kind() == SyntaxKind::SuiteDecl)
                    .count(),
                2,
                "{mode:?}"
            );
            assert_eq!(
                parsed
                    .cst()
                    .nodes()
                    .iter()
                    .filter(|node| node.kind() == SyntaxKind::SuiteBlock)
                    .count(),
                2,
                "{mode:?}"
            );
            assert_lossless(&sources, file, &parsed, source);

            let root = SourceFile::root(parsed.cst()).expect("the module root is typed");
            let declarations = root.declarations().collect::<Vec<_>>();
            assert_eq!(declarations.len(), 2, "{mode:?}");
            let top_test = match declarations[0] {
                Declaration::Test(test) => test,
                declaration => panic!("expected top-level test, got {declaration:?}"),
            };
            assert_eq!(
                top_test
                    .name_token()
                    .and_then(|token| token.token().normalized_identifier()),
                Some("top_level")
            );
            assert_eq!(top_test.body().expect("test body").items().count(), 1);

            let suite = match declarations[1] {
                Declaration::Suite(suite) => suite,
                declaration => panic!("expected top-level suite, got {declaration:?}"),
            };
            let body = suite.body().expect("suite body");
            assert_eq!(body.setup().count(), 1);
            assert_eq!(body.members().count(), 2);
            assert_eq!(
                body.members()
                    .filter_map(|member| match member {
                        Declaration::Test(test) => Some(test),
                        _ => None,
                    })
                    .count(),
                1
            );
        }
    }

    #[test]
    fn suite_recovery_rejects_setup_after_first_member_and_keeps_later_members() {
        let source = br#"suite broken {
    let before = 1

    test first {}
    let after = 2

    test second {}
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TestDecl)
                .count(),
            2
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .any(|node| node.kind() == SyntaxKind::Error)
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn nested_test_declarations_are_rejected_without_losing_the_outer_test() {
        let source = br#"test outer {
    test inner {}
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TestDecl)
                .count(),
            1
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn control_flow_cannot_hide_suite_members_during_recovery() {
        let source = br#"suite control {
    test first {}
    if true {
        test nested {}
    }
    test second {}
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TestDecl)
                .count(),
            2
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn test_and_suite_modifiers_are_rejected_but_recovered() {
        let source = br#"pub test invalid {}
async suite invalid {
    test valid {}
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004", "E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TestDecl)
                .count(),
            2
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::SuiteDecl)
                .count(),
            1
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn prohibited_test_and_suite_suffixes_are_diagnosed_losslessly() {
        for source in [
            &b"test with_parameter(value) {}\n"[..],
            &b"suite with_parameter(value) { test child {} }\n"[..],
            &b"test generic[T] {}\n"[..],
            &b"suite generic[T] { test child {} }\n"[..],
            &b"test result {}: Int\n"[..],
            &b"suite result {} ! Error\n"[..],
            &b"test \"string_name\" {}\n"[..],
            &b"test signature\n"[..],
            &b"suite signature\n"[..],
        ] {
            let (sources, file, parsed) = parse_source(source, ParseMode::Module);
            assert!(!parsed.diagnostics().is_empty(), "source: {source:?}");
            assert_lossless(&sources, file, &parsed, source);
        }
    }

    #[test]
    fn test_and_suite_declarations_are_rejected_in_scripts() {
        let source = b"test top_level {}\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::Script);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .all(|node| node.kind() != SyntaxKind::TestDecl)
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn parser_public_error_and_mode_surfaces_are_stable() {
        let source_error = ParseError::from(crate::source::SourceError::EmptySourceId);
        assert_eq!(source_error.to_string(), "source ID cannot be empty");
        let diagnostic_error = ParseError::from(DiagnosticError::InvalidCode("bad".into()));
        assert_eq!(
            diagnostic_error.to_string(),
            "invalid diagnostic code `bad`"
        );
        let resource_error = ParseError::ResourceLimit {
            resource: ParseResource::NestingDepth,
            offset: 17,
        };
        assert_eq!(
            resource_error.to_string(),
            "parser nesting depth limit reached at byte 17"
        );
        assert_eq!(
            ParseResource::Diagnostics.to_string(),
            "primary diagnostic count"
        );

        let type_error = parse_source_result(
            b"type Value = Int\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        )
        .expect_err("a zero type-depth budget must fail before parsing the type");
        assert!(matches!(
            type_error,
            ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                ..
            }
        ));

        let imported_source = b"let value = 1\n";
        let (sources, file, imported) = parse_source(imported_source, ParseMode::ImportedModule);
        assert_eq!(codes(&imported), ["E1801"]);
        assert_lossless(&sources, file, &imported, imported_source);

        let standalone_source = b"{\n    value\n}\n";
        let (sources, file, standalone) =
            parse_source(standalone_source, ParseMode::StandaloneBlock);
        assert!(
            standalone.diagnostics().is_empty(),
            "{:#?}",
            standalone.diagnostics()
        );
        assert_lossless(&sources, file, &standalone, standalone_source);
        let (cst, diagnostics) = standalone.into_parts();
        assert_eq!(cst.node(cst.root()).kind(), SyntaxKind::StandaloneBlock);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn syntax_sequence_accepts_a_bare_value_expression() {
        let source = b"value\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::SyntaxSequence);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn syntax_sequence_accepts_borrow_binding_patterns() {
        let source = b"ref value\nmut value\nvar value\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::SyntaxSequence);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn syntax_sequence_accepts_multiple_statements_and_comments() {
        let source =
            b"var original = [1, 2, 3]\nvar copy = original\n\ncopy[0] = 9\n\n// unchanged\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::SyntaxSequence);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn opaque_outcomes_are_syntax_only_for_free_inherent_and_associated_functions() {
        let valid = br#"type Item = { value: Int }
fn free(): impl Discard { 1 }
fn Item.method(self): impl Discard { self.value }
fn Item.associated(): impl Discard { 1 }
async fn later(): impl Discard ! String { 1 }
"#;
        let (_, _, parsed) = parse_source(valid, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::OpaqueOutcome)
                .count(),
            4
        );

        for source in [
            &b"trait Invalid {\n    fn make(): impl Discard\n}\n"[..],
            &b"trait Contract { fn make(): Int }\ntype Item = Int\nimpl Contract for Item {\n    fn make(): impl Discard { 1 }\n}\n"[..],
            &b"alias Invalid = impl Discard\n"[..],
            &b"type Invalid = { value: impl Discard }\n"[..],
            &b"fn invalid(value: impl Discard) {}\n"[..],
            &b"fn invalid() {\n    let closure = (): impl Discard { 1 }\n}\n"[..],
        ] {
            let (_, _, parsed) = parse_source(source, ParseMode::Module);
            assert!(!parsed.diagnostics().is_empty(), "source: {source:?}");
            assert!(
                parsed
                    .cst()
                    .nodes()
                    .iter()
                    .all(|node| node.kind() != SyntaxKind::OpaqueOutcome),
                "source: {source:?}"
            );
        }
    }

    #[test]
    fn declarations_types_and_methods_parse_together() {
        let source = br#"import std.io

pub type User[T] = {
    name: String
    priv secret: T?
}

pub enum Maybe[T] {
    Present(T)
    Missing
}

pub trait Display {
    fn display(self): String
}

impl Display for User[Int] {
    fn display(self): String {
        self.name
    }
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert!(parsed.diagnostics().is_empty(), "{:?}", codes(&parsed));
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn pratt_precedence_postfix_and_record_update_parse() {
        let source = br#"fn calculate(value: Int): Int {
    let result = repository.find(value)?.score * 2 + 1 << 3 and ready or fallback
    result with { score: result }
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert!(parsed.diagnostics().is_empty(), "{:?}", codes(&parsed));
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::BinaryExpr)
                .count()
                >= 6
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn unary_expressions_remain_operands_of_outer_binary_operators() {
        let source = b"fn calculate(value: Int): Int {\n    -value + 2\n}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .any(|node| node.kind() == SyntaxKind::BinaryExpr)
        );
    }

    #[test]
    fn preliminary_brackets_accept_expression_and_type_grammars() {
        let source = br#"fn use(value: Value, index: Index) {
    consume[fn(Int): String](value)
    consume[Result ! Error](value)
    consume[(Left ! Error) | Right](value)
    value[index.fn]
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::BracketItem)
                .count(),
            4
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn block_last_expression_is_a_tail_even_with_a_newline() {
        let source = b"fn answer(): Int {\n    42\n}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .any(|node| node.kind() == SyntaxKind::TailExpression)
        );
    }

    #[test]
    fn patterns_match_and_multiple_assignment_are_syntax() {
        let source = br#"fn swap(values: Array[Int]): Int {
    let (left, right) = (values[0], values[1])
    (left, right) = (right, left)
    match values {
        [first, ..ref rest] if first > 0 => first
        _ => 0
    }
}
fn edit(
    values: Array[Int],
    entries: Map[Int, Int],
    groups: Array[Array[Int]],
) {
    for mut value in values {}
    for var value in values {}
    for (ref key, mut value) in entries {}
    for [ref first, ..var rest] in groups {}
}
"#;
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .any(|node| node.kind() == SyntaxKind::TupleAssignmentPattern)
        );
    }

    #[test]
    fn record_rest_with_trailing_comma_is_valid_and_has_its_own_node() {
        let source = b"fn read(value: User): Int {\n    match value {\n        User { id, .., } => id\n    }\n}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .any(|node| node.kind() == SyntaxKind::RecordRestPattern)
        );
    }

    #[test]
    fn structurally_invalid_compact_forms_report_e0004() {
        for source in [
            &b"enum Empty {}\n"[..],
            &b"enum EmptyRecord {\n    Value {}\n}\n"[..],
            &b"alias NotAType = ()\n"[..],
            &b"fn oneTuple() {\n    (1,)\n}\n"[..],
            &b"fn mixed() {\n    [1, 2: 3]\n}\n"[..],
            &b"fn mixed() {\n    [1: 2, 3]\n}\n"[..],
        ] {
            let (_, _, parsed) = parse_source(source, ParseMode::Module);
            assert!(codes(&parsed).contains(&"E0004"), "source: {source:?}");
        }
    }

    #[test]
    fn member_recovery_preserves_later_methods_and_declarations() {
        let source = br#"trait Example {
    + + malformed
    fn valid(): Int
}

fn after(): Int {
    1
}
"#;
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::FunctionDecl)
                .count(),
            1
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TraitMethod)
                .count(),
            1
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn statement_recovery_emits_one_primary_and_keeps_the_next_declaration() {
        let source = b"+ + malformed\nfn valid() {}\n";
        let (sources, file, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert!(parsed.cst().nodes().iter().any(|node| {
            node.kind() == SyntaxKind::Error && node.range().start() < node.range().end()
        }));
        assert!(
            parsed.cst().tokens()[parsed.cst().original_token_count()..]
                .iter()
                .all(|token| {
                    token.is_synthetic() && token.range().start() == token.range().end()
                })
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::FunctionDecl)
                .count(),
            1
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn missing_expression_does_not_consume_the_following_declaration() {
        let source = b"const Missing =\nfn valid() {}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0004"]);
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::FunctionDecl)
                .count(),
            1
        );
    }

    #[test]
    fn script_top_level_statements_are_rejected_only_in_module_mode() {
        let source = b"let value = 1\nvalue += 2\n";
        let (_, _, script) = parse_source(source, ParseMode::Script);
        let (_, _, module) = parse_source(source, ParseMode::Module);
        assert!(script.diagnostics().is_empty(), "{:?}", codes(&script));
        assert_eq!(codes(&module), ["E1804", "E1804"]);
    }

    #[test]
    fn non_associative_operator_chains_have_the_specific_code() {
        let source = b"fn invalid(value: Int): Bool {\n    0 < value < 10\n}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0005"]);
    }

    #[test]
    fn interpolation_uses_the_ordinary_expression_parser() {
        for source in [
            &b"fn message(user: User): String {\n    \"hello {user.name + suffix()}\"\n}\n"[..],
            &b"fn answer(): String {\n    \"\"\"\n        answer: {\n            40 + 2\n        }\n        \"\"\"\n}\n"[..],
        ] {
            let (sources, file, parsed) = parse_source(source, ParseMode::Module);
            assert!(parsed.diagnostics().is_empty(), "{:?}", codes(&parsed));
            assert!(
                parsed
                    .cst()
                    .nodes()
                    .iter()
                    .any(|node| node.kind() == SyntaxKind::Interpolation)
            );
            assert_lossless(&sources, file, &parsed, source);
        }
    }

    #[test]
    fn spilled_expression_frames_preserve_deep_group_and_operator_shapes() {
        let depth = 96;
        let mut source = Vec::with_capacity(depth * 2 + 32);
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"left + right * scale");
        source.extend(std::iter::repeat_n(b')', depth));
        source.push(b'\n');
        let (sources, file, parsed) = parse_source_with_limits(
            &source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::GroupExpr)
                .count(),
            depth
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::BinaryExpr)
                .count()
                >= 2
        );
        assert_lossless(&sources, file, &parsed, &source);
    }

    #[test]
    fn spilled_expression_frames_handle_nested_arrays_and_unary_prefixes() {
        let depth = 96;
        let mut array_source = Vec::with_capacity(depth * 2 + 8);
        array_source.extend(std::iter::repeat_n(b'[', depth));
        array_source.extend_from_slice(b"value");
        array_source.extend(std::iter::repeat_n(b']', depth));
        array_source.push(b'\n');
        let (sources, file, parsed) = parse_source_with_limits(
            &array_source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::BracketLiteralExpr)
                .count(),
            depth
        );
        assert_lossless(&sources, file, &parsed, &array_source);

        let mut unary_source = Vec::with_capacity(depth + 8);
        for _ in 0..depth {
            unary_source.extend_from_slice(b"- ");
        }
        unary_source.extend_from_slice(b"value\n");
        let (_, _, parsed) = parse_source_with_limits(
            &unary_source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::PrefixExpr)
                .count(),
            depth
        );
    }

    #[test]
    fn spilled_parentheses_preserve_operator_boundaries_after_nested_groups() {
        let source = b"(((value * (485 - value)) + ((457 * value) * value)) + 893)\n";
        let (sources, file, parsed) = parse_source_with_limits(
            source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 128,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::BinaryExpr)
                .count()
                >= 6
        );
        assert_lossless(&sources, file, &parsed, source);
    }

    #[test]
    fn spill_frames_cover_prefixes_closures_control_flow_and_budget_rejection() {
        for source in [
            &b"await value?\n"[..],
            &b"spawn value\n"[..],
            &b"() { value }\n"[..],
            &b"match value { _ => value, }\n"[..],
            &b"if true { value } else { value }\n"[..],
            &b"scope { value }\n"[..],
            &b"unsafe { value }\n"[..],
            &b"Set[value]\n"[..],
            &b"value with { field: value }\n"[..],
        ] {
            let (_, _, parsed) = parse_source(source, ParseMode::Fragment);
            assert!(
                parsed.diagnostics().is_empty(),
                "{source:?}: {:#?}",
                parsed.diagnostics()
            );
        }
        let (_, _, parsed) = parse_source(
            b"fn f(): Value {\n    let closure = async () { value }\n    closure\n}\n",
            ParseMode::Module,
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );

        let nested = |open: &str, close: &str, inner: &str, count: usize| {
            let mut source = String::new();
            for _ in 0..count {
                source.push_str(open);
            }
            source.push_str(inner);
            for _ in 0..count {
                source.push_str(close);
            }
            source.push('\n');
            source
        };
        for (source, mode) in [
            (nested("(", ")", "value", 8), ParseMode::Fragment),
            (nested("[", "]", "value", 8), ParseMode::Fragment),
            (nested("{", "}", "value", 8), ParseMode::Fragment),
        ] {
            let error = parse_source_result(
                source.as_bytes(),
                mode,
                ParseLimits {
                    max_nesting_depth: 4,
                    ..ParseLimits::default()
                },
            )
            .expect_err("the logical depth budget must reject the source");
            assert!(matches!(
                error,
                ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    ..
                }
            ));
        }
        for source in [
            nested("some(", ")", "value", 8),
            nested("Node{next:", "}", "value", 8),
        ] {
            let error = parse_source_result(
                source.as_bytes(),
                ParseMode::Fragment,
                ParseLimits {
                    max_nesting_depth: 1,
                    ..ParseLimits::default()
                },
            )
            .expect_err("constructor and record depth must be bounded");
            assert!(matches!(
                error,
                ParseError::ResourceLimit {
                    resource: ParseResource::NestingDepth,
                    ..
                }
            ));
        }

        let type_source = "type Broken = ".to_owned() + &nested("A[", "]", "Int", 8);
        let error = parse_source_result(
            type_source.as_bytes(),
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        )
        .expect_err("generic type depth must use the same logical budget");
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                ..
            }
        ));

        let pattern_source = "fn broken(value: Value) { let ".to_owned()
            + &nested("[", "]", "item", 8)
            + " = value\n}\n";
        let error = parse_source_result(
            pattern_source.as_bytes(),
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        )
        .expect_err("pattern depth must use the same logical budget");
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                ..
            }
        ));
    }

    #[test]
    fn spill_frames_cover_boundary_forms_and_lossless_recovery_shapes() {
        let valid = [
            "await value?\n",
            "spawn value\n",
            "Some(value)\n",
            "Ok(value)\n",
            "Err(value)\n",
            "Set[value]\n",
            "value with { field: value }\n",
            "scope { value }\n",
            "unsafe { value }\n",
            "match value { _ => value, }\n",
            "if true { value } else { value }\n",
            "for item in items { item }\n",
            "async () { value }\n",
            "(value, value)\n",
            "[value, value]\n",
            "value.field[0]\n",
            "Node{next: value}\n",
            "a + b * c\n",
            "value?\n",
            "value[0..=1]\n",
        ];
        for source in valid {
            let (sources, file, parsed) = parse_source(source.as_bytes(), ParseMode::Fragment);
            assert_lossless(&sources, file, &parsed, source.as_bytes());
        }

        let invalid = [
            "Some(\n",
            "value with {\n",
            "(value\n",
            "[value\n",
            "match value {\n",
            "Node{next: }\n",
            "if true { value\n",
            "for item in items {\n",
            "value[0..]\n",
            "Set[value: value]\n",
        ];
        for source in invalid {
            let (sources, file, parsed) = parse_source(source.as_bytes(), ParseMode::Fragment);
            assert!(
                !parsed.diagnostics().is_empty(),
                "expected diagnostics for {source:?}"
            );
            assert_lossless(&sources, file, &parsed, source.as_bytes());
        }
    }

    #[test]
    fn spilled_expression_dispatch_covers_heap_atom_forms() {
        let wrap = |expression: &str| format!("(((({expression}))))\n");
        let valid = [
            wrap("{ value }"),
            wrap("scope { value }"),
            wrap("unsafe { value }"),
            wrap("Some(Some(Some(Some(value))))"),
            wrap("value, value, value"),
            wrap("await value?"),
            wrap("spawn value"),
            wrap("if true { value } else { value }"),
            wrap("scope { match value { _ => value\n } }"),
            wrap("async () { value }"),
            wrap("unsafe () { value }"),
            wrap("value.field[0]"),
        ];
        for source in valid {
            let (sources, file, parsed) = parse_source(source.as_bytes(), ParseMode::Fragment);
            assert!(
                parsed.diagnostics().is_empty(),
                "source {source:?}: {:#?}",
                parsed.diagnostics()
            );
            assert_lossless(&sources, file, &parsed, source.as_bytes());
        }

        let block_with_for = "{{{{for item in items { item }\n}}}}\n";
        let (sources, file, parsed) = parse_source(block_with_for.as_bytes(), ParseMode::Fragment);
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(&sources, file, &parsed, block_with_for.as_bytes());

        let (sources, file, parsed) = parse_source(
            b"fn assign(value: Value) {\n    ((((item.field[0], item), item), item), item) = value\n}\n",
            ParseMode::Module,
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(
            &sources,
            file,
            &parsed,
            b"fn assign(value: Value) {\n    ((((item.field[0], item), item), item), item) = value\n}\n",
        );

        let (sources, file, parsed) = parse_source(
            b"fn assign_discard(value: Value) {\n    ((((_, item), item), item), item) = value\n}\n",
            ParseMode::Module,
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_lossless(
            &sources,
            file,
            &parsed,
            b"fn assign_discard(value: Value) {\n    ((((_, item), item), item), item) = value\n}\n",
        );
    }

    #[test]
    fn parser_depth_budget_rejects_every_iterative_entry_point() {
        let assert_depth_error = |source: &str, mode: ParseMode, limits: ParseLimits| {
            let error = parse_source_result(source.as_bytes(), mode, limits)
                .expect_err("the configured nesting budget must reject this source");
            assert!(
                matches!(
                    error,
                    ParseError::ResourceLimit {
                        resource: ParseResource::NestingDepth,
                        ..
                    }
                ),
                "unexpected error for {source:?}: {error:?}"
            );
        };

        assert_depth_error(
            "value\n",
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "if true { value }\n",
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "for item in items { item }\n",
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "fn assign(value: Value) {\n    left = value\n}\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "fn pattern(value: Value) {\n    let item = value\n}\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 0,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "type Grouped = ((((Int))))\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "((((- - - - value))))\n",
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "((((Some(Some(Some(Some(value))))))))\n",
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );

        let mut loops = String::from("fn loops(items: Array[Int]) {\n");
        for _ in 0..9 {
            loops.push_str("    for item in items {\n");
        }
        loops.push_str("        item\n");
        for _ in 0..9 {
            loops.push_str("    }\n");
        }
        loops.push_str("}\n");
        assert_depth_error(
            &loops,
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 5,
                ..ParseLimits::default()
            },
        );

        assert_depth_error(
            "fn assignment(value: Value) {\n    ((((item, item), item), item), item) = value\n}\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "fn tuple_pattern(value: Value) {\n    let ((((item, item, item)))) = value\n}\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );
        assert_depth_error(
            "fn constructor_pattern(value: Value) {\n    let Some(Some(Some(Some(item)))) = value\n}\n",
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 4,
                ..ParseLimits::default()
            },
        );
    }

    #[test]
    fn spilled_expression_frames_are_safe_on_a_small_host_stack() {
        let depth = 4_000;
        let mut source = Vec::with_capacity(depth * 2 + 8);
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"value");
        source.extend(std::iter::repeat_n(b')', depth));
        source.push(b'\n');
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-parser-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 32,
                        max_nodes: depth as u32 * 4,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack parser thread panicked");
    }

    #[test]
    fn spilled_expression_frames_keep_the_logical_depth_limit() {
        let depth = 96;
        let mut source = Vec::with_capacity(depth * 2 + 8);
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"value");
        source.extend(std::iter::repeat_n(b')', depth));
        source.push(b'\n');
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:parser-logical-depth").unwrap(),
                ModulePath::new("parser").unwrap(),
                LogicalPath::new("depth.to").unwrap(),
                Arc::<[u8]>::from(source),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Fragment).unwrap();
        let error = parse(
            &sources,
            file,
            lexed,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 64,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                ..
            }
        ));
    }

    #[test]
    fn spilled_type_frames_handle_deep_generic_arguments_on_a_small_stack() {
        let depth = 2_000;
        let mut source = Vec::with_capacity(depth * 6 + 32);
        source.extend_from_slice(b"type Deep = ");
        for _ in 0..depth {
            source.extend_from_slice(b"Array[");
        }
        source.extend_from_slice(b"Int");
        source.extend(std::iter::repeat_n(b']', depth));
        source.push(b'\n');
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-type-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::GenericArgs)
                        .count()
                        >= depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack type parser panicked");

        let source = b"type Pair = A[B[C[D[Int, String]]]]\n";
        let (_, _, parsed) = parse_source_with_limits(
            source,
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 128,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert_eq!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::GenericArgs)
                .count(),
            4
        );
    }

    #[test]
    fn spilled_type_frames_handle_deep_parenthesized_types_on_a_small_stack() {
        let depth = 2_000;
        let mut source = Vec::with_capacity(depth * 2 + 32);
        source.extend_from_slice(b"type Deep = ");
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"Int");
        source.extend(std::iter::repeat_n(b')', depth));
        source.push(b'\n');
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-type-group-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::GroupType)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle
            .join()
            .expect("small-stack parenthesized type parser panicked");
    }

    #[test]
    fn spilled_pattern_frames_handle_deep_arrays_constructors_and_tuples() {
        let depth = 96;

        let mut array_source = b"fn array(value: Value) {\n    let ".to_vec();
        array_source.extend(std::iter::repeat_n(b'[', depth));
        array_source.extend_from_slice(b"item");
        array_source.extend(std::iter::repeat_n(b']', depth));
        array_source.extend_from_slice(b" = value\n}\n");
        let (sources, file, parsed) = parse_source_with_limits(
            &array_source,
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::ArrayPattern)
                .count()
                >= depth
        );
        assert_lossless(&sources, file, &parsed, &array_source);

        let mut constructor_source =
            b"fn constructor(value: Value): Value {\n    match value {\n        ".to_vec();
        for _ in 0..depth {
            constructor_source.extend_from_slice(b"Some(");
        }
        constructor_source.extend_from_slice(b"item");
        constructor_source.extend(std::iter::repeat_n(b')', depth));
        constructor_source.extend_from_slice(b" => value\n    }\n}\n");
        let (_, _, parsed) = parse_source_with_limits(
            &constructor_source,
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::ConstructorPattern)
                .count()
                >= depth
        );

        let mut tuple_pattern = String::from("item");
        for _ in 0..depth {
            tuple_pattern = format!("({}, item)", tuple_pattern);
        }
        let tuple_source =
            format!("fn tuple(value: Value) {{\n    let {tuple_pattern} = value\n}}\n");
        let (_, _, parsed) = parse_source_with_limits(
            tuple_source.as_bytes(),
            ParseMode::Module,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        assert!(
            parsed
                .cst()
                .nodes()
                .iter()
                .filter(|node| node.kind() == SyntaxKind::TuplePattern)
                .count()
                >= depth
        );
    }

    #[test]
    fn spilled_type_and_pattern_frames_recover_missing_closers() {
        let depth = 2_000;
        let mut type_source = Vec::with_capacity(depth * 6 + 32);
        type_source.extend_from_slice(b"type Broken = ");
        for _ in 0..depth {
            type_source.extend_from_slice(b"Array[");
        }
        type_source.extend_from_slice(b"Int");
        type_source.extend(std::iter::repeat_n(b']', depth - 3));
        type_source.push(b'\n');
        let expected_type = type_source.clone();
        let type_handle = std::thread::Builder::new()
            .name("tondo-invalid-type-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &type_source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(!parsed.diagnostics().is_empty());
                assert_lossless(&sources, file, &parsed, &expected_type);
            })
            .unwrap();
        type_handle
            .join()
            .expect("small-stack invalid type parser panicked");

        let mut pattern_source = b"fn broken(value: Value) {\n    let ".to_vec();
        pattern_source.extend(std::iter::repeat_n(b'[', depth));
        pattern_source.extend_from_slice(b"item");
        pattern_source.extend(std::iter::repeat_n(b']', depth - 3));
        pattern_source.extend_from_slice(b" = value\n}\n");
        let expected_pattern = pattern_source.clone();
        let pattern_handle = std::thread::Builder::new()
            .name("tondo-invalid-pattern-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &pattern_source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(!parsed.diagnostics().is_empty());
                assert_lossless(&sources, file, &parsed, &expected_pattern);
            })
            .unwrap();
        pattern_handle
            .join()
            .expect("small-stack invalid pattern parser panicked");
    }

    #[test]
    fn spilled_if_frames_handle_deep_nested_blocks_on_a_small_stack() {
        let depth = 1000;
        let mut source = b"fn main(): Int {\n".to_vec();
        for _ in 0..depth {
            source.extend_from_slice(b"    if true {\n");
        }
        source.extend_from_slice(b"        1\n");
        for _ in 0..depth {
            source.extend_from_slice(b"    }\n");
        }
        source.extend_from_slice(b"}\n");
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-if-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::IfExpr)
                        .count()
                        >= depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack if parser panicked");
    }

    #[test]
    fn spilled_if_frames_handle_deep_else_if_chains_on_a_small_stack() {
        let depth = 1_000;
        let mut source = b"fn main(): Int {\n    if true {\n        1\n    }".to_vec();
        for _ in 1..depth {
            source.extend_from_slice(b" else if true {\n        1\n    }");
        }
        source.extend_from_slice(b"\n}\n");
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-else-if-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::IfExpr)
                        .count()
                        >= depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack else-if parser panicked");
    }

    #[test]
    fn spilled_for_frames_handle_deep_nested_loops_on_a_small_stack() {
        let depth = 1_000;
        let mut source = b"fn main(items: Array[Int]) {\n".to_vec();
        for _ in 0..depth {
            source.extend_from_slice(b"    for item in items {\n");
        }
        source.extend_from_slice(b"        item\n");
        for _ in 0..depth {
            source.extend_from_slice(b"    }\n");
        }
        source.extend_from_slice(b"}\n");
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-for-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 24,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::ForStmt)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack for parser panicked");
    }

    #[test]
    fn spilled_assignment_frames_handle_deep_tuple_destinations_on_a_small_stack() {
        let depth = 1_000;
        let mut tuple = String::from("item");
        for _ in 0..depth {
            tuple = format!("({}, item)", tuple);
        }
        let source = format!("fn assign(value: Value) {{\n    {tuple} = value\n}}\n");
        let expected = source.clone().into_bytes();
        let handle = std::thread::Builder::new()
            .name("tondo-assignment-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    source.as_bytes(),
                    ParseMode::Module,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 24,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::TupleAssignmentPattern)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle
            .join()
            .expect("small-stack assignment parser panicked");
    }

    #[test]
    fn spilled_expression_frames_handle_deep_block_and_constructor_chains() {
        let depth = 2_000;
        let mut block_source = Vec::with_capacity(depth * 2 + 8);
        block_source.extend(std::iter::repeat_n(b'{', depth));
        block_source.extend_from_slice(b"value");
        block_source.extend(std::iter::repeat_n(b'}', depth));
        block_source.push(b'\n');
        let expected = block_source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-block-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &block_source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 8,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::Block)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack block parser panicked");

        let mut constructor_source = Vec::with_capacity(depth * 5 + 8);
        for _ in 0..depth {
            constructor_source.extend_from_slice(b"some(");
        }
        constructor_source.extend_from_slice(b"value");
        constructor_source.extend(std::iter::repeat_n(b')', depth));
        constructor_source.push(b'\n');
        let (_, _, parsed) = parse_source_with_limits(
            &constructor_source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: depth as u32 + 64,
                max_nodes: depth as u32 * 8,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        let constructors = parsed
            .cst()
            .nodes()
            .iter()
            .filter(|node| node.kind() == SyntaxKind::OptionResultConstructor)
            .count();
        assert_eq!(constructors, depth);

        let mut call_source = Vec::with_capacity(depth * 3 + 8);
        for _ in 0..depth {
            call_source.extend_from_slice(b"f(");
        }
        call_source.extend_from_slice(b"value");
        call_source.extend(std::iter::repeat_n(b')', depth));
        call_source.push(b'\n');
        let expected = call_source.clone();
        let call_handle = std::thread::Builder::new()
            .name("tondo-call-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &call_source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::CallSuffix)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        call_handle
            .join()
            .expect("small-stack call parser panicked");

        let mut record_source = Vec::with_capacity(depth * 14 + 8);
        for _ in 0..depth {
            record_source.extend_from_slice(b"Node{next:");
        }
        record_source.extend_from_slice(b"value");
        record_source.extend(std::iter::repeat_n(b'}', depth));
        record_source.push(b'\n');
        let expected = record_source.clone();
        let record_handle = std::thread::Builder::new()
            .name("tondo-record-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &record_source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 16,
                        ..ParseLimits::default()
                    },
                );
                assert!(
                    parsed.diagnostics().is_empty(),
                    "{:#?}",
                    parsed.diagnostics()
                );
                assert_eq!(
                    parsed
                        .cst()
                        .nodes()
                        .iter()
                        .filter(|node| node.kind() == SyntaxKind::RecordLikeExpr)
                        .count(),
                    depth
                );
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        record_handle
            .join()
            .expect("small-stack record parser panicked");
    }

    #[test]
    fn spilled_frames_recover_deep_invalid_delimiters_without_panicking() {
        let depth = 2_000;
        let mut source = Vec::with_capacity(depth * 2 + 16);
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"value");
        source.extend(std::iter::repeat_n(b')', depth - 3));
        source.push(b'\n');
        let expected = source.clone();
        let handle = std::thread::Builder::new()
            .name("tondo-invalid-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 8,
                        ..ParseLimits::default()
                    },
                );
                assert!(!parsed.diagnostics().is_empty());
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        handle.join().expect("small-stack invalid parser panicked");

        let mut array_source = Vec::with_capacity(depth * 2 + 16);
        array_source.extend(std::iter::repeat_n(b'[', depth));
        array_source.extend_from_slice(b"value");
        array_source.extend(std::iter::repeat_n(b']', depth - 3));
        array_source.push(b'\n');
        let expected = array_source.clone();
        let array_handle = std::thread::Builder::new()
            .name("tondo-invalid-array-small-stack".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                let (sources, file, parsed) = parse_source_with_limits(
                    &array_source,
                    ParseMode::Fragment,
                    ParseLimits {
                        max_nesting_depth: depth as u32 + 64,
                        max_nodes: depth as u32 * 8,
                        ..ParseLimits::default()
                    },
                );
                assert!(!parsed.diagnostics().is_empty());
                assert_lossless(&sources, file, &parsed, &expected);
            })
            .unwrap();
        array_handle
            .join()
            .expect("small-stack invalid array parser panicked");
    }

    #[test]
    fn spilled_cst_shapes_round_trip_through_the_formatter() {
        let depth = 96;
        let mut source = Vec::with_capacity(depth * 2 + 32);
        source.extend(std::iter::repeat_n(b'(', depth));
        source.extend_from_slice(b"value + other");
        source.extend(std::iter::repeat_n(b')', depth));
        source.push(b'\n');
        let (sources, file, parsed) = parse_source_with_limits(
            &source,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            parsed.diagnostics().is_empty(),
            "{:#?}",
            parsed.diagnostics()
        );
        let formatted = format_parsed(&sources, file, &parsed).unwrap().into_bytes();
        let (_, _, reparsed) = parse_source_with_limits(
            &formatted,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 512,
                max_nodes: 100_000,
                ..ParseLimits::default()
            },
        );
        assert!(
            reparsed.diagnostics().is_empty(),
            "formatted source: {:?}; diagnostics: {:#?}",
            String::from_utf8_lossy(&formatted),
            reparsed.diagnostics()
        );
        let significant = |parsed: &Parsed| {
            parsed
                .cst()
                .token_kinds_in_tree_order()
                .into_iter()
                .filter(|kind| !kind.is_trivia())
                .collect::<Vec<_>>()
        };
        assert_eq!(significant(&parsed), significant(&reparsed));
    }

    #[test]
    fn parser_resource_limits_are_typed_failures() {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:parser-limit").unwrap(),
                ModulePath::new("parser").unwrap(),
                LogicalPath::new("input.to").unwrap(),
                Arc::<[u8]>::from(&b"fn main() {}\n"[..]),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        let error = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits {
                max_nodes: 1,
                ..ParseLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::Nodes,
                ..
            }
        ));

        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        let error = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits {
                max_diagnostics: 0,
                ..ParseLimits::default()
            },
        )
        .unwrap_or_else(|error| match error {
            ParseError::ResourceLimit {
                resource: ParseResource::Diagnostics,
                ..
            } => panic!("the valid source did not exercise diagnostics"),
            other => panic!("unexpected parser failure: {other}"),
        });
        assert!(error.diagnostics().is_empty());

        let mut nested_sources = SourceDatabase::new();
        let nested_file = nested_sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:parser-depth").unwrap(),
                ModulePath::new("parser").unwrap(),
                LogicalPath::new("nested.to").unwrap(),
                Arc::<[u8]>::from(&b"((((value))))\n"[..]),
            ))
            .unwrap();
        let lexed = lex(&nested_sources, nested_file, LexMode::Fragment).unwrap();
        let error = parse(
            &nested_sources,
            nested_file,
            lexed,
            ParseMode::Fragment,
            ParseLimits {
                max_nesting_depth: 3,
                ..ParseLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                ..
            }
        ));

        let mut invalid_sources = SourceDatabase::new();
        let invalid_file = invalid_sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:parser-diagnostics").unwrap(),
                ModulePath::new("parser").unwrap(),
                LogicalPath::new("invalid.to").unwrap(),
                Arc::<[u8]>::from(&b"enum Empty {}\n"[..]),
            ))
            .unwrap();
        let lexed = lex(&invalid_sources, invalid_file, LexMode::Module).unwrap();
        let error = parse(
            &invalid_sources,
            invalid_file,
            lexed,
            ParseMode::Module,
            ParseLimits {
                max_diagnostics: 0,
                ..ParseLimits::default()
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ParseError::ResourceLimit {
                resource: ParseResource::Diagnostics,
                ..
            }
        ));
    }
}
