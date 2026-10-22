//! Recursive-descent parser (spec 23).

use crate::ast::*;
use crate::diag::Diagnostics;
use crate::lexer::{self, Kw, StrPart, Tok, Token};
use crate::source::{SourceFile, Span};

pub fn parse_file(file: &SourceFile, diags: &mut Diagnostics) -> Module {
    parse_with_parens(file, diags).0
}

/// The module and the spans of the expressions written in parentheses,
/// which the formatter keeps.
pub fn parse_with_parens(file: &SourceFile, diags: &mut Diagnostics) -> (Module, Vec<Span>) {
    let tokens = lexer::lex(file, None, diags);
    let mut parser = Parser { file, tokens, pos: 0, diags, recovering: false, no_brace: false, parens: Vec::new() };
    let module = parser.module();
    (module, parser.parens)
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
    /// Expressions written in parentheses.
    parens: Vec<Span>,
}

type PResult<T> = Result<T, ()>;

/// `ok`, `err` and `some` construct `Result`/`Option` only when followed by
/// `(`; elsewhere they are ordinary names (spec 5.5).
pub const CONTEXTUAL_CONSTRUCTORS: [&str; 3] = ["ok", "err", "some"];

/// The shared collections of `std.sync`, whose qualified names take a
/// literal: `sync.Queue[a, b]` (5.11).
const SYNC_COLLECTIONS: [&str; 5] = ["Array", "Map", "Set", "Stack", "Queue"];

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
        let offset = usize::from(self.at_kw(Kw::Pub));
        let tok = self.nth(offset);
        if matches!(tok, Tok::At | Tok::Kw(Kw::Extern)) {
            return true;
        }
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
        ) || (matches!(tok, Tok::Kw(Kw::Unsafe)) && matches!(self.nth(offset + 1), Tok::Kw(Kw::Fn)))
    }

    fn declaration(&mut self) -> PResult<Option<Item>> {
        let start = self.span();
        let annotations = self.annotations()?;
        if !annotations.is_empty() && !matches!(self.tok(), Tok::Kw(Kw::Pub | Kw::Extern | Kw::Fn | Kw::Type)) {
            return self.expected("a declaration after the annotation");
        }
        let public = self.eat_kw(Kw::Pub);
        let item = match self.tok() {
            Tok::Kw(Kw::Extern) if self.nth(1) == &Tok::Kw(Kw::Fn) => {
                self.bump();
                let f = self.function(public, start, true)?;
                if let Some(body) = &f.body {
                    self.diags.error("E0004", body.span, "an `extern fn` has no body: C defines it");
                }
                Item::Fn(f)
            }
            Tok::Kw(Kw::Extern) if self.nth(1) == &Tok::Kw(Kw::Type) => {
                self.bump();
                let mut t = self.type_declaration(public, start)?;
                t.foreign = true;
                Item::Type(t)
            }
            Tok::Kw(Kw::Extern) => return self.expected("`fn` or `type` after `extern`"),
            _ if !annotations.is_empty() => {
                return self
                    .declaration_after(public, start)
                    .map(|item| item.map(|item| Self::annotate(item, annotations)));
            }
            _ => return self.declaration_after(public, start),
        };
        Ok(Some(Self::annotate(item, annotations)))
    }

    /// Gives a declaration the annotations written before it; `@symbol`
    /// names the C function an `extern fn` calls.
    fn annotate(mut item: Item, annotations: Vec<Annotation>) -> Item {
        match &mut item {
            Item::Fn(f) => {
                if let Some(foreign) = &mut f.foreign
                    && let Some(AnnotationArg::Text(symbol)) =
                        annotations.iter().find(|a| a.name.name == "symbol").and_then(|a| a.argument.clone())
                {
                    foreign.symbol = symbol;
                }
                f.annotations = annotations;
            }
            Item::Type(t) => t.annotations = annotations,
            _ => {}
        }
        item
    }

    /// `@name`, `@name("text")` or `@name(16)`, each on its own line or
    /// before the declaration or field it annotates.
    fn annotations(&mut self) -> PResult<Vec<Annotation>> {
        let mut annotations = Vec::new();
        while self.at(&Tok::At) {
            let start = self.bump().span;
            let name = self.ident()?;
            let argument = if self.eat(&Tok::LParen) {
                let argument = match self.tok().clone() {
                    // A plain string, without interpolation.
                    Tok::Str(parts) => parts
                        .iter()
                        .map(|p| match p {
                            crate::lexer::StrPart::Text(text) => Some(text.as_str()),
                            crate::lexer::StrPart::Expr(_) => None,
                        })
                        .collect::<Option<String>>()
                        .map(AnnotationArg::Text),
                    Tok::Int { value, suffix: None } => Some(AnnotationArg::Int(value)),
                    _ => None,
                };
                let Some(argument) = argument else {
                    return self.expected("a string without interpolation or an integer");
                };
                self.bump();
                self.expect(Tok::RParen)?;
                Some(argument)
            } else {
                None
            };
            annotations.push(Annotation { name, argument, span: start.to(self.prev_span()) });
            self.skip_newlines();
        }
        Ok(annotations)
    }

    fn declaration_after(&mut self, public: bool, start: Span) -> PResult<Option<Item>> {
        match self.tok() {
            Tok::Kw(Kw::Fn) => Ok(Some(Item::Fn(self.function(public, start, false)?))),
            Tok::Kw(Kw::Unsafe) if self.nth(1) == &Tok::Kw(Kw::Fn) => {
                self.bump();
                let mut f = self.function(public, start, false)?;
                f.is_unsafe = true;
                Ok(Some(Item::Fn(f)))
            }
            Tok::Kw(Kw::Type) => Ok(Some(Item::Type(self.type_declaration(public, start)?))),
            Tok::Kw(Kw::Const) => {
                self.bump();
                let name = self.ident()?;
                let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                self.expect(Tok::Eq)?;
                let value = self.expression()?;
                let span = start.to(self.prev_span());
                self.end_of_statement()?;
                Ok(Some(Item::Const(ConstDecl { public, name, ty, value, span, module: 0 })))
            }
            Tok::Kw(Kw::Enum) => Ok(Some(Item::Enum(self.enum_declaration(public, start)?))),
            Tok::Kw(Kw::Trait) => Ok(Some(Item::Trait(self.trait_declaration(public, start)?))),
            Tok::Kw(Kw::Impl) => Ok(Some(Item::Impl(self.impl_declaration(start)?))),
            Tok::Kw(Kw::Test | Kw::Suite) => {
                if public {
                    let keyword = self.tok().describe();
                    self.diags.error("E0004", start, format!("a {keyword} is never `pub`: remove `pub`"));
                }
                let node = self.test_node()?;
                self.end_of_statement()?;
                Ok(Some(Item::Test(node)))
            }
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

    /// `test name { ... }` or `suite name { setup members }` (testing 3.1).
    fn test_node(&mut self) -> PResult<TestNode> {
        let start = self.span();
        let suite = self.at_kw(Kw::Suite);
        self.bump();
        let name = self.ident()?;
        if !suite {
            let body = self.block()?;
            let span = start.to(body.span);
            return Ok(TestNode::Test(TestDecl { name, span, body, module: 0, class: SourceClass::Production }));
        }
        self.expect(Tok::LBrace)?;
        let (mut setup, mut members) = (Vec::new(), Vec::new());
        loop {
            self.skip_newlines();
            if self.at(&Tok::RBrace) || self.at(&Tok::Eof) {
                break;
            }
            let before = self.pos;
            let result = if self.at_kw(Kw::Test) || self.at_kw(Kw::Suite) {
                self.test_node().and_then(|node| {
                    members.push(node);
                    self.end_of_statement()
                })
            } else if !members.is_empty() {
                let message = "after its first `test` or `suite`, a suite holds only tests and suites: \
                     move this statement before them";
                self.diags.error("E0004", self.span(), message);
                Err(())
            } else {
                self.statement_or_tail().map(|stmt| {
                    setup.push(stmt.unwrap_or_else(|expr| Stmt { span: expr.span, kind: StmtKind::Expr(expr) }))
                })
            };
            if result.is_err() {
                self.recover();
                if self.pos == before && !self.at(&Tok::RBrace) {
                    self.bump();
                }
            }
        }
        let end = self.expect(Tok::RBrace)?;
        let span = start.to(end);
        Ok(TestNode::Suite(SuiteDecl { name, setup, members, span, module: 0, class: SourceClass::Production }))
    }

    fn no_generics(&mut self) -> PResult<()> {
        if self.at(&Tok::LBracket) {
            self.diags.unsupported(self.span(), "generic declarations");
            return Err(());
        }
        Ok(())
    }

    /// `[T, U: Bound + Bound]` after a declared name (12.1), or nothing.
    fn generic_params(&mut self) -> PResult<Vec<GenericParam>> {
        let mut params = Vec::new();
        if !self.eat(&Tok::LBracket) {
            return Ok(params);
        }
        loop {
            self.skip_newlines();
            if self.at(&Tok::RBracket) {
                break;
            }
            let name = self.ident()?;
            let mut bounds = Vec::new();
            if self.eat(&Tok::Colon) {
                bounds.push(self.bound(false)?);
                while self.eat(&Tok::Plus) {
                    bounds.push(self.bound(false)?);
                }
            }
            params.push(GenericParam { name, bounds });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.skip_newlines();
        self.expect(Tok::RBracket)?;
        Ok(params)
    }

    fn type_declaration(&mut self, public: bool, start: Span) -> PResult<TypeDecl> {
        let foreign = self.prev_is_extern();
        self.bump();
        let name = self.ident()?;
        let generics = self.generic_params()?;
        // `extern type Name` declares an opaque C type.
        let body = if foreign && !self.at(&Tok::Eq) {
            TypeBody::Opaque
        } else {
            self.expect(Tok::Eq)?;
            if self.at(&Tok::LBrace) {
                TypeBody::Record(self.record_body()?)
            } else {
                TypeBody::Newtype(self.type_expr()?)
            }
        };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(TypeDecl { public, foreign: false, annotations: Vec::new(), name, generics, body, span, module: 0 })
    }

    /// Whether the token before the current one is `extern`.
    fn prev_is_extern(&self) -> bool {
        self.pos > 0 && self.tokens[self.pos - 1].tok == Tok::Kw(Kw::Extern)
    }

    fn enum_declaration(&mut self, public: bool, start: Span) -> PResult<EnumDecl> {
        self.bump();
        let name = self.ident()?;
        let generics = self.generic_params()?;
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
        Ok(EnumDecl { public, name, generics, variants, span, module: 0 })
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
            let annotations = self.annotations()?;
            // `priv` is a modifier only when another field name follows (23.10).
            let private = self.at_kw(Kw::Priv) && self.nth(1) != &Tok::Colon;
            if private {
                self.bump();
            }
            let name = self.field_name()?;
            self.expect(Tok::Colon)?;
            let ty = self.type_expr()?;
            fields.push(FieldDecl { private, name, ty, annotations });
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

    /// A function or method; with `foreign`, an `extern fn`, whose
    /// parameters may end with C's `...`.
    fn function(&mut self, public: bool, start: Span, foreign: bool) -> PResult<FnDecl> {
        self.expect(Tok::Kw(Kw::Fn))?;
        let first = self.ident()?;
        let first_generics = self.generic_params()?;
        let (owner, owner_generics, name, generics) = if self.eat(&Tok::Dot) {
            let name = self.ident()?;
            (Some(first), first_generics, name, self.generic_params()?)
        } else {
            (None, Vec::new(), first, first_generics)
        };
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&Tok::RParen) {
                break;
            }
            params.push(self.parameter(params.is_empty())?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.skip_newlines();
        self.expect(Tok::RParen)?;
        let dots = params.last().filter(|p| p.name.name == "...").map(|p| p.span);
        if dots.is_some() {
            params.pop();
        }
        let misplaced = params.iter().find(|p| p.name.name == "...").map(|p| p.span);
        if let Some(span) = misplaced.or(dots.filter(|_| !foreign)) {
            let message = if foreign && misplaced.is_some() {
                "C's `...` must be the last parameter"
            } else {
                "C's `...` only ends the parameters of an `extern fn`; a Tondo variadic parameter is `name: ...T`"
            };
            self.diags.error("E0004", span, message);
        }
        let variadic = dots.is_some();
        let result = if self.eat(&Tok::Colon) { Some(self.function_result()?) } else { None };
        // A required method of a trait ends at its signature.
        let body = if self.at(&Tok::LBrace) { Some(self.block()?) } else { None };
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(FnDecl {
            public,
            is_unsafe: false,
            foreign: foreign.then(|| Foreign { symbol: name.name.clone(), variadic }),
            annotations: Vec::new(),
            owner,
            owner_generics,
            name,
            generics,
            params,
            result,
            body,
            span,
            module: 0,
        })
    }

    /// One bound of a list: a type, or `~Copy`. In an opaque result each
    /// bound is a path, since a `!` that follows belongs to the function.
    fn bound(&mut self, opaque: bool) -> PResult<TypeExpr> {
        let start = self.span();
        if !self.eat(&Tok::Tilde) {
            return if opaque { self.primary_type() } else { self.type_expr() };
        }
        let name = self.ident()?;
        if name.name != "Copy" {
            self.diags.error("E0004", name.span, "only `Copy` can be relaxed with `~`: write `~Copy`");
        }
        Ok(TypeExpr { kind: TypeKind::NotCopy, span: start.to(name.span) })
    }

    /// A function's result: a type, or `impl A + B` with an optional
    /// `! E` channel (12.8), which no other position accepts.
    fn function_result(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        if !self.eat_kw(Kw::Impl) {
            return self.type_expr();
        }
        // Each bound is a path; the `!` that may follow belongs to the function.
        let mut bounds = vec![self.bound(true)?];
        while self.eat(&Tok::Plus) {
            bounds.push(self.bound(true)?);
        }
        let opaque = TypeExpr { kind: TypeKind::Opaque(bounds), span: start.to(self.prev_span()) };
        if !self.eat(&Tok::Bang) {
            return Ok(opaque);
        }
        let err = self.error_type()?;
        Ok(TypeExpr {
            span: start.to(err.span),
            kind: TypeKind::Result { ok: Some(Box::new(opaque)), err: Box::new(err) },
        })
    }

    fn trait_declaration(&mut self, public: bool, start: Span) -> PResult<TraitDecl> {
        self.bump();
        let name = self.ident()?;
        let generics = self.generic_params()?;
        let methods = self.method_block()?;
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(TraitDecl { public, name, generics, methods, span, module: 0 })
    }

    fn impl_declaration(&mut self, start: Span) -> PResult<ImplDecl> {
        self.bump();
        let generics = self.generic_params()?;
        let trait_ref = self.type_expr()?;
        self.expect(Tok::Kw(Kw::For))?;
        let no_brace = std::mem::replace(&mut self.no_brace, true);
        let target = self.type_expr();
        self.no_brace = no_brace;
        let target = target?;
        let methods = self.method_block()?;
        let span = start.to(self.prev_span());
        self.end_of_statement()?;
        Ok(ImplDecl { generics, trait_ref, target, methods, span, module: 0 })
    }

    /// `{ fn ... }` of a trait or implementation.
    fn method_block(&mut self) -> PResult<Vec<FnDecl>> {
        self.expect(Tok::LBrace)?;
        let mut methods = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat(&Tok::RBrace) {
                return Ok(methods);
            }
            let start = self.span();
            let is_unsafe = self.at_kw(Kw::Unsafe) && self.nth(1) == &Tok::Kw(Kw::Fn);
            if is_unsafe {
                self.bump();
            }
            if !self.at_kw(Kw::Fn) {
                return self.expected("`fn`");
            }
            let mut method = self.function(false, start, false)?;
            method.is_unsafe = is_unsafe;
            methods.push(method);
        }
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
            Tok::Kw(Kw::Fn) => {
                self.bump();
                self.expect(Tok::LParen)?;
                let (mut params, mut modes, mut variadic) = (Vec::new(), Vec::new(), false);
                loop {
                    self.skip_newlines();
                    if self.at(&Tok::RParen) {
                        break;
                    }
                    modes.push(self.mode());
                    variadic = self.eat(&Tok::Ellipsis);
                    // C's `...`, last in an `extern fn(...)` type, has a
                    // mode but no type.
                    if variadic {
                        let save = self.pos;
                        self.eat(&Tok::Comma);
                        self.skip_newlines();
                        if self.at(&Tok::RParen) {
                            break;
                        }
                        self.pos = save;
                    }
                    params.push(self.type_expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.skip_newlines();
                self.expect(Tok::RParen)?;
                let result = if self.eat(&Tok::Colon) { Some(Box::new(self.result_type()?)) } else { None };
                Ok(TypeExpr {
                    kind: TypeKind::Fn { params, modes, variadic, result, is_unsafe: false, is_extern: false },
                    span: start.to(self.prev_span()),
                })
            }
            Tok::Kw(Kw::Unsafe | Kw::Extern) if self.nth(1) == &Tok::Kw(Kw::Fn) => {
                let keyword = self.bump().tok;
                // A `?` after it belongs to the whole function type.
                let mut ty = self.primary_type()?;
                if let TypeKind::Fn { is_unsafe, is_extern, .. } = &mut ty.kind {
                    *is_unsafe = keyword == Tok::Kw(Kw::Unsafe);
                    *is_extern = keyword == Tok::Kw(Kw::Extern);
                }
                ty.span = start.to(ty.span);
                Ok(ty)
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
        if self.at_kw(Kw::Test) || self.at_kw(Kw::Suite) {
            let keyword = self.tok().describe();
            let message = format!("a {keyword} is declared at the top level of a test file or inside a `suite`");
            self.diags.error("E0004", self.span(), message);
            return Err(());
        }
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
                self.bump();
                if self.at(&Tok::LBrace) {
                    StmtKind::Defer(Defer::Block(self.block()?))
                } else {
                    StmtKind::Defer(Defer::Call(self.expression()?))
                }
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

    /// `=` (as `Some(None)`) or a compound assignment's operator.
    fn assignment_op(&self) -> Option<Option<BinOp>> {
        match self.tok() {
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
        }
    }

    fn finish_expression_statement(&mut self, expr: Expr) -> PResult<Stmt> {
        let op = self.assignment_op();
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
        for (offset, token) in self.tokens[self.pos..].iter().enumerate() {
            match token.tok {
                Tok::Kw(Kw::In) if depth == 0 => return true,
                // A record pattern, as in `for Point { x, y } in points`,
                // unless the brace opens the body.
                Tok::LBrace if depth == 0 => return self.record_pattern_before_in(self.pos + offset),
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => depth = depth.saturating_sub(1),
                Tok::Eof | Tok::Newline if depth == 0 => return false,
                _ => {}
            }
        }
        false
    }

    /// Whether the braces opening at `open` close right before `in`.
    fn record_pattern_before_in(&self, open: usize) -> bool {
        let mut depth = 0usize;
        for (offset, token) in self.tokens[open..].iter().enumerate() {
            match token.tok {
                Tok::LParen | Tok::LBracket | Tok::LBrace => depth += 1,
                Tok::RParen | Tok::RBracket | Tok::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(self.tokens.get(open + offset + 1), Some(t) if t.tok == Tok::Kw(Kw::In));
                    }
                }
                Tok::Eof => return false,
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
        let header = header?;
        if let ForHeader::In { source, .. } | ForHeader::Condition(source) = &header {
            self.record_in_header(source)?;
        }
        Ok(StmtKind::For { header, body: self.block()? })
    }

    /// A record literal cannot follow a header expression directly, since
    /// its `{` would open the body (23.27); suggests the parentheses.
    fn record_in_header(&mut self, header: &Expr) -> PResult<()> {
        let path = matches!(header.kind, ExprKind::Path(_) | ExprKind::Generic { .. });
        if path && self.at(&Tok::LBrace) && matches!(self.nth(1), Tok::Ident(_)) && self.nth(2) == &Tok::Colon {
            return self.error("a record literal in a header needs parentheses: `(Type { field: value })`");
        }
        Ok(())
    }

    // ---- expressions -----------------------------------------------------

    fn expression(&mut self) -> PResult<Expr> {
        match self.tok() {
            Tok::Kw(Kw::If) => self.if_expression(),
            Tok::Kw(Kw::Match) => self.match_expression(),
            Tok::Kw(Kw::Select) => self.select_expression(),
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
        let scrutinee = scrutinee?;
        self.record_in_header(&scrutinee)?;
        let scrutinee = Box::new(scrutinee);
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
        let body = self.arm_body()?;
        Ok(MatchArm { span: pattern.span.to(self.prev_span()), pattern, guard, body })
    }

    /// `select { arms }` (11.14).
    fn select_expression(&mut self) -> PResult<Expr> {
        let start = self.bump().span;
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let arms = self.select_arms();
        self.no_brace = no_brace;
        let arms = arms?;
        Ok(Expr { kind: ExprKind::Select(arms), span: start.to(self.prev_span()) })
    }

    fn select_arms(&mut self) -> PResult<Vec<SelectArm>> {
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
            match self.select_arm() {
                Ok(arm) => arms.push(arm),
                Err(()) => self.recover(),
            }
        }
    }

    fn select_arm(&mut self) -> PResult<SelectArm> {
        let start = self.span();
        let (pattern, operation) = if self.eat_kw(Kw::Else) {
            (None, None)
        } else if self.eat_kw(Kw::Let) {
            let pattern = self.pattern()?;
            self.expect(Tok::Eq)?;
            (Some(pattern), Some(self.expression()?))
        } else {
            (None, Some(self.expression()?))
        };
        self.expect(Tok::FatArrow)?;
        let body = self.arm_body()?;
        Ok(SelectArm { pattern, operation, body, span: start.to(self.prev_span()) })
    }

    /// What follows `=>` in a `match` or `select` arm, up to its end.
    fn arm_body(&mut self) -> PResult<ArmBody> {
        let start = self.span();
        let statement =
            |kind: StmtKind, this: &Self| ArmBody::Statement(Box::new(Stmt { kind, span: start.to(this.prev_span()) }));
        let body = match self.tok() {
            Tok::Kw(Kw::Return | Kw::Fail | Kw::Break | Kw::Continue) => {
                let kind = self.transfer()?;
                statement(kind, self)
            }
            Tok::Ident(name) if name == "_" && self.nth(1) == &Tok::Eq => {
                self.bump();
                self.bump();
                let kind = StmtKind::Discard(self.expression()?);
                statement(kind, self)
            }
            Tok::Kw(kw @ (Kw::Let | Kw::Var | Kw::Defer | Kw::For)) => {
                return self.error(format!("`{}` in a match arm needs a block: `=> {{ ... }}`", kw.as_str()));
            }
            _ => {
                let expr = self.expression()?;
                match self.assignment_op() {
                    Some(op) => {
                        self.bump();
                        let value = self.expression()?;
                        statement(StmtKind::Assign { target: expr, op, value }, self)
                    }
                    None => ArmBody::Expr(expr),
                }
            }
        };
        // An arm ends at a new line or a comma (23.17).
        if !self.eat(&Tok::Comma) && !self.at(&Tok::Newline) && !self.at(&Tok::RBrace) {
            return self.expected("a new line after the arm");
        }
        Ok(body)
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
                self.bump();
                let mut items = Vec::new();
                let mut rest = None;
                while !self.at(&Tok::RBracket) {
                    if self.eat(&Tok::DotDot) {
                        if items.is_empty() {
                            return self.error(
                                "`..` needs at least one pattern before it; `[..rest]` is just `rest` and `[..]` is `_`",
                            );
                        }
                        let mode = if self.eat_kw(Kw::Ref) { Mode::Ref } else { Mode::Value };
                        rest = Some(match self.tok() {
                            Tok::Ident(_) => Some((mode, self.ident()?)),
                            _ if mode == Mode::Ref => return self.expected("a name after `..ref`"),
                            _ => None,
                        });
                        self.eat(&Tok::Comma);
                        break;
                    }
                    items.push(self.pattern()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                if !self.at(&Tok::RBracket) && rest.is_some() {
                    return self.error("`..` must be the last item of an array pattern");
                }
                self.expect(Tok::RBracket)?;
                PatternKind::Array { items, rest }
            }
            Tok::Kw(Kw::Ref | Kw::Mut | Kw::Var) => {
                let mode = self.mode();
                if matches!(self.tok(), Tok::Ident(name) if name == "_") {
                    let message = format!("`{} _` borrows nothing; `_` alone ignores the part", mode.keyword());
                    self.diags.error("E1202", start.to(self.span()), message);
                }
                PatternKind::Loan(mode, self.ident()?)
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
            } else if matches!(self.tok(), Tok::Kw(Kw::Ref | Kw::Mut | Kw::Var)) {
                // `ref field` abbreviates `field: ref field`.
                let start = self.span();
                let mode = self.mode();
                let name = self.field_name()?;
                let kind = PatternKind::Loan(mode, name.clone());
                let pattern = Pattern { kind, span: start.to(name.span) };
                fields.push(FieldPattern { name, pattern: Some(pattern) });
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
        let condition = condition?;
        self.record_in_header(&condition)?;
        let condition = Box::new(condition);
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
            Tok::Kw(Kw::Spawn) => {
                let start = self.bump().span;
                let call = self.postfix()?;
                if let ExprKind::Try(_) = call.kind {
                    return self.error("`spawn` returns a `Join`, not a result: write `?` after `wait()`, as in `(spawn fetch(url)).wait()?`");
                }
                let span = start.to(call.span);
                return Ok(Expr { kind: ExprKind::Spawn(Box::new(call)), span });
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
                Tok::LBracket
                    if matches!(&expr.kind, ExprKind::Path(path)
                        if path.len() == 2 && SYNC_COLLECTIONS.contains(&path[1].name.as_str())) =>
                {
                    // `sync.Map["one": 1]`: a literal of a shared collection.
                    let ExprKind::Path(path) = expr.kind else { unreachable!() };
                    let literal = self.bracket_literal()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::SyncLiteral(path, Box::new(literal)), span };
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
                    // Type arguments follow a path, or a raw operation on any
                    // value: `call().cast[Byte]()`.
                    let raw = matches!(&expr.kind, ExprKind::Field(_, m) if matches!(m.name.as_str(), "cast" | "toPointer" | "toFunction"));
                    if (matches!(expr.kind, ExprKind::Path(_)) || raw)
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
                    expr = Expr { kind: ExprKind::Record { path, type_args: Vec::new(), fields }, span };
                }
                Tok::LBrace
                    if !self.no_brace
                        && matches!(&expr.kind, ExprKind::Generic { base, .. } if matches!(base.kind, ExprKind::Path(_))) =>
                {
                    let ExprKind::Generic { base, args } = expr.kind else { unreachable!() };
                    let ExprKind::Path(path) = base.kind else { unreachable!() };
                    let fields = self.field_inits()?;
                    let span = expr.span.to(self.prev_span());
                    expr = Expr { kind: ExprKind::Record { path, type_args: args, fields }, span };
                }
                _ => return Ok(expr),
            }
        }
    }

    /// `(params): Result { body }` (11.8); `None`, with nothing consumed,
    /// when the parentheses are not a closure's parameter list.
    fn try_closure(&mut self) -> Option<PResult<Expr>> {
        let (pos, errors, recovering) = (self.pos, self.diags.list.len(), self.recovering);
        let start = self.span();
        let header = (|| -> PResult<(Vec<ClosureParam>, Option<TypeExpr>)> {
            self.expect(Tok::LParen)?;
            let mut params = Vec::new();
            loop {
                self.skip_newlines();
                if self.at(&Tok::RParen) {
                    break;
                }
                let name = self.ident()?;
                let (ty, mode, variadic) = if self.eat(&Tok::Colon) {
                    let mode = self.mode();
                    let variadic = mode == Mode::Value && self.eat(&Tok::Ellipsis);
                    (Some(self.type_expr()?), mode, variadic)
                } else {
                    (None, Mode::Value, false)
                };
                params.push(ClosureParam { name, ty, mode, variadic });
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.skip_newlines();
            self.expect(Tok::RParen)?;
            let result = if self.eat(&Tok::Colon) { Some(self.result_type()?) } else { None };
            if !self.at(&Tok::LBrace) {
                return Err(());
            }
            Ok((params, result))
        })();
        let Ok((params, result)) = header else {
            self.pos = pos;
            self.diags.list.truncate(errors);
            self.recovering = recovering;
            return None;
        };
        Some(self.block().map(|body| {
            let span = start.to(body.span);
            Expr { kind: ExprKind::Closure(Box::new(Closure { is_unsafe: false, params, result, body })), span }
        }))
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
            // Type arguments precede a call or a record literal; an index
            // never has several items.
            if !self.at(&Tok::LParen) && !(self.at(&Tok::LBrace) && !self.no_brace) && args.len() < 2 {
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

    /// `self`, `mut self` or `var self` first; otherwise
    /// `name: [ref|mut|var] Type` or `name: ...Type` (11.3, 11.5).
    fn parameter(&mut self, first: bool) -> PResult<Param> {
        let start = self.span();
        let receiver_mode = match (self.tok(), self.nth(1)) {
            (Tok::Kw(Kw::SelfValue), _) => Some(Mode::Ref),
            (Tok::Kw(Kw::Mut), Tok::Kw(Kw::SelfValue)) => Some(Mode::Mut),
            (Tok::Kw(Kw::Var), Tok::Kw(Kw::SelfValue)) => Some(Mode::Var),
            (Tok::Kw(Kw::Ref), Tok::Kw(Kw::SelfValue)) => {
                return self.error("`ref self` is redundant: a plain `self` receiver is already a shared loan");
            }
            _ => None,
        };
        if let Some(mode) = receiver_mode {
            if mode != Mode::Ref {
                self.bump();
            }
            let span = self.bump().span;
            if !first {
                return self.error("the receiver `self` must be the first parameter");
            }
            return Ok(Param {
                name: Ident { name: "self".into(), span },
                ty: None,
                mode,
                variadic: false,
                default: None,
                span: start.to(span),
            });
        }
        // C's `...` ends the parameters of an `extern fn`.
        if self.at(&Tok::Ellipsis) {
            let span = self.bump().span;
            let name = Ident { name: "...".into(), span };
            return Ok(Param { name, ty: None, mode: Mode::Value, variadic: true, default: None, span });
        }
        let name = self.ident()?;
        self.expect(Tok::Colon)?;
        let mode = self.mode();
        let variadic = mode == Mode::Value && self.eat(&Tok::Ellipsis);
        let ty = self.type_expr()?;
        let default = if self.eat(&Tok::Eq) { Some(self.expression()?) } else { None };
        let span = name.span.to(self.prev_span());
        Ok(Param { span, name, ty: Some(ty), mode, variadic, default })
    }

    fn mode(&mut self) -> Mode {
        let mode = match self.tok() {
            Tok::Kw(Kw::Ref) => Mode::Ref,
            Tok::Kw(Kw::Mut) => Mode::Mut,
            Tok::Kw(Kw::Var) => Mode::Var,
            _ => return Mode::Value,
        };
        self.bump();
        mode
    }

    fn call_arguments(&mut self) -> PResult<Vec<Arg>> {
        // Inside the parentheses a block or closure is unambiguous, even in a
        // header such as `match` or `for` (23.17).
        let no_brace = std::mem::replace(&mut self.no_brace, false);
        let args = self.call_arguments_inner();
        self.no_brace = no_brace;
        args
    }

    fn call_arguments_inner(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        loop {
            self.skip_newlines();
            if self.at(&Tok::RParen) {
                break;
            }
            let start = self.span();
            let name = if matches!(self.tok(), Tok::Ident(_)) && self.nth(1) == &Tok::Colon {
                let name = self.ident()?;
                self.bump();
                Some(name)
            } else {
                None
            };
            let mode = self.mode();
            let spread = mode == Mode::Value && self.eat(&Tok::Ellipsis);
            let value = self.expression()?;
            args.push(Arg { span: start.to(value.span), name, value, mode, spread });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.skip_newlines();
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
            Tok::LParen => {
                if !self.no_brace
                    && let Some(closure) = self.try_closure()
                {
                    return closure;
                }
                return self.paren();
            }
            Tok::LBrace if !self.no_brace => {
                let block = self.block()?;
                ExprKind::Block(block)
            }
            Tok::Kw(Kw::If) => return self.if_expression(),
            Tok::LBracket => return self.bracket_literal(),
            Tok::Kw(Kw::SelfValue) => {
                self.bump();
                ExprKind::Path(vec![Ident { name: "self".into(), span: start }])
            }
            Tok::Kw(Kw::Unsafe) => {
                self.bump();
                if self.at(&Tok::LParen) {
                    let Some(closure) = self.try_closure() else {
                        return self.expected("an unsafe closure: `unsafe (parameters) { ... }`");
                    };
                    let mut closure = closure?;
                    if let ExprKind::Closure(c) = &mut closure.kind {
                        c.is_unsafe = true;
                    }
                    closure.span = start.to(closure.span);
                    return Ok(closure);
                }
                ExprKind::Unsafe(self.block()?)
            }
            Tok::Bang => return self.error("`!` is not a prefix operator: write `not value`".to_string()),
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
            let span = start.to(self.prev_span());
            self.parens.push(span);
            return Ok(Expr { kind: first.kind, span });
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
                        parens: Vec::new(),
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
