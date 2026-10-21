//! Recursive-descent parser (spec 23).

use crate::ast::*;
use crate::diag::Diagnostics;
use crate::lexer::{self, Kw, StrPart, Tok, Token};
use crate::source::{SourceFile, Span};

pub fn parse_file(file: &SourceFile, diags: &mut Diagnostics) -> Module {
    let tokens = lexer::lex(file, None, diags);
    let mut parser = Parser { file, tokens, pos: 0, diags, recovering: false, no_brace: false };
    parser.module()
}

struct Parser<'a> {
    file: &'a SourceFile,
    tokens: Vec<Token>,
    pos: usize,
    diags: &'a mut Diagnostics,
    /// Suppresses cascading syntax errors until the next statement boundary.
    recovering: bool,
    /// Inside an `if`/`for` header, where `{` starts the control block.
    no_brace: bool,
}

type PResult<T> = Result<T, ()>;

/// `ok`, `err` and `some` construct `Result`/`Option` only when followed by
/// `(`; elsewhere they are ordinary names (spec 5.5).
pub const CONTEXTUAL_CONSTRUCTORS: [&str; 3] = ["ok", "err", "some"];

impl Parser<'_> {
    // ---- token helpers ----------------------------------------------------

    fn tok(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn nth(&self, n: usize) -> &Tok {
        &self.tokens[(self.pos + n).min(self.tokens.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn at(&self, tok: &Tok) -> bool {
        self.tok() == tok
    }

    fn at_kw(&self, kw: Kw) -> bool {
        *self.tok() == Tok::Kw(kw)
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        let found = self.at(tok);
        if found {
            self.bump();
        }
        found
    }

    fn eat_kw(&mut self, kw: Kw) -> bool {
        self.eat(&Tok::Kw(kw))
    }

    fn skip_newlines(&mut self) {
        while self.at(&Tok::Newline) {
            self.bump();
        }
    }

    fn error<T>(&mut self, message: impl Into<String>) -> PResult<T> {
        if !self.recovering {
            let span = self.span();
            self.diags.error("E0004", span, message);
            self.recovering = true;
        }
        Err(())
    }

    fn expected<T>(&mut self, what: &str) -> PResult<T> {
        let found = self.tok().describe();
        self.error(format!("expected {what}, found {found}"))
    }

    fn expect(&mut self, tok: Tok) -> PResult<Span> {
        if self.at(&tok) {
            return Ok(self.bump().span);
        }
        let what = tok.describe();
        self.expected(&what)
    }

    fn ident(&mut self) -> PResult<Ident> {
        match self.tok().clone() {
            Tok::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            Tok::Kw(kw) => self.error(format!("`{}` is a reserved keyword and cannot be used as a name", kw.as_str())),
            _ => self.expected("identifier"),
        }
    }

    /// Skips to the end of the current statement or declaration.
    fn recover(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.tok() {
                Tok::Eof => break,
                Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
                // `}` may close the enclosing block; stray `)`/`]` are skipped.
                Tok::RBrace if depth == 0 => break,
                Tok::RParen | Tok::RBracket if depth == 0 => {}
                Tok::RBrace | Tok::RParen | Tok::RBracket => depth -= 1,
                Tok::Newline if depth == 0 => break,
                _ => {}
            }
            self.bump();
        }
        self.recovering = false;
    }

    fn end_of_statement(&mut self) -> PResult<()> {
        match self.tok() {
            Tok::Newline => {
                self.bump();
                Ok(())
            }
            Tok::RBrace | Tok::Eof => Ok(()),
            _ => self.expected("end of line"),
        }
    }

    // ---- module ----------------------------------------------------------

    fn module(&mut self) -> Module {
        let mut module = Module::default();
        let mut seen_other = false;
        loop {
            self.skip_newlines();
            if self.at(&Tok::Eof) {
                break;
            }
            let result = if self.at_kw(Kw::Import) {
                let start = self.span();
                let import = self.import();
                if seen_other {
                    self.diags.error("E1007", start, "imports must precede every other declaration and statement");
                }
                import.map(|i| module.imports.push(i))
            } else {
                seen_other = true;
                if self.at_declaration() {
                    self.declaration().map(|item| module.items.extend(item))
                } else {
                    self.statement().map(|s| module.statements.push(s))
                }
            };
            if result.is_err() {
                self.recover();
            }
            if self.at(&Tok::RBrace) {
                let found = self.tok().describe();
                self.diags.error("E0004", self.span(), format!("unexpected {found}"));
                self.bump();
            }
        }
        module
    }

    fn import(&mut self) -> PResult<Import> {
        let start = self.bump().span;
        let mut path = vec![self.ident()?];
        while self.eat(&Tok::Dot) {
            path.push(self.ident()?);
        }
        let alias = if self.eat_kw(Kw::As) { Some(self.ident()?) } else { None };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(Import { path, alias, span })
    }

    fn at_declaration(&self) -> bool {
        let tok = if self.at_kw(Kw::Pub) { self.nth(1) } else { self.tok() };
        matches!(
            tok,
            Tok::Kw(
                Kw::Fn
                    | Kw::Const
                    | Kw::Type
                    | Kw::Alias
                    | Kw::Enum
                    | Kw::Trait
                    | Kw::Impl
                    | Kw::Derive
                    | Kw::Test
                    | Kw::Suite
            )
        ) || (matches!(tok, Tok::Kw(Kw::Unsafe)) && matches!(self.nth(1), Tok::Kw(Kw::Fn)))
    }

    fn declaration(&mut self) -> PResult<Option<Item>> {
        let start = self.span();
        let public = self.eat_kw(Kw::Pub);
        match self.tok() {
            Tok::Kw(Kw::Fn) => Ok(Some(Item::Fn(self.function(public, start)?))),
            Tok::Kw(Kw::Type) => Ok(Some(Item::Type(self.type_declaration(public, start)?))),
            Tok::Kw(Kw::Enum) => Ok(Some(Item::Enum(self.enum_declaration(public, start)?))),
            Tok::Kw(Kw::Alias) => {
                self.bump();
                let name = self.ident()?;
                self.no_generics()?;
                self.expect(Tok::Eq)?;
                let target = self.type_expr()?;
                let span = start.to(target.span);
                self.end_of_statement()?;
                Ok(Some(Item::Alias(AliasDecl { public, name, target, span })))
            }
            Tok::Kw(kw) => {
                let what = format!("`{}` declarations", kw.as_str());
                self.diags.unsupported(start, &what);
                Err(())
            }
            _ => self.expected("declaration"),
        }
    }

    fn no_generics(&mut self) -> PResult<()> {
        if self.at(&Tok::LBracket) {
            self.diags.unsupported(self.span(), "generic declarations");
            return Err(());
        }
        Ok(())
    }

    fn type_declaration(&mut self, public: bool, start: Span) -> PResult<TypeDecl> {
        self.bump();
        let name = self.ident()?;
        self.no_generics()?;
        self.expect(Tok::Eq)?;
        let body = if self.at(&Tok::LBrace) {
            TypeBody::Record(self.record_body()?)
        } else {
            TypeBody::Newtype(self.type_expr()?)
        };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(TypeDecl { public, name, body, span })
    }

    fn enum_declaration(&mut self, public: bool, start: Span) -> PResult<EnumDecl> {
        self.bump();
        let name = self.ident()?;
        self.no_generics()?;
        self.expect(Tok::LBrace)?;
        let mut variants = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                break;
            }
            let name = self.ident()?;
            let payload = match self.tok() {
                Tok::LParen => {
                    self.bump();
                    let mut items = Vec::new();
                    while !self.at(&Tok::RParen) {
                        items.push(self.type_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(Tok::RParen)?;
                    VariantPayload::Tuple(items)
                }
                Tok::LBrace => VariantPayload::Record(self.record_body()?),
                _ => VariantPayload::Unit,
            };
            variants.push(VariantDecl { name, payload });
            self.field_separator()?;
        }
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(EnumDecl { public, name, variants, span })
    }

    /// `{ [priv] name: Type, ... }` separated by commas or newlines.
    fn record_body(&mut self) -> PResult<Vec<FieldDecl>> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                return Ok(fields);
            }
            // `priv` is a modifier only when another field name follows (23.10).
            let private = self.at_kw(Kw::Priv) && self.nth(1) != &Tok::Colon;
            if private {
                self.bump();
            }
            let name = self.field_name()?;
            self.expect(Tok::Colon)?;
            let ty = self.type_expr()?;
            fields.push(FieldDecl { private, name, ty });
            self.field_separator()?;
        }
    }

    /// A field name may be a keyword (5.4).
    fn field_name(&mut self) -> PResult<Ident> {
        match self.tok().clone() {
            Tok::Ident(name) => Ok(Ident { name, span: self.bump().span }),
            Tok::Kw(kw) => Ok(Ident { name: kw.as_str().into(), span: self.bump().span }),
            _ => self.expected("field name"),
        }
    }

    /// `,` and/or newlines between fields; nothing is needed before `}`.
    fn field_separator(&mut self) -> PResult<()> {
        let comma = self.eat(&Tok::Comma);
        let newline = self.at(&Tok::Newline);
        self.skip_newlines();
        if comma || newline || self.at(&Tok::RBrace) { Ok(()) } else { self.expected("`,` or a new line") }
    }

    fn field_inits(&mut self) -> PResult<Vec<FieldInit>> {
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let result = self.field_inits_inner();
        self.no_brace = no_brace;
        result
    }

    fn field_inits_inner(&mut self) -> PResult<Vec<FieldInit>> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                return Ok(fields);
            }
            let name = self.field_name()?;
            let value = if self.eat(&Tok::Colon) { Some(self.expression()?) } else { None };
            fields.push(FieldInit { name, value });
            self.field_separator()?;
        }
    }

    fn function(&mut self, public: bool, start: Span) -> PResult<FnDecl> {
        self.expect(Tok::Kw(Kw::Fn))?;
        let first = self.ident()?;
        let (owner, name) = if self.eat(&Tok::Dot) { (Some(first), self.ident()?) } else { (None, first) };
        if self.at(&Tok::LBracket) {
            self.diags.unsupported(self.span(), "generic functions");
            return Err(());
        }
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        while !self.at(&Tok::RParen) {
            let name = self.ident()?;
            self.expect(Tok::Colon)?;
            let ty = self.type_expr()?;
            params.push(Param { span: name.span.to(ty.span), name, ty });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        let result = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
        let suspends = self.eat_kw(Kw::Suspends);
        let body = self.block()?;
        let span = start.to(body.span);
        self.end_of_statement()?;
        Ok(FnDecl { public, owner, name, params, result, suspends, body, span })
    }

    // ---- types -----------------------------------------------------------

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let first = self.result_type()?;
        if !self.at(&Tok::Pipe) {
            return Ok(first);
        }
        let mut members = vec![first];
        while self.eat(&Tok::Pipe) {
            members.push(self.result_type()?);
        }
        let span = members[0].span.to(self.prev_span());
        Ok(TypeExpr { kind: TypeKind::Union(members), span })
    }

    fn result_type(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        if self.eat(&Tok::Bang) {
            let err = self.error_type()?;
            return Ok(TypeExpr { span: start.to(err.span), kind: TypeKind::Result { ok: None, err: Box::new(err) } });
        }
        let ok = self.optional_type()?;
        if !self.eat(&Tok::Bang) {
            return Ok(ok);
        }
        let err = self.error_type()?;
        Ok(TypeExpr {
            span: ok.span.to(err.span),
            kind: TypeKind::Result { ok: Some(Box::new(ok)), err: Box::new(err) },
        })
    }

    fn error_type(&mut self) -> PResult<TypeExpr> {
        if self.at(&Tok::LParen) {
            let start = self.bump().span;
            let inner = self.type_expr()?;
            self.expect(Tok::RParen)?;
            return Ok(TypeExpr { kind: inner.kind, span: start.to(self.prev_span()) });
        }
        self.optional_type()
    }

    fn optional_type(&mut self) -> PResult<TypeExpr> {
        let inner = self.primary_type()?;
        if self.eat(&Tok::Question) {
            return Ok(TypeExpr { span: inner.span.to(self.prev_span()), kind: TypeKind::Optional(Box::new(inner)) });
        }
        Ok(inner)
    }

    fn primary_type(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        match self.tok() {
            Tok::Ident(_) => {
                let mut segments = vec![self.ident()?];
                while self.eat(&Tok::Dot) {
                    segments.push(self.ident()?);
                }
                let mut args = Vec::new();
                if self.eat(&Tok::LBracket) {
                    while !self.at(&Tok::RBracket) {
                        args.push(self.type_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(Tok::RBracket)?;
                }
                Ok(TypeExpr { kind: TypeKind::Path { segments, args }, span: start.to(self.prev_span()) })
            }
            Tok::LParen => {
                self.bump();
                let first = self.type_expr()?;
                if self.eat(&Tok::RParen) {
                    return Ok(TypeExpr { kind: first.kind, span: start.to(self.prev_span()) });
                }
                let mut items = vec![first];
                while self.eat(&Tok::Comma) && !self.at(&Tok::RParen) {
                    items.push(self.type_expr()?);
                }
                self.expect(Tok::RParen)?;
                Ok(TypeExpr { kind: TypeKind::Tuple(items), span: start.to(self.prev_span()) })
            }
            Tok::Kw(Kw::Fn | Kw::Unsafe) => {
                self.diags.unsupported(start, "function types");
                Err(())
            }
            _ => self.expected("type"),
        }
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self) -> PResult<Block> {
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let result = self.block_inner();
        self.no_brace = no_brace;
        result
    }

    fn block_inner(&mut self) -> PResult<Block> {
        let start = self.expect(Tok::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail = None;
        loop {
            self.skip_newlines();
            if self.at(&Tok::RBrace) || self.at(&Tok::Eof) {
                break;
            }
            let before = self.pos;
            match self.statement_or_tail() {
                Ok(Ok(stmt)) => stmts.push(stmt),
                Ok(Err(expr)) => {
                    tail = Some(Box::new(expr));
                    break;
                }
                Err(()) => {
                    self.recover();
                    if self.pos == before && !self.at(&Tok::RBrace) {
                        self.bump();
                    }
                }
            }
        }
        let end = self.expect(Tok::RBrace)?;
        Ok(Block { stmts, tail, span: start.to(end) })
    }

    /// Parses a statement, or the expression that ends a block (`Err(expr)`).
    fn statement_or_tail(&mut self) -> PResult<Result<Stmt, Expr>> {
        if self.starts_statement() {
            return self.statement().map(Ok);
        }
        let expr = self.expression()?;
        let save = self.pos;
        self.skip_newlines();
        if self.at(&Tok::RBrace) {
            return Ok(Err(expr));
        }
        self.pos = save;
        self.finish_expression_statement(expr).map(Ok)
    }

    fn starts_statement(&self) -> bool {
        matches!(
            self.tok(),
            Tok::Kw(Kw::Let | Kw::Var | Kw::Return | Kw::Fail | Kw::Break | Kw::Continue | Kw::For | Kw::Defer)
        ) || matches!(self.tok(), Tok::Ident(name) if name == "_")
    }

    fn statement(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kind = match self.tok().clone() {
            Tok::Kw(kw @ (Kw::Let | Kw::Var)) => {
                self.bump();
                let pattern = self.pattern()?;
                let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                let value = if self.eat(&Tok::Eq) { Some(self.expression()?) } else { None };
                StmtKind::Let { mutable: kw == Kw::Var, pattern, ty, value }
            }
            Tok::Kw(Kw::Return | Kw::Fail | Kw::Break | Kw::Continue) => self.transfer()?,
            Tok::Kw(Kw::For) => self.for_statement()?,
            Tok::Kw(Kw::Defer) => {
                self.diags.unsupported(start, "`defer`");
                return Err(());
            }
            Tok::Ident(name) if name == "_" && self.nth(1) == &Tok::Eq => {
                self.bump();
                self.bump();
                StmtKind::Discard(self.expression()?)
            }
            _ => {
                let expr = self.expression()?;
                return self.finish_expression_statement(expr);
            }
        };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(Stmt { kind, span })
    }

    fn finish_expression_statement(&mut self, expr: Expr) -> PResult<Stmt> {
        let op = match self.tok() {
            Tok::Eq => Some(None),
            Tok::PlusEq => Some(Some(BinOp::Add)),
            Tok::MinusEq => Some(Some(BinOp::Sub)),
            Tok::StarEq => Some(Some(BinOp::Mul)),
            Tok::SlashEq => Some(Some(BinOp::Div)),
            Tok::PercentEq => Some(Some(BinOp::Rem)),
            Tok::AmpEq => Some(Some(BinOp::BitAnd)),
            Tok::PipeEq => Some(Some(BinOp::BitOr)),
            Tok::CaretEq => Some(Some(BinOp::BitXor)),
            Tok::ShlEq => Some(Some(BinOp::Shl)),
            Tok::ShrEq => Some(Some(BinOp::Shr)),
            _ => None,
        };
        let start = expr.span;
        let kind = match op {
            Some(op) => {
                self.bump();
                let value = self.expression()?;
                StmtKind::Assign { target: expr, op, value }
            }
            None => StmtKind::Expr(expr),
        };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(Stmt { kind, span })
    }

    /// Whether the `for` header has an `in` at its top level, which makes it
    /// an iteration (23.16).
    fn header_has_in(&self) -> bool {
        let mut depth = 0usize;
        for token in &self.tokens[self.pos..] {
            match token.tok {
                Tok::Kw(Kw::In) if depth == 0 => return true,
                Tok::LBrace if depth == 0 => return false,
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
                Tok::Eof | Tok::Newline if depth == 0 => return false,
                _ => {}
            }
        }
        false
    }

    fn for_statement(&mut self) -> PResult<StmtKind> {
        self.bump();
        if self.at(&Tok::LBrace) {
            return Ok(StmtKind::For { header: ForHeader::Infinite, body: self.block()? });
        }
        let in_header = self.header_has_in();
        let no_brace = std::mem::replace(&mut self.no_brace, true);
        let header = if in_header {
            let pattern = self.pattern()?;
            self.expect(Tok::Kw(Kw::In))?;
            self.expression().map(|source| ForHeader::In { pattern, source })
        } else {
            self.expression().map(ForHeader::Condition)
        };
        self.no_brace = no_brace;
        Ok(StmtKind::For { header: header?, body: self.block()? })
    }

    // ---- expressions -----------------------------------------------------

    fn expression(&mut self) -> PResult<Expr> {
        match self.tok() {
            Tok::Kw(Kw::If) => self.if_expression(),
            Tok::Kw(Kw::Match) => self.match_expression(),
            Tok::Kw(kw @ Kw::Select) => {
                let what = format!("`{}` expressions", kw.as_str());
                self.diags.unsupported(self.span(), &what);
                Err(())
            }
            _ => {
                let mut expr = self.binary(0)?;
                while self.eat_kw(Kw::With) {
                    let updates = self.field_inits()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::With { base: Box::new(expr), updates }, span };
                }
                Ok(expr)
            }
        }
    }

    fn match_expression(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let no_brace = std::mem::replace(&mut self.no_brace, true);
        let scrutinee = self.expression();
        self.no_brace = no_brace;
        let scrutinee = Box::new(scrutinee?);
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let arms = self.match_arms();
        self.no_brace = no_brace;
        let arms = arms?;
        Ok(Expr { kind: ExprKind::Match { scrutinee, arms }, span: start.to(self.prev_span()) })
    }

    fn match_arms(&mut self) -> PResult<Vec<MatchArm>> {
        self.expect(Tok::LBrace)?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                return Ok(arms);
            }
            if self.at(&Tok::Eof) {
                return self.expected("`}`");
            }
            match self.match_arm() {
                Ok(arm) => arms.push(arm),
                // Recover at the next arm.
                Err(()) => self.recover(),
            }
        }
    }

    fn match_arm(&mut self) -> PResult<MatchArm> {
        let pattern = self.pattern()?;
        let guard = if self.eat_kw(Kw::If) { Some(self.expression()?) } else { None };
        self.expect(Tok::FatArrow)?;
        let body = match self.tok() {
            Tok::Kw(Kw::Return | Kw::Fail | Kw::Break | Kw::Continue) => {
                let start = self.span();
                let kind = self.transfer()?;
                ArmBody::Transfer(Box::new(Stmt { kind, span: start.to(self.prev_span()) }))
            }
            Tok::Ident(name) if name == "_" && self.nth(1) == &Tok::Eq => {
                return self.error("a statement in a match arm needs a block: `=> { ... }`");
            }
            _ => ArmBody::Expr(self.expression()?),
        };
        if matches!(self.tok(), Tok::Eq | Tok::PlusEq | Tok::MinusEq | Tok::StarEq | Tok::SlashEq) {
            return self.error("an assignment in a match arm needs a block: `=> { ... }`");
        }
        // An arm ends at a new line or a comma (23.17).
        if !self.eat(&Tok::Comma) && !self.at(&Tok::Newline) && !self.at(&Tok::RBrace) {
            return self.expected("a new line after the match arm");
        }
        Ok(MatchArm { span: pattern.span.to(self.prev_span()), pattern, guard, body })
    }

    /// `return`, `fail`, `break` or `continue` (shared by statements and arms).
    fn transfer(&mut self) -> PResult<StmtKind> {
        Ok(match self.bump().tok {
            Tok::Kw(Kw::Return) => {
                let value = if matches!(self.tok(), Tok::Newline | Tok::RBrace | Tok::Eof | Tok::Comma) {
                    None
                } else {
                    Some(self.expression()?)
                };
                StmtKind::Return(value)
            }
            Tok::Kw(Kw::Fail) => StmtKind::Fail(self.expression()?),
            Tok::Kw(Kw::Break) => StmtKind::Break,
            _ => StmtKind::Continue,
        })
    }

    // ---- patterns --------------------------------------------------------

    fn pattern(&mut self) -> PResult<Pattern> {
        let start = self.span();
        let kind = match self.tok().clone() {
            Tok::Ident(name) if name == "_" => {
                self.bump();
                PatternKind::Wildcard
            }
            Tok::Kw(Kw::None) => {
                self.bump();
                PatternKind::None
            }
            Tok::Kw(Kw::True | Kw::False) | Tok::Int { .. } | Tok::Float { .. } | Tok::Char(_) => {
                PatternKind::Literal(self.primary()?)
            }
            Tok::Str(parts) => {
                if parts.iter().any(|p| matches!(p, StrPart::Expr(_))) {
                    return self.error("an interpolated string is not a pattern");
                }
                PatternKind::Literal(self.primary()?)
            }
            Tok::Minus if matches!(self.nth(1), Tok::Int { .. } | Tok::Float { .. }) => {
                PatternKind::Literal(self.unary()?)
            }
            Tok::LParen => {
                self.bump();
                if self.eat(&Tok::RParen) {
                    PatternKind::Literal(Expr { kind: ExprKind::Unit, span: start.to(self.prev_span()) })
                } else {
                    let first = self.pattern()?;
                    if self.eat(&Tok::RParen) {
                        return Ok(Pattern { kind: first.kind, span: start.to(self.prev_span()) });
                    }
                    let mut items = vec![first];
                    while self.eat(&Tok::Comma) && !self.at(&Tok::RParen) {
                        items.push(self.pattern()?);
                    }
                    self.expect(Tok::RParen)?;
                    PatternKind::Tuple(items)
                }
            }
            Tok::Ident(_) => {
                let mut path = vec![self.ident()?];
                while self.eat(&Tok::Dot) {
                    path.push(self.ident()?);
                }
                match self.tok() {
                    Tok::LBracket => {
                        // `Type[Args](pattern)` tests a union member (9.6).
                        self.bump();
                        let mut args = vec![self.type_expr()?];
                        while self.eat(&Tok::Comma) && !self.at(&Tok::RBracket) {
                            args.push(self.type_expr()?);
                        }
                        self.expect(Tok::RBracket)?;
                        let ty = TypeExpr {
                            kind: TypeKind::Path { segments: path, args },
                            span: start.to(self.prev_span()),
                        };
                        self.expect(Tok::LParen)?;
                        let inner = self.pattern()?;
                        self.expect(Tok::RParen)?;
                        PatternKind::Member { ty, inner: Box::new(inner) }
                    }
                    Tok::LParen => {
                        self.bump();
                        let mut args = Vec::new();
                        while !self.at(&Tok::RParen) {
                            args.push(self.pattern()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(Tok::RParen)?;
                        PatternKind::Constructor { path, args }
                    }
                    Tok::LBrace => {
                        let (fields, rest) = self.field_patterns()?;
                        PatternKind::Record { path, fields, rest }
                    }
                    _ if path.len() == 1 => PatternKind::Binding(path.pop().unwrap()),
                    _ => PatternKind::Path(path),
                }
            }
            Tok::LBracket => {
                self.diags.unsupported(start, "array patterns");
                return Err(());
            }
            Tok::Kw(Kw::Ref | Kw::Mut | Kw::Var) => {
                self.diags.unsupported(start, "loan patterns");
                return Err(());
            }
            _ => return self.expected("pattern"),
        };
        Ok(Pattern { kind, span: start.to(self.prev_span()) })
    }

    fn field_patterns(&mut self) -> PResult<(Vec<FieldPattern>, bool)> {
        self.expect(Tok::LBrace)?;
        let mut fields = Vec::new();
        let mut rest = false;
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                return Ok((fields, rest));
            }
            if self.eat(&Tok::DotDot) {
                rest = true;
            } else {
                let name = self.field_name()?;
                let pattern = if self.eat(&Tok::Colon) { Some(self.pattern()?) } else { None };
                fields.push(FieldPattern { name, pattern });
            }
            self.field_separator()?;
        }
    }

    fn if_expression(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let no_brace = std::mem::replace(&mut self.no_brace, true);
        let condition = self.expression();
        self.no_brace = no_brace;
        let condition = Box::new(condition?);
        let then = self.block()?;
        let otherwise = if self.eat_kw(Kw::Else) {
            if self.at_kw(Kw::If) {
                Some(Box::new(self.if_expression()?))
            } else {
                let block = self.block()?;
                Some(Box::new(Expr { span: block.span, kind: ExprKind::Block(block) }))
            }
        } else {
            None
        };
        let span = start.to(self.prev_span());
        Ok(Expr { kind: ExprKind::If { condition, then, otherwise }, span })
    }

    /// Precedence climbing over levels 3–13 of spec 17.11 (lowest first).
    fn binary(&mut self, level: usize) -> PResult<Expr> {
        const LEVELS: usize = 11;
        if level == LEVELS {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        let non_associative = matches!(level, 2..=4);
        let mut chained = false;
        while let Some(op) = self.binary_op(level) {
            if non_associative && chained {
                let span = self.span();
                self.diags.error(
                    "E0005",
                    span,
                    format!("`{}` cannot be chained; combine comparisons with `and`", op.as_str()),
                );
                return Err(());
            }
            self.bump();
            let right = self.binary(level + 1)?;
            let span = left.span.to(right.span);
            left = Expr { kind: ExprKind::Binary(op, Box::new(left), Box::new(right)), span };
            chained = true;
        }
        Ok(left)
    }

    fn binary_op(&self, level: usize) -> Option<BinOp> {
        let op = match (level, self.tok()) {
            (0, Tok::Kw(Kw::Or)) => BinOp::Or,
            (1, Tok::Kw(Kw::And)) => BinOp::And,
            (2, Tok::EqEq) => BinOp::Eq,
            (2, Tok::NotEq) => BinOp::Ne,
            (3, Tok::Lt) => BinOp::Lt,
            (3, Tok::Le) => BinOp::Le,
            (3, Tok::Gt) => BinOp::Gt,
            (3, Tok::Ge) => BinOp::Ge,
            (3, Tok::Kw(Kw::In)) => BinOp::In,
            (4, Tok::DotDot) => BinOp::Range,
            (4, Tok::DotDotEq) => BinOp::RangeInclusive,
            (5, Tok::Pipe) => BinOp::BitOr,
            (6, Tok::Caret) => BinOp::BitXor,
            (7, Tok::Amp) => BinOp::BitAnd,
            (8, Tok::Shl) => BinOp::Shl,
            (8, Tok::Shr) => BinOp::Shr,
            (9, Tok::Plus) => BinOp::Add,
            (9, Tok::Minus) => BinOp::Sub,
            (10, Tok::Star) => BinOp::Mul,
            (10, Tok::Slash) => BinOp::Div,
            (10, Tok::Percent) => BinOp::Rem,
            _ => return None,
        };
        Some(op)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let op = match self.tok() {
            Tok::Minus => UnaryOp::Neg,
            Tok::Kw(Kw::Not) => UnaryOp::Not,
            Tok::Tilde => UnaryOp::BitNot,
            Tok::Kw(kw @ (Kw::Await | Kw::Spawn)) => {
                let what = format!("`{}`", kw.as_str());
                self.diags.unsupported(self.span(), &what);
                return Err(());
            }
            _ => return self.postfix(),
        };
        let start = self.bump().span;
        let operand = self.unary()?;
        let span = start.to(operand.span);
        Ok(Expr { kind: ExprKind::Unary(op, Box::new(operand)), span })
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.primary()?;
        loop {
            match self.tok() {
                Tok::LParen => {
                    self.bump();
                    let args = self.call_arguments()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Call { callee: Box::new(expr), args }, span };
                }
                Tok::Dot => {
                    self.bump();
                    let field = match self.tok().clone() {
                        Tok::Ident(name) => Ident { name, span: self.bump().span },
                        Tok::Kw(kw) => Ident { name: kw.as_str().into(), span: self.bump().span },
                        Tok::Int { value, suffix: None } => {
                            let span = expr.span.to(self.bump().span);
                            expr = Expr { kind: ExprKind::TupleSlot(Box::new(expr), value as u32), span };
                            continue;
                        }
                        // `pair.0.1` lexes `0.1` as one number.
                        Tok::Float { text, suffix: None }
                            if text.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) =>
                        {
                            let span = expr.span.to(self.bump().span);
                            for slot in text.split('.') {
                                expr = Expr {
                                    kind: ExprKind::TupleSlot(Box::new(expr), slot.parse().unwrap_or(u32::MAX)),
                                    span,
                                };
                            }
                            continue;
                        }
                        _ => return self.expected("field or method name"),
                    };
                    let span = expr.span.to(field.span);
                    expr = match expr.kind {
                        ExprKind::Path(mut segments) => {
                            segments.push(field);
                            Expr { kind: ExprKind::Path(segments), span }
                        }
                        kind => Expr { kind: ExprKind::Field(Box::new(Expr { kind, span: expr.span }), field), span },
                    };
                }
                Tok::Question => {
                    self.bump();
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Try(Box::new(expr)), span };
                }
                Tok::LBracket if matches!(&expr.kind, ExprKind::Path(path) if path.len() == 1 && path[0].name == "Set") =>
                {
                    // `Set[...]` in expression position is a set literal (23.24).
                    let ExprKind::Array(items) = self.bracket_literal()?.kind else {
                        return self.error("a set literal lists values: `Set[a, b]`");
                    };
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Set(items), span };
                }
                Tok::LBracket => {
                    if matches!(expr.kind, ExprKind::Path(_))
                        && let Some(args) = self.try_generic_arguments()
                    {
                        let span = expr.span.to(self.prev_span());
                        expr = Expr { kind: ExprKind::Generic { base: Box::new(expr), args }, span };
                        continue;
                    }
                    expr = self.index_or_slice(expr)?;
                }
                Tok::LBrace if !self.no_brace && matches!(expr.kind, ExprKind::Path(_)) => {
                    let ExprKind::Path(path) = expr.kind else { unreachable!() };
                    let fields = self.field_inits()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Record { path, fields }, span };
                }
                _ => return Ok(expr),
            }
        }
    }

    /// `[Type, ...]` directly followed by a call. Undone without diagnostics
    /// when the brackets are not type arguments (23.27).
    fn try_generic_arguments(&mut self) -> Option<Vec<TypeExpr>> {
        let (pos, errors, recovering) = (self.pos, self.diags.list.len(), self.recovering);
        let attempt = (|| -> PResult<Vec<TypeExpr>> {
            self.expect(Tok::LBracket)?;
            let mut args = vec![self.type_expr()?];
            while self.eat(&Tok::Comma) && !self.at(&Tok::RBracket) {
                args.push(self.type_expr()?);
            }
            self.expect(Tok::RBracket)?;
            if !self.at(&Tok::LParen) {
                return Err(());
            }
            Ok(args)
        })();
        if attempt.is_err() {
            self.pos = pos;
            self.diags.list.truncate(errors);
            self.recovering = recovering;
        }
        attempt.ok()
    }

    /// `base[index]` or `base[start:end:step]` (23.22).
    fn index_or_slice(&mut self, base: Expr) -> PResult<Expr> {
        self.bump();
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let result = self.index_or_slice_inner(base);
        self.no_brace = no_brace;
        result
    }

    fn index_or_slice_inner(&mut self, base: Expr) -> PResult<Expr> {
        let start = if self.at(&Tok::Colon) { None } else { Some(Box::new(self.expression()?)) };
        if !self.eat(&Tok::Colon) {
            let Some(index) = start else { return self.expected("index") };
            self.expect(Tok::RBracket)?;
            let span = base.span.to(self.prev_span());
            return Ok(Expr { kind: ExprKind::Index { base: Box::new(base), index }, span });
        }
        let end =
            if matches!(self.tok(), Tok::Colon | Tok::RBracket) { None } else { Some(Box::new(self.expression()?)) };
        // A second `:` requires the step (23.22).
        let step = if self.eat(&Tok::Colon) { Some(Box::new(self.expression()?)) } else { None };
        self.expect(Tok::RBracket)?;
        let span = base.span.to(self.prev_span());
        Ok(Expr { kind: ExprKind::Slice { base: Box::new(base), start, end, step }, span })
    }

    fn call_arguments(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        while !self.at(&Tok::RParen) {
            let start = self.span();
            if matches!(self.tok(), Tok::Kw(Kw::Ref | Kw::Mut | Kw::Var)) || self.at(&Tok::Ellipsis) {
                self.diags.unsupported(start, "loan and spread arguments");
                return Err(());
            }
            let name = if matches!(self.tok(), Tok::Ident(_)) && self.nth(1) == &Tok::Colon {
                let name = self.ident()?;
                self.bump();
                Some(name)
            } else {
                None
            };
            let value = self.expression()?;
            args.push(Arg { span: start.to(value.span), name, value });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(Tok::RParen)?;
        Ok(args)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let kind = match self.tok().clone() {
            Tok::Int { value, suffix } => {
                self.bump();
                ExprKind::Int { value, suffix }
            }
            Tok::Float { text, suffix } => {
                self.bump();
                ExprKind::Float { text, suffix }
            }
            Tok::Char(c) => {
                self.bump();
                ExprKind::Char(c)
            }
            Tok::Kw(Kw::True) => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::Kw(Kw::False) => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Str(parts) => {
                self.bump();
                ExprKind::Str(self.string_segments(parts)?)
            }
            Tok::Ident(name) if CONTEXTUAL_CONSTRUCTORS.contains(&name.as_str()) && self.nth(1) == &Tok::LParen => {
                let constructor = CONTEXTUAL_CONSTRUCTORS.iter().find(|c| **c == name).unwrap();
                self.bump();
                self.bump();
                let no_brace = std::mem::replace(&mut self.no_brace, false);
                let value = self.expression();
                self.no_brace = no_brace;
                let value = value?;
                self.expect(Tok::RParen)?;
                ExprKind::Construct(constructor, Box::new(value))
            }
            Tok::Kw(Kw::None) => {
                self.bump();
                ExprKind::None
            }
            Tok::Ident(name) => {
                self.bump();
                ExprKind::Path(vec![Ident { name, span: start }])
            }
            Tok::LParen => return self.paren(),
            Tok::LBrace if !self.no_brace => {
                let block = self.block()?;
                ExprKind::Block(block)
            }
            Tok::Kw(Kw::If) => return self.if_expression(),
            Tok::LBracket => return self.bracket_literal(),
            Tok::Kw(kw @ (Kw::Scope | Kw::Unsafe | Kw::SelfValue)) => {
                let what = format!("`{}`", kw.as_str());
                self.diags.unsupported(start, &what);
                return Err(());
            }
            _ => return self.expected("expression"),
        };
        Ok(Expr { kind, span: start.to(self.prev_span()) })
    }

    /// `[]`, `[a, b]`; map literals use `:` (23.23).
    fn bracket_literal(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let result = (|| -> PResult<ExprKind> {
            if self.at(&Tok::Colon) && self.nth(1) == &Tok::RBracket {
                self.bump();
                self.bump();
                return Ok(ExprKind::Map(Vec::new()));
            }
            if self.eat(&Tok::RBracket) {
                return Ok(ExprKind::Array(Vec::new()));
            }
            let first = self.expression()?;
            if self.eat(&Tok::Colon) {
                // A `:` after the first entry's key makes it a map (23.23).
                let mut pairs = vec![(first, self.expression()?)];
                while self.eat(&Tok::Comma) && !self.at(&Tok::RBracket) {
                    let key = self.expression()?;
                    self.expect(Tok::Colon)?;
                    pairs.push((key, self.expression()?));
                }
                self.expect(Tok::RBracket)?;
                return Ok(ExprKind::Map(pairs));
            }
            let mut items = vec![first];
            while self.eat(&Tok::Comma) && !self.at(&Tok::RBracket) {
                items.push(self.expression()?);
            }
            self.expect(Tok::RBracket)?;
            Ok(ExprKind::Array(items))
        })();
        self.no_brace = no_brace;
        Ok(Expr { kind: result?, span: start.to(self.prev_span()) })
    }

    fn paren(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        if self.eat(&Tok::RParen) {
            return Ok(Expr { kind: ExprKind::Unit, span: start.to(self.prev_span()) });
        }
        let outer_no_brace = std::mem::replace(&mut self.no_brace, false);
        let result = self.paren_inner(start, outer_no_brace);
        self.no_brace = outer_no_brace;
        result
    }

    fn paren_inner(&mut self, start: Span, outer_no_brace: bool) -> PResult<Expr> {
        let first = self.expression()?;
        if self.eat(&Tok::RParen) {
            if self.at(&Tok::LBrace) && !outer_no_brace {
                self.diags.unsupported(start, "closures");
                return Err(());
            }
            return Ok(Expr { kind: first.kind, span: start.to(self.prev_span()) });
        }
        let mut items = vec![first];
        while self.eat(&Tok::Comma) && !self.at(&Tok::RParen) {
            items.push(self.expression()?);
        }
        self.expect(Tok::RParen)?;
        Ok(Expr { kind: ExprKind::Tuple(items), span: start.to(self.prev_span()) })
    }

    fn string_segments(&mut self, parts: Vec<StrPart>) -> PResult<Vec<StrSegment>> {
        let mut segments = Vec::new();
        for part in parts {
            match part {
                StrPart::Text(text) => segments.push(StrSegment::Text(text)),
                StrPart::Expr(span) => {
                    let tokens = lexer::lex(self.file, Some(span), self.diags);
                    let mut inner = Parser {
                        file: self.file,
                        tokens,
                        pos: 0,
                        diags: &mut *self.diags,
                        recovering: false,
                        no_brace: false,
                    };
                    inner.skip_newlines();
                    let expr = inner.expression()?;
                    inner.skip_newlines();
                    if !inner.at(&Tok::Eof) {
                        return inner.expected("end of interpolation");
                    }
                    segments.push(StrSegment::Expr(expr));
                }
            }
        }
        Ok(segments)
    }
}
