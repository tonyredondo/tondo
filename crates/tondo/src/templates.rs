//! Templates compiled with the program (standard library 14.31). Before the
//! program is linked, each `template.html(...)` and `template.text(...)`
//! whose text is known when it compiles, a raw string or
//! `embed.text("...")`, is read with the templates it extends and includes
//! and turned into the code that writes it: statements whose expressions
//! are those of its actions, which the linker puts in place of its text and
//! links as any other code, and the checker checks. Each file read joins the
//! sources, so that an error in it points at its line.

use crate::ast::{
    ArmBody, Block, Expr, ExprKind, ForHeader, Ident, MatchArm, Pattern, PatternKind, Stmt, StmtKind, StrSegment,
    UnaryOp,
};
use crate::diag::Diagnostics;
use crate::lexer::{StrPart, Tok};
use crate::modules::ModuleSource;
use crate::source::{SourceMap, Span};
use crate::template_syntax::{self as syntax, Node};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The binding the code of a template writes to, which the checker gives it.
pub const OUT: &str = "__template";

/// Puts the code of each compiled template of the program where the linker
/// takes it, in place of its text.
pub fn compile(modules: &mut [ModuleSource], sources: &mut SourceMap, diags: &mut Diagnostics) {
    let mut files = HashMap::new();
    for module in modules.iter_mut() {
        for (file, _) in &mut module.files {
            let standard = |name: &str| {
                file.imports
                    .iter()
                    .find(|i| matches!(&i.path[..], [s, m] if s.name == "std" && m.name == name))
                    .map(|i| (i.alias_name().to_string(), i.span))
            };
            let Some((alias, span)) = standard("template") else { continue };
            let embed = standard("embed").map(|(alias, _)| alias);
            let source = sources.file_of(span.start);
            let (path, module_name) = (source.path.clone(), source.module.clone());
            for call in calls(sources, span.start, &alias, embed.as_deref()) {
                let mut translator = Translator {
                    sources,
                    diags,
                    files: &mut files,
                    html: call.html,
                    stack: Vec::new(),
                    next: 0,
                    module: module_name.clone(),
                };
                if let Some(block) = translator.call(&path, call.source, call.span) {
                    let code = Expr { kind: ExprKind::Template(Box::new(block)), span: call.span };
                    file.templates.push((call.span.start, code));
                }
            }
        }
    }
}

/// A call of `template.html` or `template.text` whose text is known.
struct Call {
    html: bool,
    source: Source,
    /// Where its text is written: the string, or the call of `embed.text`.
    span: Span,
}

enum Source {
    /// A string, its text if it has no interpolation, and whether it is raw.
    Literal(Option<String>, bool),
    /// The path `embed.text` reads, as written.
    File(String),
}

/// The calls of the file with the offset `offset` whose text is a string or
/// `embed.text("...")`.
fn calls(sources: &SourceMap, offset: u32, alias: &str, embed: Option<&str>) -> Vec<Call> {
    let file = sources.file_of(offset);
    let tokens = crate::lexer::lex(file, None, &mut Diagnostics::default());
    let tok = |at: usize| tokens.get(at).map(|t| &t.tok);
    let mut found = Vec::new();
    for start in 0..tokens.len() {
        let (Some(Tok::Ident(module)), Some(Tok::Dot), Some(Tok::Ident(kind))) =
            (tok(start), tok(start + 1), tok(start + 2))
        else {
            continue;
        };
        if module != alias || !(kind == "html" || kind == "text") {
            continue;
        }
        let mut at = start + 3;
        if tok(at) == Some(&Tok::LBracket) {
            let mut depth = 0;
            while let Some(t) = tok(at) {
                at += 1;
                match t {
                    Tok::LBracket => depth += 1,
                    Tok::RBracket if depth == 1 => break,
                    Tok::RBracket => depth -= 1,
                    _ => {}
                }
            }
        }
        if tok(at) != Some(&Tok::LParen) {
            continue;
        }
        at += 1;
        let html = kind == "html";
        let text = |parts: &[StrPart]| -> Option<String> {
            parts
                .iter()
                .map(|p| match p {
                    StrPart::Text(text) => Some(text.as_str()),
                    StrPart::Expr(..) => None,
                })
                .collect()
        };
        match (tok(at), tok(at + 1), tok(at + 2), tok(at + 3), tok(at + 4), tok(at + 5)) {
            (Some(Tok::Str(parts)), ..) => {
                let span = tokens[at].span;
                let raw = sources.slice(span).starts_with('r');
                found.push(Call { html, source: Source::Literal(text(parts), raw), span });
            }
            (
                Some(Tok::Ident(module)),
                Some(Tok::Dot),
                Some(Tok::Ident(name)),
                Some(Tok::LParen),
                Some(Tok::Str(parts)),
                Some(Tok::RParen),
            ) if Some(module.as_str()) == embed && name == "text" => {
                let Some(written) = text(parts) else { continue };
                let span = tokens[at].span.to(tokens[at + 5].span);
                found.push(Call { html, source: Source::File(written), span });
            }
            _ => {}
        }
    }
    found
}

/// A template among the sources: its file, and where its text starts in it.
#[derive(Clone, Copy)]
struct Loaded {
    file: usize,
    start: usize,
}

/// Turns templates into code.
struct Translator<'a> {
    sources: &'a mut SourceMap,
    diags: &'a mut Diagnostics,
    /// The templates read, by their paths.
    files: &'a mut HashMap<PathBuf, Loaded>,
    html: bool,
    /// The templates being written, the outermost first.
    stack: Vec<usize>,
    /// Numbers the bindings the code needs of its own.
    next: usize,
    module: String,
}

impl Translator<'_> {
    fn call(&mut self, tondo: &Path, source: Source, span: Span) -> Option<Block> {
        let loaded = match source {
            Source::Literal(None, _) => {
                let message = "a compiled template is its text as it is: write it as a raw string, `r\"...\"`, without `{...}` of the string";
                self.diags.error("E1906", span, message);
                return None;
            }
            Source::Literal(Some(_), false) if self.sources.slice(span).contains("{{") => {
                let message = "write the template as a raw string, `r\"...\"`: in a string, `{{` writes one brace";
                self.diags.error("E1906", span, message);
                return None;
            }
            Source::Literal(Some(text), _) => {
                // The template's lines at those of the file, for its errors.
                let (line, column) = self.sources.file_of(span.start).line_col(span.start);
                let below = self.sources.slice(span).trim_start_matches('r').starts_with("\"\"\"");
                let padding = match below {
                    true => "\n".repeat(line),
                    false => format!("{}{}", "\n".repeat(line - 1), " ".repeat(column + 1)),
                };
                let file = self.sources.add(tondo, format!("{padding}{text}"));
                let identity = format!("<template>/{}:{}", tondo.display(), span.start);
                self.sources.set_identity(file, identity, self.module.clone());
                Loaded { file, start: padding.len() }
            }
            // `embed.text` says why a path is wrong.
            Source::File(written) => self.load(&crate::embed::resolve(tondo, &written).ok()?, span)?,
        };
        self.template(loaded, span)
    }

    /// The template at `path`, read once.
    fn load(&mut self, path: &Path, span: Span) -> Option<Loaded> {
        if let Some(&loaded) = self.files.get(path) {
            return Some(loaded);
        }
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                self.diags.error("E1906", span, format!("cannot read the template `{}`: {e}", path.display()));
                return None;
            }
        };
        let file = self.sources.add(path, text);
        self.sources.set_identity(file, format!("<template>/{}", path.display()), self.module.clone());
        let loaded = Loaded { file, start: 0 };
        self.files.insert(path.to_path_buf(), loaded);
        Some(loaded)
    }

    /// Where `range` of a template's text is among the sources.
    fn span(&self, loaded: Loaded, range: &Range<usize>) -> Span {
        let base = self.sources.file(loaded.file).base + loaded.start as u32;
        Span::new(base + range.start as u32, base + range.end.max(range.start + 1) as u32)
    }

    fn text(&self, loaded: Loaded, range: &Range<usize>) -> String {
        self.sources.file(loaded.file).text[loaded.start..][range.clone()].to_string()
    }

    fn read(&mut self, loaded: Loaded) -> Option<syntax::Template> {
        let text = &self.sources.file(loaded.file).text[loaded.start..];
        match syntax::parse(text, self.html) {
            Ok(template) => Some(template),
            Err(error) => {
                let span = self.span(loaded, &(error.at..error.at));
                self.diags.error("E1906", span, error.message);
                None
            }
        }
    }

    /// The template named `name` in one of `loaded`'s directory.
    fn named(&mut self, loaded: Loaded, name: &str, at: usize) -> Option<Loaded> {
        let span = self.span(loaded, &(at..at));
        let path = self.sources.file(loaded.file).path.clone();
        match crate::embed::resolve(&path, name) {
            Ok(found) => self.load(&found, span),
            Err(why) => {
                self.diags.error("E1906", span, why);
                None
            }
        }
    }

    /// The code that writes a template: that of the template it extends,
    /// if any, with the blocks it redefines.
    fn template(&mut self, loaded: Loaded, span: Span) -> Option<Block> {
        if self.stack.contains(&loaded.file) {
            let message = "this template includes itself, so writing it would never end";
            self.diags.error("E1906", span, message);
            return None;
        }
        // The template and those it extends, the most derived first.
        let mut chain = vec![(loaded, self.read(loaded)?)];
        while let Some((name, at)) = chain.last().and_then(|(_, t)| t.extends.clone()) {
            let (from, _) = *chain.last()?;
            if syntax::computed(&name) {
                let message = "a compiled template reads the template it extends when it compiles, so it needs its name then: write one in quotes with no values in it, or load the templates into a `template.Set`, which computes the name as it writes";
                self.diags.error("E1906", self.span(from, &(at..at)), message);
                return None;
            }
            let base = self.named(from, &name, at)?;
            if chain.iter().any(|(l, _)| l.file == base.file) {
                self.diags.error("E1906", self.span(from, &(at..at)), "these templates extend each other in a circle");
                return None;
            }
            chain.push((base, self.read(base)?));
        }
        let mut blocks: HashMap<String, (Loaded, Vec<Node>)> = HashMap::new();
        for (loaded, template) in &chain[..chain.len() - 1] {
            for node in &template.nodes {
                if let Node::Block { name, body, .. } = node {
                    blocks.entry(name.clone()).or_insert((*loaded, body.clone()));
                }
            }
        }
        let (base, template) = chain.pop()?;
        self.stack.push(loaded.file);
        let stmts = self.nodes(base, &template.nodes, &blocks);
        self.stack.pop();
        Some(Block { stmts: stmts?, tail: None, span })
    }

    fn nodes(
        &mut self,
        loaded: Loaded,
        nodes: &[Node],
        blocks: &HashMap<String, (Loaded, Vec<Node>)>,
    ) -> Option<Vec<Stmt>> {
        let mut stmts = Vec::new();
        let mut failed = false;
        for node in nodes {
            match self.node(loaded, node, blocks) {
                Some(mut more) => stmts.append(&mut more),
                None => failed = true,
            }
        }
        (!failed).then_some(stmts)
    }

    fn block(
        &mut self,
        loaded: Loaded,
        nodes: &[Node],
        blocks: &HashMap<String, (Loaded, Vec<Node>)>,
        span: Span,
    ) -> Option<Block> {
        Some(Block { stmts: self.nodes(loaded, nodes, blocks)?, tail: None, span })
    }

    fn expression(&mut self, loaded: Loaded, range: &Range<usize>) -> Option<Expr> {
        let span = self.span(loaded, range);
        crate::parser::parse_expression(self.sources.file(loaded.file), span, self.diags)
    }

    fn node(
        &mut self,
        loaded: Loaded,
        node: &Node,
        blocks: &HashMap<String, (Loaded, Vec<Node>)>,
    ) -> Option<Vec<Stmt>> {
        let place = |at: usize| self.span(loaded, &(at..at));
        match node {
            Node::Text(text) => {
                let span = place(0);
                Some(vec![append(StrSegment::Text(text.clone()), span)])
            }
            Node::Write { value, format, place: written } => {
                let span = self.span(loaded, value);
                let value = self.expression(loaded, value)?;
                let format = match format {
                    Some(range) => {
                        Some(crate::ast::parse_format(&self.text(loaded, range), self.span(loaded, range)).ok()?)
                    }
                    None => None,
                };
                let place = self.html.then_some(*written as u8);
                let write = ExprKind::TemplateWrite { value: Box::new(value), format, place };
                Some(vec![append(StrSegment::Expr(Expr { kind: write, span }, None), span)])
            }
            Node::If { at, branches, otherwise } => {
                let span = place(*at);
                let mut tail = match otherwise {
                    Some(nodes) => Some(Box::new(block_expr(self.block(loaded, nodes, blocks, span)?))),
                    None => None,
                };
                for (condition, body) in branches.iter().rev() {
                    let condition = Box::new(self.expression(loaded, condition)?);
                    let then = self.block(loaded, body, blocks, span)?;
                    tail = Some(Box::new(Expr { kind: ExprKind::If { condition, then, otherwise: tail }, span }));
                }
                Some(vec![Stmt { kind: StmtKind::Expr(*tail?), span }])
            }
            Node::For { at, pattern, items, body, otherwise } => {
                let span = place(*at);
                let pattern = crate::parser::parse_pattern(
                    self.sources.file(loaded.file),
                    self.span(loaded, pattern),
                    self.diags,
                )?;
                let source = self.expression(loaded, items)?;
                let mut body = self.block(loaded, body, blocks, span)?;
                let Some(otherwise) = otherwise else {
                    return Some(vec![Stmt {
                        kind: StmtKind::For { header: ForHeader::In { pattern, source }, body },
                        span,
                    }]);
                };
                // Whether the loop went round, for its `else`.
                self.next += 1;
                let ran = Ident { name: format!("__ran{}", self.next), span };
                let path = || Expr { kind: ExprKind::Path(vec![ran.clone()]), span };
                let flag = |value: bool| Expr { kind: ExprKind::Bool(value), span };
                let pattern_ran = Pattern { kind: PatternKind::Binding(ran.clone()), span };
                let declare = StmtKind::Let { mutable: true, pattern: pattern_ran, ty: None, value: Some(flag(false)) };
                body.stmts
                    .insert(0, Stmt { kind: StmtKind::Assign { target: path(), op: None, value: flag(true) }, span });
                let condition = Box::new(Expr { kind: ExprKind::Unary(UnaryOp::Not, Box::new(path())), span });
                let then = self.block(loaded, otherwise, blocks, span)?;
                Some(vec![
                    Stmt { kind: declare, span },
                    Stmt { kind: StmtKind::For { header: ForHeader::In { pattern, source }, body }, span },
                    Stmt {
                        kind: StmtKind::Expr(Expr { kind: ExprKind::If { condition, then, otherwise: None }, span }),
                        span,
                    },
                ])
            }
            Node::With { at, name, value, body, otherwise } => {
                let span = place(*at);
                let name = Ident { name: self.text(loaded, name), span: self.span(loaded, name) };
                let scrutinee = Box::new(self.expression(loaded, value)?);
                let body = self.block(loaded, body, blocks, span)?;
                let otherwise = self.block(loaded, otherwise.as_deref().unwrap_or_default(), blocks, span)?;
                let pattern = |kind: PatternKind| Pattern { kind, span };
                let some = PatternKind::Constructor {
                    path: vec![Ident { name: "some".into(), span }],
                    args: vec![pattern(PatternKind::Binding(name))],
                };
                let arm = |kind, body| MatchArm {
                    pattern: pattern(kind),
                    guard: None,
                    body: ArmBody::Expr(block_expr(body)),
                    span,
                };
                let arms = vec![arm(some, body), arm(PatternKind::None, otherwise)];
                Some(vec![Stmt {
                    kind: StmtKind::Expr(Expr { kind: ExprKind::Match { scrutinee, arms }, span }),
                    span,
                }])
            }
            Node::Let { name, value } => {
                let span = self.span(loaded, name);
                let binding =
                    Pattern { kind: PatternKind::Binding(Ident { name: self.text(loaded, name), span }), span };
                let value = self.expression(loaded, value)?;
                Some(vec![Stmt {
                    kind: StmtKind::Let { mutable: false, pattern: binding, ty: None, value: Some(value) },
                    span,
                }])
            }
            Node::Block { at, name, body } => {
                let span = place(*at);
                let (from, body) = match blocks.get(name) {
                    Some((from, body)) => (*from, body.clone()),
                    None => (loaded, body.clone()),
                };
                let block = self.block(from, &body, blocks, span)?;
                Some(vec![Stmt { kind: StmtKind::Expr(block_expr(block)), span }])
            }
            Node::Include { at, name, data, named } => {
                let span = place(*at);
                let syntax::Included::Named(name) = name else {
                    let message = "a compiled template reads the templates it includes when it compiles, so it needs their names then: write one in quotes with no values in it, or load the templates into a `template.Set`, which computes the name as it writes";
                    self.diags.error("E1906", span, message);
                    return None;
                };
                let included = self.named(loaded, name, *at)?;
                let data = match data {
                    Some(range) => Some(Box::new(self.expression(loaded, range)?)),
                    None => None,
                };
                let mut values = Vec::new();
                for (key, range) in named {
                    let name = Ident { name: key.clone(), span: self.span(loaded, range) };
                    values.push((name, self.expression(loaded, range)?));
                }
                let body = Box::new(self.template(included, span)?);
                let include = Expr { kind: ExprKind::TemplateInclude { data, named: values, body }, span };
                Some(vec![append(StrSegment::Expr(include, None), span)])
            }
            Node::Break => Some(vec![Stmt { kind: StmtKind::Break, span: place(0) }]),
            Node::Continue => Some(vec![Stmt { kind: StmtKind::Continue, span: place(0) }]),
        }
    }
}

/// `__template = "{__template}..."`: a piece written after the others.
fn append(piece: StrSegment, span: Span) -> Stmt {
    let out = || Expr { kind: ExprKind::Path(vec![Ident { name: OUT.into(), span }]), span };
    let value = Expr { kind: ExprKind::Str(vec![StrSegment::Expr(out(), None), piece]), span };
    Stmt { kind: StmtKind::Assign { target: out(), op: None, value }, span }
}

fn block_expr(block: Block) -> Expr {
    let span = block.span;
    Expr { kind: ExprKind::Block(block), span }
}

/// The templates a source file's compiled templates and its embedded texts
/// extend and include, which a build reads with it.
pub fn inputs(path: &Path, texts: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = texts.to_vec();
    pending.push(path.to_path_buf());
    while let Some(next) = pending.pop() {
        let Ok(text) = std::fs::read_to_string(&next) else { continue };
        for name in syntax::references(&text) {
            if let Ok(target) = crate::embed::resolve(&next, &name)
                && !found.contains(&target)
            {
                found.push(target.clone());
                pending.push(target);
            }
        }
    }
    found
}
