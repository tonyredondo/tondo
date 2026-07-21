use std::error::Error;
use std::fmt;

use crate::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticError, PrimaryLocation, Severity};
use crate::source::{FileId, SourceDatabase, SourceError};

use super::cst::{Checkpoint, CstBuilder, TokenId};
use super::{Cst, Lexed, SyntaxKind, Token, TokenKind};

const MAX_SAFE_NESTING_DEPTH: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMode {
    Module,
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
            max_nesting_depth: MAX_SAFE_NESTING_DEPTH,
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
    let mut parser = Parser {
        sources,
        file,
        mode,
        limits,
        builder: CstBuilder::new(tokens),
        original_token_count,
        cursor: 0,
        diagnostics: Vec::new(),
        nodes_started: 0,
        depth: 0,
        recursion_depth: 0,
        header_expression_depth: 0,
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
    cursor: usize,
    diagnostics: Vec<Diagnostic>,
    nodes_started: u32,
    depth: u32,
    recursion_depth: u32,
    header_expression_depth: u32,
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
            ParseMode::Module => SyntaxKind::Module,
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
            if self.mode == ParseMode::Module && !had_syntax_error {
                self.push_diagnostic_at(
                    "E0006",
                    "statements are only allowed at top level in scripts",
                    None,
                    range,
                    actual,
                )?;
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

    fn parse_expression_or_assignment_statement(&mut self, allow_tail: bool) -> ParseResult {
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
        if self.recursion_depth >= self.limits.max_nesting_depth.min(MAX_SAFE_NESTING_DEPTH) {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
                offset: self.current_offset(),
            });
        }
        self.recursion_depth += 1;
        let result = self.parse_expression_bp_inner(minimum_binding_power);
        self.recursion_depth -= 1;
        result
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
            TokenKind::Identifier => self.parse_path_or_record_expression()?,
            TokenKind::SelfKw => {
                self.start(SyntaxKind::SelfExpr)?;
                self.bump();
                self.finish();
            }
            TokenKind::LParen => self.parse_tuple_or_group_expression()?,
            TokenKind::LBracket => self.parse_bracket_literal()?,
            TokenKind::LBrace => self.parse_block()?,
            TokenKind::Scope => {
                self.start(SyntaxKind::ScopeExpr)?;
                self.bump();
                self.parse_block()?;
                self.finish();
            }
            TokenKind::Unsafe if self.nth(1) == TokenKind::LBrace => {
                self.start(SyntaxKind::UnsafeExpr)?;
                self.bump();
                self.parse_block()?;
                self.finish();
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
                if !self.at(TokenKind::InterpolationEnd) {
                    self.parse_expression()?;
                }
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
        match self.current() {
            TokenKind::Identifier if self.at_discard() => {
                self.start(SyntaxKind::WildcardPattern)?;
                self.bump();
                self.finish();
            }
            TokenKind::Identifier => self.parse_named_pattern()?,
            TokenKind::Ref => {
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
                if self.eat(TokenKind::Ref) {
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
            if self.at(TokenKind::Ref) && self.nth(1) != TokenKind::Colon {
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
        self.at(TokenKind::Ref) || self.line_has_pattern_rest()
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
        let mut index = self.cursor;
        let mut seen = 0;
        while index < self.original_token_count {
            let kind = self.builder.original_token(index).kind();
            if !kind.is_trivia() {
                if seen == significant_offset {
                    return kind;
                }
                seen += 1;
            }
            index += 1;
        }
        TokenKind::Eof
    }

    fn current_offset(&self) -> u32 {
        let mut index = self.cursor;
        while index < self.original_token_count {
            let token = self.builder.original_token(index);
            if !token.kind().is_trivia() {
                return token.range().start();
            }
            index += 1;
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
        let mut index = self.cursor;
        while index < self.original_token_count {
            let token = self.builder.original_token(index);
            if !token.kind().is_trivia() {
                return token;
            }
            index += 1;
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
        if self.depth >= self.limits.max_nesting_depth.min(MAX_SAFE_NESTING_DEPTH) {
            return Err(ParseError::ResourceLimit {
                resource: ParseResource::NestingDepth,
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
    use crate::syntax::{LexMode, lex};

    fn parse_source(source: &[u8], mode: ParseMode) -> (SourceDatabase, FileId, Parsed) {
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
            ParseMode::Script => LexMode::Script,
            ParseMode::Fragment | ParseMode::SyntaxSequence | ParseMode::StandaloneBlock => {
                LexMode::Fragment
            }
        };
        let lexed = lex(&sources, file, lex_mode).unwrap();
        let parsed = parse(&sources, file, lexed, mode, ParseLimits::default()).unwrap();
        (sources, file, parsed)
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
        assert_eq!(codes(&module), ["E0006", "E0006"]);
    }

    #[test]
    fn non_associative_operator_chains_have_the_specific_code() {
        let source = b"fn invalid(value: Int): Bool {\n    0 < value < 10\n}\n";
        let (_, _, parsed) = parse_source(source, ParseMode::Module);
        assert_eq!(codes(&parsed), ["E0005"]);
    }

    #[test]
    fn interpolation_uses_the_ordinary_expression_parser() {
        let source = b"fn message(user: User): String {\n    \"hello {user.name + suffix()}\"\n}\n";
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
