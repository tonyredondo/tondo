//! The canonical formatter, `tondo-format-0.1` (21).
//!
//! The syntax tree becomes a document of text, line breaks, indentation and
//! groups; a group stays on one line when it fits in 100 columns. Literals
//! keep their spelling, parentheses stay where they were written, and
//! comments are placed back before or after the unit they belong to.

use crate::ast::*;
use crate::diag::Diagnostics;
use crate::source::{SourceFile, Span};
use crate::{lexer, parser};

const WIDTH: usize = 100;

/// Formats a file, or reports its syntax errors in `diags`.
pub fn format(file: &SourceFile, diags: &mut Diagnostics) -> Option<String> {
    let (module, parens) = parser::parse_with_parens(file, diags);
    if diags.has_errors() {
        return None;
    }
    let (_, comments) = lexer::lex_with_comments(file, None, &mut Diagnostics::default());
    let mut formatter = Formatter { file, comments, next: 0, parens };
    let doc = formatter.module(&module);
    let mut text = render(&doc);
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Some(text)
}

// ---- documents ---------------------------------------------------------

enum Doc {
    Text(String),
    /// Always a line break.
    Hardline,
    /// A space on one line, a line break otherwise.
    Softline,
    /// Nothing on one line, a line break otherwise.
    Softzero,
    /// Forces every enclosing group onto several lines: a line comment.
    Break,
    Indent(Box<Doc>),
    Group(Box<Doc>),
    /// What to emit when the enclosing group is broken, or else on one line.
    IfBreak(Box<Doc>, Box<Doc>),
    Concat(Vec<Doc>),
}

fn text(s: impl Into<String>) -> Doc {
    Doc::Text(s.into())
}

fn concat(docs: Vec<Doc>) -> Doc {
    Doc::Concat(docs)
}

fn group(doc: Doc) -> Doc {
    Doc::Group(Box::new(doc))
}

fn indent(doc: Doc) -> Doc {
    Doc::Indent(Box::new(doc))
}

fn if_break(broken: Doc, flat: Doc) -> Doc {
    Doc::IfBreak(Box::new(broken), Box::new(flat))
}

/// Writes the document: groups are decided from the outside in, each
/// staying flat when its flat form fits from the current column.
fn render(doc: &Doc) -> String {
    let mut out = String::new();
    let mut column = 0;
    let mut pending: Option<usize> = Some(0);
    let mut stack: Vec<(usize, bool, &Doc)> = vec![(0, false, doc)];
    let write = |s: &str, out: &mut String, column: &mut usize, pending: &mut Option<usize>| {
        if s.is_empty() {
            return;
        }
        if let Some(spaces) = pending.take() {
            out.extend(std::iter::repeat_n(' ', spaces));
            *column = spaces;
        }
        out.push_str(s);
        *column = match s.rfind('\n') {
            Some(at) => s[at + 1..].chars().count(),
            None => *column + s.chars().count(),
        };
    };
    while let Some((level, flat, doc)) = stack.pop() {
        match doc {
            Doc::Text(s) => write(s, &mut out, &mut column, &mut pending),
            Doc::Softline if flat => write(" ", &mut out, &mut column, &mut pending),
            Doc::Softline | Doc::Softzero if flat => {}
            Doc::Hardline | Doc::Softline | Doc::Softzero => {
                out.push('\n');
                column = 0;
                pending = Some(level);
            }
            Doc::Break => {}
            Doc::Indent(inner) => stack.push((level + 4, flat, inner)),
            Doc::Group(inner) => {
                let start = pending.unwrap_or(column);
                stack.push((level, flat || fits(inner, start), inner));
            }
            Doc::IfBreak(broken, plain) => stack.push((level, flat, if flat { plain } else { broken })),
            Doc::Concat(docs) => stack.extend(docs.iter().rev().map(|d| (level, flat, d))),
        }
    }
    out
}

/// Whether `doc` fits on one line from `column`.
fn fits(doc: &Doc, column: usize) -> bool {
    let mut width = column;
    let mut stack = vec![doc];
    while let Some(doc) = stack.pop() {
        match doc {
            Doc::Text(s) if s.contains('\n') => return false,
            Doc::Text(s) => width += s.chars().count(),
            Doc::Hardline | Doc::Break => return false,
            Doc::Softline => width += 1,
            Doc::Softzero => {}
            Doc::Indent(inner) | Doc::Group(inner) => stack.push(inner),
            Doc::IfBreak(_, plain) => stack.push(plain),
            Doc::Concat(docs) => stack.extend(docs.iter().rev()),
        }
        if width > WIDTH {
            return false;
        }
    }
    true
}

// ---- the tree ----------------------------------------------------------

struct Formatter<'a> {
    file: &'a SourceFile,
    comments: Vec<Span>,
    /// The next comment not emitted yet.
    next: usize,
    /// Expressions written in parentheses.
    parens: Vec<Span>,
}

/// A top-level unit in source order.
enum Unit<'m> {
    Item(&'m Item),
    Stmt(&'m Stmt),
}

impl Formatter<'_> {
    fn source(&self, span: Span) -> &str {
        self.file.slice(span)
    }

    /// Whether the source has an empty line between two offsets.
    fn blank_between(&self, start: u32, end: u32) -> bool {
        if start >= end {
            return false;
        }
        let lines: Vec<&str> = self.source(Span::new(start, end)).split('\n').collect();
        lines.len() > 2 && lines[1..lines.len() - 1].iter().any(|l| l.trim().is_empty())
    }

    /// The comments before `pos`, each on its own line, keeping one empty
    /// line where the source had any.
    fn leading(&mut self, pos: u32) -> Doc {
        let mut docs = Vec::new();
        while let Some(&comment) = self.comments.get(self.next) {
            if comment.end > pos {
                break;
            }
            self.next += 1;
            let body = self.source(comment).to_string();
            let inline_block = body.starts_with("/*") && !body.contains('\n') && !self.ends_line(comment.end);
            if inline_block {
                docs.push(text(format!("{body} ")));
                continue;
            }
            docs.push(text(body));
            docs.push(Doc::Hardline);
            let following = self.comments.get(self.next).map_or(pos, |c| c.start.min(pos));
            if self.blank_between(comment.end, following) {
                docs.push(Doc::Hardline);
            }
        }
        concat(docs)
    }

    /// Whether only whitespace follows `offset` on its line.
    fn ends_line(&self, offset: u32) -> bool {
        let rest = &self.file.text[(offset - self.file.base) as usize..];
        rest.split('\n').next().is_none_or(|line| line.trim().is_empty())
    }

    /// A comment on the same line after `end`, emitted after the unit.
    fn trailing(&mut self, end: u32) -> Doc {
        let Some(&comment) = self.comments.get(self.next) else { return concat(Vec::new()) };
        if comment.start < end || self.source(Span::new(end, comment.start)).contains('\n') {
            return concat(Vec::new());
        }
        self.next += 1;
        let body = self.source(comment).to_string();
        if body.starts_with("//") {
            concat(vec![text(format!("  {body}")), Doc::Break])
        } else {
            text(format!(" {body}"))
        }
    }

    fn module(&mut self, module: &Module) -> Doc {
        let mut docs = Vec::new();
        if self.file.text.starts_with("#!") {
            let line = self.file.text.lines().next().unwrap_or_default().trim_end().to_string();
            docs.push(text(line));
            docs.push(Doc::Hardline);
            if !module.imports.is_empty() || !module.items.is_empty() || !module.statements.is_empty() {
                docs.push(Doc::Hardline);
            }
        }
        if !module.imports.is_empty() {
            docs.push(self.imports(&module.imports));
            docs.push(Doc::Hardline);
        }
        let mut units: Vec<(Span, Unit)> = module
            .items
            .iter()
            .map(|i| (item_span(i), Unit::Item(i)))
            .chain(module.statements.iter().map(|s| (s.span, Unit::Stmt(s))))
            .collect();
        units.sort_by_key(|(span, _)| span.start);
        let mut previous: Option<(u32, bool)> = None;
        for (span, unit) in units {
            let declaration = matches!(unit, Unit::Item(_));
            if let Some((end, was_declaration)) = previous {
                docs.push(Doc::Hardline);
                if declaration || was_declaration || self.blank_between(end, self.first_comment_or(span.start)) {
                    docs.push(Doc::Hardline);
                }
            }
            docs.push(self.leading(span.start));
            docs.push(match unit {
                Unit::Item(item) => self.item(item),
                Unit::Stmt(stmt) => self.stmt(stmt),
            });
            docs.push(self.trailing(span.end));
            previous = Some((span.end, declaration));
        }
        let end = self.file.base + self.file.text.len() as u32;
        let rest = self.leading(end);
        if !matches!(&rest, Doc::Concat(d) if d.is_empty()) {
            if previous.is_some() {
                docs.push(Doc::Hardline);
                docs.push(Doc::Hardline);
            }
            docs.push(rest);
        }
        docs.push(Doc::Hardline);
        concat(docs)
    }

    /// The start of the first comment before `pos`, or `pos`.
    fn first_comment_or(&self, pos: u32) -> u32 {
        self.comments.get(self.next).map_or(pos, |c| c.start.min(pos))
    }

    /// Imports in groups separated by empty lines, each sorted by path and
    /// alias (21.4).
    fn imports(&mut self, imports: &[Import]) -> Doc {
        let mut groups: Vec<Vec<&Import>> = vec![Vec::new()];
        for (index, import) in imports.iter().enumerate() {
            if index > 0 && self.blank_between(imports[index - 1].span.end, import.span.start) {
                groups.push(Vec::new());
            }
            groups.last_mut().unwrap().push(import);
        }
        let mut docs = Vec::new();
        for (index, group) in groups.iter_mut().enumerate() {
            group.sort_by(|a, b| {
                let key = |i: &Import| (path_text(&i.path), i.alias.as_ref().map(|a| a.name.clone()));
                key(a).cmp(&key(b))
            });
            if index > 0 {
                docs.push(Doc::Hardline);
                docs.push(Doc::Hardline);
            }
            for (position, import) in group.iter().enumerate() {
                if position > 0 {
                    docs.push(Doc::Hardline);
                }
                docs.push(self.leading(import.span.start));
                let alias = import.alias.as_ref().map_or(String::new(), |a| format!(" as {}", a.name));
                docs.push(text(format!("import {}{alias}", path_text(&import.path))));
                docs.push(self.trailing(import.span.end));
            }
        }
        docs.push(Doc::Hardline);
        concat(docs)
    }

    // ---- declarations ----------------------------------------------------

    fn item(&mut self, item: &Item) -> Doc {
        match item {
            Item::Fn(f) => concat(vec![annotations(&f.annotations), self.function(f)]),
            Item::Type(t) => {
                let head = concat(vec![
                    annotations(&t.annotations),
                    text(format!(
                        "{}{}{} {}",
                        if t.public { "pub " } else { "" },
                        if t.foreign { "extern " } else { "" },
                        if t.context { "context" } else { "type" },
                        t.name.name
                    )),
                    self.generics(&t.generics),
                ]);
                match &t.body {
                    TypeBody::Opaque => head,
                    TypeBody::Newtype(inner) => concat(vec![head, text(" = "), self.ty(inner)]),
                    TypeBody::Record(fields) => {
                        let end = |f: &FieldDecl| f.default.as_ref().map_or(f.ty.span.end, |d| d.span.end);
                        let fields = fields.iter().map(|f| (f.name.span.start, end(f), self.field_decl(f))).collect();
                        concat(vec![head, text(" = "), self.braced_lines(fields, t.span.end)])
                    }
                }
            }
            Item::Const(c) => {
                let mut parts = vec![text(format!("{}const {}", if c.public { "pub " } else { "" }, c.name.name))];
                if let Some(ty) = &c.ty {
                    parts.push(text(": "));
                    parts.push(self.ty(ty));
                }
                parts.push(text(" = "));
                parts.push(self.expr(&c.value));
                concat(parts)
            }
            Item::Alias(a) => concat(vec![
                text(format!("{}alias {} = ", if a.public { "pub " } else { "" }, a.name.name)),
                self.ty(&a.target),
            ]),
            Item::Enum(e) => {
                let head = concat(vec![
                    text(format!("{}enum {}", if e.public { "pub " } else { "" }, e.name.name)),
                    self.generics(&e.generics),
                    text(" "),
                ]);
                let variants = e
                    .variants
                    .iter()
                    .map(|v| {
                        let end = match &v.payload {
                            VariantPayload::Unit => v.name.span.end,
                            VariantPayload::Tuple(types) => types.last().map_or(v.name.span.end, |t| t.span.end),
                            VariantPayload::Record(fields) => fields.last().map_or(v.name.span.end, |f| f.ty.span.end),
                        };
                        // The variant's own comments come first, or its
                        // payload would take them.
                        let leading = self.leading(v.name.span.start);
                        (v.name.span.start, end, concat(vec![leading, self.variant(v)]))
                    })
                    .collect();
                concat(vec![head, self.braced_lines(variants, e.span.end)])
            }
            Item::Trait(t) => {
                let head = concat(vec![
                    text(format!("{}trait {}", if t.public { "pub " } else { "" }, t.name.name)),
                    self.generics(&t.generics),
                    text(" "),
                ]);
                concat(vec![head, self.members(&t.methods, t.span.end)])
            }
            Item::Impl(i) => {
                let head = concat(vec![
                    text("impl"),
                    self.generics(&i.generics),
                    text(" "),
                    self.ty(&i.trait_ref),
                    text(" for "),
                    self.ty(&i.target),
                    text(" "),
                ]);
                concat(vec![head, self.members(&i.methods, i.span.end)])
            }
            Item::Test(node) => self.test_node(node),
            Item::Derive(d) => {
                let mut parts = vec![text("derive"), self.generics(&d.generics), text(" ")];
                for (index, t) in d.traits.iter().enumerate() {
                    if index > 0 {
                        parts.push(text(" + "));
                    }
                    parts.push(self.ty(t));
                }
                parts.push(text(" for "));
                parts.push(self.ty(&d.target));
                concat(parts)
            }
        }
    }

    /// `test name { ... }`, or a suite: its setup, then each member after
    /// an empty line (testing 3.4).
    fn test_node(&mut self, node: &TestNode) -> Doc {
        let suite = match node {
            TestNode::Test(t) => return concat(vec![text(format!("test {} ", t.name.name)), self.block(&t.body)]),
            TestNode::Suite(s) => s,
        };
        let head = text(format!("suite {} ", suite.name.name));
        let has_comments = self.comments.get(self.next).is_some_and(|c| c.start < suite.span.end);
        if suite.setup.is_empty() && suite.members.is_empty() && !has_comments {
            return concat(vec![head, text("{}")]);
        }
        let mut body = Vec::new();
        let mut previous: Option<u32> = None;
        for stmt in &suite.setup {
            body.push(Doc::Hardline);
            if let Some(end) = previous
                && self.blank_between(end, self.first_comment_or(stmt.span.start))
            {
                body.push(Doc::Hardline);
            }
            body.push(self.leading(stmt.span.start));
            body.push(self.stmt(stmt));
            body.push(self.trailing(stmt.span.end));
            previous = Some(stmt.span.end);
        }
        for member in &suite.members {
            body.push(Doc::Hardline);
            if previous.is_some() {
                body.push(Doc::Hardline);
            }
            let span = member.span();
            body.push(self.leading(span.start));
            body.push(self.test_node(member));
            body.push(self.trailing(span.end));
            previous = Some(span.end);
        }
        body.push(self.leading_in_block(suite.span.end));
        concat(vec![head, text("{"), indent(concat(body)), Doc::Hardline, text("}")])
    }

    /// `{` with one unit per line, then `}`; never on one line.
    fn braced_lines(&mut self, units: Vec<(u32, u32, Doc)>, end: u32) -> Doc {
        if units.is_empty() {
            return text("{}");
        }
        let mut body = Vec::new();
        for (start, unit_end, doc) in units {
            body.push(Doc::Hardline);
            body.push(self.leading(start));
            body.push(doc);
            body.push(self.trailing(unit_end));
        }
        body.push(self.leading_in_block(end));
        concat(vec![text("{"), indent(concat(body)), Doc::Hardline, text("}")])
    }

    /// Comments before the closing brace at `end`, inside the block.
    fn leading_in_block(&mut self, end: u32) -> Doc {
        let rest = self.leading(end.saturating_sub(1));
        match rest {
            Doc::Concat(docs) if docs.is_empty() => concat(Vec::new()),
            rest => concat(vec![Doc::Hardline, strip_last_hardline(rest)]),
        }
    }

    /// The methods of a trait or an implementation, one empty line apart.
    fn members(&mut self, methods: &[FnDecl], end: u32) -> Doc {
        if methods.is_empty() {
            return text("{}");
        }
        let mut body = Vec::new();
        for (index, method) in methods.iter().enumerate() {
            body.push(Doc::Hardline);
            if index > 0 {
                body.push(Doc::Hardline);
            }
            body.push(self.leading(method.span.start));
            body.push(self.function(method));
            body.push(self.trailing(method.span.end));
        }
        body.push(self.leading_in_block(end));
        concat(vec![text("{"), indent(concat(body)), Doc::Hardline, text("}")])
    }

    fn field_decl(&mut self, field: &FieldDecl) -> Doc {
        let private = if field.private { "priv " } else { "" };
        let mut parts =
            vec![annotations(&field.annotations), text(format!("{private}{}: ", field.name.name)), self.ty(&field.ty)];
        if let Some(default) = &field.default {
            parts.push(text(" = "));
            parts.push(self.expr(default));
        }
        concat(parts)
    }

    fn variant(&mut self, variant: &VariantDecl) -> Doc {
        let name = text(variant.name.name.clone());
        match &variant.payload {
            VariantPayload::Unit => name,
            VariantPayload::Tuple(types) => {
                let items = types.iter().map(|t| (t.span, self.ty(t))).collect();
                concat(vec![name, self.list("(", items, ")")])
            }
            VariantPayload::Record(fields) => {
                let fields =
                    fields.iter().map(|f| (Span::new(f.name.span.start, f.ty.span.end), self.field_decl(f))).collect();
                concat(vec![name, text(" "), self.record_body(fields)])
            }
        }
    }

    fn generics(&mut self, generics: &[GenericParam]) -> Doc {
        if generics.is_empty() {
            return concat(Vec::new());
        }
        let items = generics
            .iter()
            .map(|g| {
                let end = g.bounds.last().map_or(g.name.span.end, |b| b.span.end);
                let doc = if g.bounds.is_empty() {
                    text(g.name.name.clone())
                } else {
                    concat(vec![text(format!("{}: ", g.name.name)), self.bounds(&g.bounds)])
                };
                (Span::new(g.name.span.start, end), doc)
            })
            .collect();
        self.list("[", items, "]")
    }

    fn bounds(&mut self, bounds: &[TypeExpr]) -> Doc {
        let mut docs = vec![self.ty(&bounds[0])];
        for bound in &bounds[1..] {
            docs.push(text(" +"));
            docs.push(indent(concat(vec![Doc::Softline, self.ty(bound)])));
        }
        group(concat(docs))
    }

    fn function(&mut self, f: &FnDecl) -> Doc {
        let mut head = vec![text(format!(
            "{}{}{}fn ",
            if f.public { "pub " } else { "" },
            if f.is_unsafe { "unsafe " } else { "" },
            if f.foreign.is_some() { "extern " } else { "" }
        ))];
        if let Some(owner) = &f.owner {
            head.push(text(owner.name.clone()));
            head.push(self.generics(&f.owner_generics));
            head.push(text("."));
        }
        head.push(text(f.name.name.clone()));
        head.push(self.generics(&f.generics));
        let mut params: Vec<(Span, Doc)> = f.params.iter().map(|p| (p.span, self.param(p))).collect();
        if f.foreign.as_ref().is_some_and(|f| f.variadic) {
            let end = f.params.last().map_or(f.name.span, |p| p.span);
            params.push((Span::new(end.end, end.end), text("...")));
        }
        head.push(self.list("(", params, ")"));
        if let Some(result) = &f.result
            && !is_unit(result)
        {
            head.push(text(": "));
            head.push(self.result(result));
        }
        match &f.body {
            Some(body) => concat(vec![group(concat(head)), text(" "), self.block(body)]),
            None => group(concat(head)),
        }
    }

    fn param(&mut self, param: &Param) -> Doc {
        let Some(ty) = &param.ty else {
            return text(match param.mode {
                Mode::Mut => "mut self",
                Mode::Var => "var self",
                _ => "self",
            });
        };
        let mode = match param.mode {
            Mode::Value => String::new(),
            mode => format!("{} ", mode.keyword()),
        };
        let variadic = if param.variadic { "..." } else { "" };
        let mut parts = vec![text(format!("{}: {mode}{variadic}", param.name.name)), self.ty(ty)];
        if let Some(default) = &param.default {
            parts.push(text(" = "));
            parts.push(self.expr(default));
        }
        concat(parts)
    }

    // ---- types -----------------------------------------------------------

    /// A function or closure result: `T`, `T ! E` or `!E`.
    fn result(&mut self, ty: &TypeExpr) -> Doc {
        self.ty(ty)
    }

    fn ty(&mut self, ty: &TypeExpr) -> Doc {
        match &ty.kind {
            TypeKind::Path { segments, args } => {
                let name = path_text(segments);
                match (name.as_str(), args.as_slice()) {
                    ("Option", [inner]) => self.optional(inner),
                    ("Result", [ok, err]) => self.result_of(Some(ok), err),
                    _ if args.is_empty() => text(name),
                    _ => {
                        let items = args.iter().map(|a| (a.span, self.ty(a))).collect();
                        concat(vec![text(name), self.list("[", items, "]")])
                    }
                }
            }
            TypeKind::Optional(inner) => self.optional(inner),
            TypeKind::Result { ok, err } => self.result_of(ok.as_deref(), err),
            TypeKind::Union(items) => {
                let mut docs = vec![self.ty(&items[0])];
                for item in &items[1..] {
                    docs.push(text(" |"));
                    docs.push(indent(concat(vec![Doc::Softline, self.ty(item)])));
                }
                group(concat(docs))
            }
            TypeKind::Tuple(items) => {
                let items = items.iter().map(|t| (t.span, self.ty(t))).collect();
                self.list("(", items, ")")
            }
            TypeKind::Opaque(bounds) => concat(vec![text("impl "), self.bounds(bounds)]),
            TypeKind::Fn { params, modes, variadic, result, is_unsafe, is_extern } => {
                let count = params.len();
                // C's `...` has a mode but no type.
                let c_variadic = modes.len() > params.len();
                let mut items: Vec<(Span, Doc)> = params
                    .iter()
                    .zip(modes)
                    .enumerate()
                    .map(|(index, (p, mode))| {
                        let mode = match mode {
                            Mode::Value => String::new(),
                            mode => format!("{} ", mode.keyword()),
                        };
                        let dots = if *variadic && !c_variadic && index + 1 == count { "..." } else { "" };
                        (p.span, concat(vec![text(format!("{mode}{dots}")), self.ty(p)]))
                    })
                    .collect();
                if c_variadic {
                    let end = params.last().map_or(ty.span.start, |p| p.span.end);
                    items.push((Span::new(end, end), text("...")));
                }
                let keyword = match (is_unsafe, is_extern) {
                    (true, _) => "unsafe fn",
                    (_, true) => "extern fn",
                    _ => "fn",
                };
                let mut docs = vec![text(keyword), self.list("(", items, ")")];
                if let Some(result) = result
                    && !is_unit(result)
                {
                    docs.push(text(": "));
                    docs.push(self.ty(result));
                }
                concat(docs)
            }
            TypeKind::NotCopy => text("~Copy"),
        }
    }

    /// `T?`, with parentheses around a type that would otherwise bind
    /// differently.
    fn optional(&mut self, inner: &TypeExpr) -> Doc {
        let doc = self.ty(inner);
        let wrap = matches!(&inner.kind, TypeKind::Union(_) | TypeKind::Result { .. } | TypeKind::Fn { .. })
            || matches!(&inner.kind, TypeKind::Path { segments, args } if path_text(segments) == "Result" && args.len() == 2);
        if wrap { concat(vec![text("("), doc, text(")?")]) } else { concat(vec![doc, text("?")]) }
    }

    /// `T ! E`, or `!E` when there is no success value.
    fn result_of(&mut self, ok: Option<&TypeExpr>, err: &TypeExpr) -> Doc {
        // A union, a result or a function type inside another result keeps
        // its parentheses: `(T ! E) ! F`.
        let nested = |t: &TypeExpr| {
            matches!(&t.kind, TypeKind::Union(_) | TypeKind::Result { .. } | TypeKind::Fn { .. })
                || matches!(&t.kind, TypeKind::Path { segments, args } if path_text(segments) == "Result" && args.len() == 2)
        };
        let err_doc = self.ty(err);
        let err_doc = if nested(err) { concat(vec![text("("), err_doc, text(")")]) } else { err_doc };
        match ok {
            Some(ok) if !is_unit(ok) => {
                let ok_doc = self.ty(ok);
                let ok_doc = if nested(ok) { concat(vec![text("("), ok_doc, text(")")]) } else { ok_doc };
                group(concat(vec![ok_doc, text(" !"), indent(concat(vec![Doc::Softline, err_doc]))]))
            }
            _ => concat(vec![text("!"), err_doc]),
        }
    }

    // ---- statements and blocks -------------------------------------------

    fn block(&mut self, block: &Block) -> Doc {
        let mut units: Vec<(Span, Option<&Stmt>)> = block.stmts.iter().map(|s| (s.span, Some(s))).collect();
        if let Some(tail) = &block.tail {
            units.push((tail.span, None));
        }
        let has_comments = self.comments.get(self.next).is_some_and(|c| c.start < block.span.end);
        if units.is_empty() && !has_comments {
            return text("{}");
        }
        let mut body = Vec::new();
        let mut previous: Option<u32> = None;
        for (span, stmt) in units {
            body.push(Doc::Hardline);
            if let Some(end) = previous
                && self.blank_between(end, self.first_comment_or(span.start))
            {
                body.push(Doc::Hardline);
            }
            body.push(self.leading(span.start));
            body.push(match stmt {
                Some(stmt) => self.stmt(stmt),
                None => self.expr(block.tail.as_ref().unwrap()),
            });
            body.push(self.trailing(span.end));
            previous = Some(span.end);
        }
        body.push(self.leading_in_block(block.span.end));
        concat(vec![text("{"), indent(concat(body)), Doc::Hardline, text("}")])
    }

    /// `if ... else if ... else ...`: with an `else` and only a value in
    /// each branch, one group that stays on one line when it fits;
    /// otherwise blocks.
    fn if_chain(&mut self, expr: &Expr) -> Doc {
        let mut branches: Vec<(Option<&Expr>, &Block)> = Vec::new();
        let mut current = expr;
        let mut last_else = None;
        while let ExprKind::If { condition, then, otherwise } = &current.kind {
            branches.push((Some(condition), then));
            match otherwise.as_deref() {
                Some(next @ Expr { kind: ExprKind::If { .. }, .. }) if !self.parenthesized(next) => current = next,
                Some(Expr { kind: ExprKind::Block(block), .. }) => {
                    branches.push((None, block));
                    break;
                }
                Some(other) => {
                    last_else = Some(other);
                    break;
                }
                None => break,
            }
        }
        let simple =
            branches.last().is_some_and(|(c, _)| c.is_none()) && branches.iter().all(|(_, b)| self.is_simple(b));
        let mut docs = Vec::new();
        for (index, (condition, block)) in branches.into_iter().enumerate() {
            if index > 0 {
                docs.push(text(" else "));
            }
            if let Some(condition) = condition {
                docs.push(text("if "));
                docs.push(self.expr(condition));
                docs.push(text(" "));
            }
            if simple {
                let value = self.expr(block.tail.as_ref().unwrap());
                docs.push(concat(vec![
                    text("{"),
                    indent(concat(vec![Doc::Softline, value])),
                    Doc::Softline,
                    text("}"),
                ]));
            } else {
                docs.push(self.block(block));
            }
        }
        if let Some(other) = last_else {
            docs.push(text(" else "));
            docs.push(self.expr(other));
        }
        if simple { group(concat(docs)) } else { concat(docs) }
    }

    /// Whether a block holds only a value and no comments.
    fn is_simple(&self, block: &Block) -> bool {
        block.stmts.is_empty()
            && block.tail.is_some()
            && !self.comments.iter().any(|c| c.start >= block.span.start && c.end <= block.span.end)
    }

    /// A block that produces a value: `{ value }` on one line when it holds
    /// only an expression without comments and fits; otherwise a block.
    fn value_block(&mut self, block: &Block) -> Doc {
        let has_comments = self.comments.get(self.next).is_some_and(|c| c.start < block.span.end);
        match &block.tail {
            Some(tail) if block.stmts.is_empty() && !has_comments => {
                let value = self.expr(tail);
                group(concat(vec![text("{"), indent(concat(vec![Doc::Softline, value])), Doc::Softline, text("}")]))
            }
            _ => self.block(block),
        }
    }

    fn stmt(&mut self, stmt: &Stmt) -> Doc {
        match &stmt.kind {
            StmtKind::Let { mutable, pattern, ty, value } => {
                let mut left = vec![text(if *mutable { "var " } else { "let " }), self.pattern(pattern)];
                if let Some(ty) = ty {
                    left.push(text(": "));
                    left.push(self.ty(ty));
                }
                match value {
                    Some(value) => self.assignment(concat(left), "=", value),
                    None => concat(left),
                }
            }
            StmtKind::Assign { target, op, value } => {
                let operator = match op {
                    Some(op) => format!("{}=", op.as_str()),
                    None => "=".into(),
                };
                let target = self.expr(target);
                self.assignment(target, &operator, value)
            }
            StmtKind::Discard(value) => self.assignment(text("_"), "=", value),
            StmtKind::With { path, fields } => {
                let fields = fields.iter().map(|f| self.field_init(f)).collect();
                concat(vec![text(format!("with {} ", path_text(path))), self.record_body(fields)])
            }
            StmtKind::Return(None) => text("return"),
            StmtKind::Return(Some(value)) => concat(vec![text("return "), self.expr(value)]),
            StmtKind::Fail(value) => concat(vec![text("fail "), self.expr(value)]),
            StmtKind::Break => text("break"),
            StmtKind::Continue => text("continue"),
            StmtKind::For { header, body } => {
                let head = match header {
                    ForHeader::Infinite => text("for "),
                    ForHeader::Condition(condition) => concat(vec![text("for "), self.expr(condition), text(" ")]),
                    ForHeader::In { pattern, source } => {
                        concat(vec![text("for "), self.pattern(pattern), text(" in "), self.expr(source), text(" ")])
                    }
                };
                concat(vec![head, self.block(body)])
            }
            StmtKind::Defer(Defer::Call(call)) => concat(vec![text("defer "), self.expr(call)]),
            StmtKind::Defer(Defer::Block(block)) => concat(vec![text("defer "), self.block(block)]),
            StmtKind::Expr(value) => self.expr(value),
        }
    }

    /// `left op right`, breaking after the operator unless the right side
    /// opens its own brackets.
    fn assignment(&mut self, left: Doc, operator: &str, value: &Expr) -> Doc {
        let right = self.expr(value);
        if self.opens_brackets(value) {
            concat(vec![left, text(format!(" {operator} ")), right])
        } else {
            group(concat(vec![left, text(format!(" {operator}")), indent(concat(vec![Doc::Softline, right]))]))
        }
    }

    /// Whether an expression ends in brackets of its own that can hold its
    /// line breaks.
    fn opens_brackets(&self, expr: &Expr) -> bool {
        // A string of several lines starts on the line of its operator.
        if let ExprKind::Str(_) = expr.kind {
            return self.source(expr.span).contains('\n');
        }
        !self.parenthesized(expr)
            && matches!(
                expr.kind,
                ExprKind::Record { .. }
                    | ExprKind::Array(_)
                    | ExprKind::Map(_)
                    | ExprKind::Set(_)
                    | ExprKind::Tuple(_)
                    | ExprKind::Call { .. }
                    | ExprKind::Block(_)
                    | ExprKind::If { .. }
                    | ExprKind::Match { .. }
                    | ExprKind::Select(_)
                    | ExprKind::Closure(_)
                    | ExprKind::With { .. }
            )
    }

    // ---- expressions -----------------------------------------------------

    fn parenthesized(&self, expr: &Expr) -> bool {
        !matches!(expr.kind, ExprKind::Tuple(_) | ExprKind::Unit) && self.parens.contains(&expr.span)
    }

    fn expr(&mut self, expr: &Expr) -> Doc {
        let doc = self.expr_inner(expr);
        if self.parenthesized(expr) { concat(vec![text("("), doc, text(")")]) } else { doc }
    }

    fn expr_inner(&mut self, expr: &Expr) -> Doc {
        match &expr.kind {
            ExprKind::Int { .. } | ExprKind::Float { .. } | ExprKind::Char(_) | ExprKind::Str(_) => {
                text(self.source(expr.span).to_string())
            }
            ExprKind::Bool(value) => text(value.to_string()),
            ExprKind::Unit => text("()"),
            ExprKind::None => text("none"),
            ExprKind::Error => text(self.source(expr.span).to_string()),
            ExprKind::Path(segments) => text(path_text(segments)),
            ExprKind::Field(..) | ExprKind::TupleSlot(..) | ExprKind::Call { .. } | ExprKind::Try(_) => {
                self.chain(expr)
            }
            ExprKind::Index { base, index } => {
                concat(vec![self.postfix_base(base), text("["), self.expr(index), text("]")])
            }
            ExprKind::Slice { base, start, end, step } => {
                let mut docs = vec![self.postfix_base(base), text("[")];
                if let Some(start) = start {
                    docs.push(self.expr(start));
                }
                docs.push(text(":"));
                if let Some(end) = end {
                    docs.push(self.expr(end));
                }
                if let Some(step) = step {
                    docs.push(text(":"));
                    docs.push(self.expr(step));
                }
                docs.push(text("]"));
                concat(docs)
            }
            ExprKind::Generic { base, args } => {
                let items = args.iter().map(|a| (a.span, self.ty(a))).collect();
                concat(vec![self.postfix_base(base), self.list("[", items, "]")])
            }
            ExprKind::Record { path, type_args, fields } => {
                let mut docs = vec![text(path_text(path))];
                if !type_args.is_empty() {
                    let items = type_args.iter().map(|a| (a.span, self.ty(a))).collect();
                    docs.push(self.list("[", items, "]"));
                }
                docs.push(text(" "));
                let fields = fields.iter().map(|f| self.field_init(f)).collect();
                docs.push(self.record_body(fields));
                concat(docs)
            }
            ExprKind::With { base, updates } => {
                let fields = updates.iter().map(|f| self.field_init(f)).collect();
                concat(vec![self.postfix_base(base), text(" with "), self.record_body(fields)])
            }
            ExprKind::Unary(op, operand) => {
                let symbol = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "not ",
                    UnaryOp::BitNot => "~",
                };
                let inner = self.expr(operand);
                // `- -value` would lex as `--`.
                let fused = *op == UnaryOp::Neg
                    && matches!(operand.kind, ExprKind::Unary(UnaryOp::Neg, _))
                    && !self.parenthesized(operand);
                let wrap = fused || matches!(operand.kind, ExprKind::Binary(..)) && !self.parenthesized(operand);
                if wrap {
                    concat(vec![text(symbol), text("("), inner, text(")")])
                } else {
                    concat(vec![text(symbol), inner])
                }
            }
            ExprKind::Binary(op, ..) => {
                let level = level(*op);
                let mut operands = Vec::new();
                self.binary_chain(expr, level, true, &mut operands);
                let mut iter = operands.into_iter();
                let (_, first) = iter.next().unwrap();
                let mut docs = vec![first];
                for (op, operand) in iter {
                    let op = op.unwrap();
                    // A range is written tight, as in `0..count`.
                    if matches!(op, BinOp::Range | BinOp::RangeInclusive) {
                        docs.push(text(op.as_str()));
                        docs.push(operand);
                        continue;
                    }
                    docs.push(text(format!(" {}", op.as_str())));
                    docs.push(indent(concat(vec![Doc::Softline, operand])));
                }
                group(concat(docs))
            }
            ExprKind::If { .. } => self.if_chain(expr),
            ExprKind::Block(block) => self.value_block(block),
            ExprKind::Match { scrutinee, arms } => {
                let mut body = Vec::new();
                for arm in arms {
                    body.push(Doc::Hardline);
                    body.push(self.leading(arm.span.start));
                    body.push(self.arm(arm));
                    body.push(self.trailing(arm.span.end));
                }
                body.push(self.leading_in_block(expr.span.end));
                concat(vec![
                    text("match "),
                    self.expr(scrutinee),
                    text(" {"),
                    indent(concat(body)),
                    Doc::Hardline,
                    text("}"),
                ])
            }
            ExprKind::Select(arms) => {
                let mut body = Vec::new();
                for arm in arms {
                    body.push(Doc::Hardline);
                    body.push(self.leading(arm.span.start));
                    let left = match (&arm.pattern, &arm.operation) {
                        (Some(pattern), Some(operation)) => {
                            vec![text("let "), self.pattern(pattern), text(" = "), self.expr(operation)]
                        }
                        (None, Some(operation)) => vec![self.expr(operation)],
                        _ => vec![text("else")],
                    };
                    body.push(self.arm_with(left, &arm.body));
                    body.push(self.trailing(arm.span.end));
                }
                body.push(self.leading_in_block(expr.span.end));
                concat(vec![text("select {"), indent(concat(body)), Doc::Hardline, text("}")])
            }
            ExprKind::Tuple(items) => {
                let items: Vec<(Span, Doc)> = items.iter().map(|e| (e.span, self.expr(e))).collect();
                if items.len() == 1 {
                    let (_, item) = items.into_iter().next().unwrap();
                    return concat(vec![text("("), item, text(",)")]);
                }
                self.list("(", items, ")")
            }
            ExprKind::Array(items) => {
                let items = items.iter().map(|e| (e.span, self.expr(e))).collect();
                self.list("[", items, "]")
            }
            ExprKind::Set(items) => {
                let items = items.iter().map(|e| (e.span, self.expr(e))).collect();
                concat(vec![text("Set"), self.list("[", items, "]")])
            }
            ExprKind::Map(entries) => {
                if entries.is_empty() {
                    return text("[:]");
                }
                let items = entries
                    .iter()
                    .map(|(k, v)| (k.span.to(v.span), concat(vec![self.expr(k), text(": "), self.expr(v)])))
                    .collect();
                self.list("[", items, "]")
            }
            ExprKind::Closure(closure) => {
                let params = closure
                    .params
                    .iter()
                    .map(|p| {
                        let end = p.ty.as_ref().map_or(p.name.span.end, |t| t.span.end);
                        let doc = match &p.ty {
                            None => text(p.name.name.clone()),
                            Some(ty) => {
                                let mode = match p.mode {
                                    Mode::Value => String::new(),
                                    mode => format!("{} ", mode.keyword()),
                                };
                                let dots = if p.variadic { "..." } else { "" };
                                concat(vec![text(format!("{}: {mode}{dots}", p.name.name)), self.ty(ty)])
                            }
                        };
                        (Span::new(p.name.span.start, end), doc)
                    })
                    .collect();
                let mut docs = vec![];
                if closure.is_unsafe {
                    docs.push(text("unsafe "));
                }
                docs.push(self.list("(", params, ")"));
                if let Some(result) = &closure.result
                    && !is_unit(result)
                {
                    docs.push(text(": "));
                    docs.push(self.ty(result));
                }
                docs.push(text(" "));
                docs.push(self.value_block(&closure.body));
                concat(docs)
            }
            ExprKind::Construct(name, value) => concat(vec![text(format!("{name}(")), self.expr(value), text(")")]),
            ExprKind::Spawn(call, detached) => {
                concat(vec![text(if *detached { "spawn detached " } else { "spawn " }), self.expr(call)])
            }
            ExprKind::Unsafe(block) => concat(vec![text("unsafe "), self.value_block(block)]),
            ExprKind::Recover(block) => concat(vec![text("recover "), self.value_block(block)]),
            ExprKind::Within(limit, block) => {
                concat(vec![text("within "), self.expr(limit), text(" "), self.value_block(block)])
            }
            ExprKind::SyncLiteral(path, literal) => concat(vec![text(path_text(path)), self.expr(literal)]),
        }
    }

    /// The operands of a run of left-associative operators of one
    /// precedence level, with the operator before each but the first.
    fn binary_chain(&mut self, expr: &Expr, chain: usize, top: bool, out: &mut Vec<(Option<BinOp>, Doc)>) {
        if let ExprKind::Binary(op, left, right) = &expr.kind
            && level(*op) == chain
            && (top || !self.parenthesized(expr))
        {
            self.binary_chain(left, chain, false, out);
            let right = self.expr(right);
            out.push((Some(*op), right));
            return;
        }
        let doc = self.expr(expr);
        out.push((None, doc));
    }

    /// The base of a postfix operation, parenthesized when it is an
    /// operation itself.
    fn postfix_base(&mut self, base: &Expr) -> Doc {
        let doc = self.expr(base);
        let needs = !self.parenthesized(base)
            && matches!(base.kind, ExprKind::Binary(..) | ExprKind::Unary(..) | ExprKind::Closure(_));
        if needs { concat(vec![text("("), doc, text(")")]) } else { doc }
    }

    /// A chain of field accesses, calls and `?`: one group that, when it
    /// breaks, puts each `.segment` on its own line.
    fn chain(&mut self, expr: &Expr) -> Doc {
        let mut links = Vec::new();
        let mut current = expr;
        let head = loop {
            match &current.kind {
                ExprKind::Field(base, _) | ExprKind::TupleSlot(base, _) | ExprKind::Try(base)
                    if !self.parenthesized(current) || std::ptr::eq(current, expr) =>
                {
                    links.push(current);
                    current = base;
                }
                ExprKind::Call { callee, .. } if !self.parenthesized(current) || std::ptr::eq(current, expr) => {
                    links.push(current);
                    current = callee;
                }
                _ => break current,
            }
        };
        links.reverse();
        let dots = links.iter().filter(|l| matches!(l.kind, ExprKind::Field(..) | ExprKind::TupleSlot(..))).count();
        // A path such as `repository.find` starts the chain at its last dot
        // when other segments follow.
        let (head_doc, path_tail) = match &head.kind {
            ExprKind::Path(segments) if segments.len() > 1 && dots > 0 && !self.parenthesized(head) => {
                let (last, rest) = segments.split_last().unwrap();
                (text(path_text(rest)), Some(last.name.clone()))
            }
            _ => (self.postfix_base(head), None),
        };
        let breakable = dots + usize::from(path_tail.is_some()) >= 2;
        let mut head_doc = head_doc;
        let mut tail = Vec::new();
        if let Some(name) = path_tail {
            tail.push(Doc::Softzero);
            tail.push(text(format!(".{name}")));
        }
        let mut after_try = false;
        for link in links {
            match &link.kind {
                ExprKind::Field(_, name) => {
                    tail.push(Doc::Softzero);
                    tail.push(text(format!(".{}", name.name)));
                }
                ExprKind::TupleSlot(_, slot) => {
                    tail.push(Doc::Softzero);
                    tail.push(text(format!(".{slot}")));
                }
                // `value? ?` would lex as `??`.
                ExprKind::Try(_) if after_try => {
                    let inner = std::mem::take(&mut tail);
                    head_doc = concat(vec![text("("), head_doc, concat(inner), text(")?")]);
                }
                ExprKind::Try(_) => tail.push(text("?")),
                ExprKind::Call { args, .. } => {
                    let items = args.iter().map(|a| (a.span, self.argument(a))).collect();
                    tail.push(self.list("(", items, ")"));
                }
                _ => unreachable!("chain links are postfix operations"),
            }
            after_try = matches!(link.kind, ExprKind::Try(_));
        }
        if !breakable {
            let tail: Vec<Doc> = tail.into_iter().filter(|d| !matches!(d, Doc::Softzero)).collect();
            return concat(std::iter::once(head_doc).chain(tail).collect());
        }
        group(concat(vec![head_doc, indent(concat(tail))]))
    }

    fn argument(&mut self, arg: &Arg) -> Doc {
        let mut prefix = String::new();
        if let Some(name) = &arg.name {
            prefix.push_str(&format!("{}: ", name.name));
        }
        if arg.mode != Mode::Value {
            prefix.push_str(arg.mode.keyword());
            prefix.push(' ');
        }
        if arg.spread {
            prefix.push_str("...");
        }
        concat(vec![text(prefix), self.expr(&arg.value)])
    }

    fn field_init(&mut self, field: &FieldInit) -> (Span, Doc) {
        match &field.value {
            None => (field.name.span, text(field.name.name.clone())),
            Some(value) => {
                (field.name.span.to(value.span), self.assignment_like(text(format!("{}:", field.name.name)), value))
            }
        }
    }

    /// `name: value` inside a record.
    fn assignment_like(&mut self, left: Doc, value: &Expr) -> Doc {
        let right = self.expr(value);
        concat(vec![left, text(" "), right])
    }

    fn arm(&mut self, arm: &MatchArm) -> Doc {
        let mut left = vec![self.pattern(&arm.pattern)];
        if let Some(guard) = &arm.guard {
            left.push(text(" if "));
            left.push(self.expr(guard));
        }
        self.arm_with(left, &arm.body)
    }

    fn arm_with(&mut self, left: Vec<Doc>, body: &ArmBody) -> Doc {
        match body {
            ArmBody::Expr(body) if self.opens_brackets(body) => {
                concat(vec![concat(left), text(" => "), self.expr(body)])
            }
            ArmBody::Expr(body) => {
                let right = self.expr(body);
                group(concat(vec![concat(left), text(" =>"), indent(concat(vec![Doc::Softline, right]))]))
            }
            ArmBody::Statement(stmt) => concat(vec![concat(left), text(" => "), self.stmt(stmt)]),
        }
    }

    // ---- patterns ----------------------------------------------------------

    fn pattern(&mut self, pattern: &Pattern) -> Doc {
        match &pattern.kind {
            PatternKind::Wildcard => text("_"),
            PatternKind::Binding(name) => text(name.name.clone()),
            PatternKind::Loan(mode, name) => text(format!("{} {}", mode.keyword(), name.name)),
            PatternKind::Literal(expr) => self.expr(expr),
            PatternKind::None => text("none"),
            PatternKind::Tuple(items) => {
                let items = items.iter().map(|p| (p.span, self.pattern(p))).collect();
                self.list("(", items, ")")
            }
            PatternKind::Constructor { path, args } => {
                let items = args.iter().map(|p| (p.span, self.pattern(p))).collect();
                concat(vec![text(path_text(path)), self.list("(", items, ")")])
            }
            PatternKind::Record { path, fields, rest } => {
                let mut items: Vec<(Span, Doc)> = fields
                    .iter()
                    .map(|f| match &f.pattern {
                        None => (f.name.span, text(f.name.name.clone())),
                        Some(Pattern { kind: PatternKind::Loan(mode, name), .. }) if name.name == f.name.name => {
                            (f.name.span, text(format!("{} {}", mode.keyword(), name.name)))
                        }
                        Some(pattern) => {
                            let doc = concat(vec![text(format!("{}: ", f.name.name)), self.pattern(pattern)]);
                            (f.name.span.to(pattern.span), doc)
                        }
                    })
                    .collect();
                if *rest {
                    items.push((Span::new(pattern.span.end, pattern.span.end), text("..")));
                }
                concat(vec![text(format!("{} ", path_text(path))), self.record_body(items)])
            }
            PatternKind::Path(path) => text(path_text(path)),
            PatternKind::Array { items, rest } => {
                let mut items: Vec<(Span, Doc)> = items.iter().map(|p| (p.span, self.pattern(p))).collect();
                if let Some(rest) = rest {
                    let doc = match rest {
                        None => text(".."),
                        Some((Mode::Ref, name)) => text(format!("..ref {}", name.name)),
                        Some((_, name)) => text(format!("..{}", name.name)),
                    };
                    items.push((Span::new(pattern.span.end, pattern.span.end), doc));
                }
                self.list("[", items, "]")
            }
            PatternKind::Member { ty, inner } => concat(vec![self.ty(ty), text("("), self.pattern(inner), text(")")]),
        }
    }

    // ---- lists -------------------------------------------------------------

    /// A delimited list (21.3): one line when it fits, otherwise one item
    /// per line with a trailing comma.
    fn list(&mut self, open: &str, items: Vec<(Span, Doc)>, close: &str) -> Doc {
        if items.is_empty() {
            return text(format!("{open}{close}"));
        }
        let count = items.len();
        let mut body = vec![Doc::Softzero];
        for (index, (span, doc)) in items.into_iter().enumerate() {
            body.push(self.leading(span.start));
            body.push(doc);
            if index + 1 < count {
                body.push(text(","));
                body.push(self.trailing(span.end));
                body.push(Doc::Softline);
            } else {
                body.push(if_break(text(","), concat(Vec::new())));
                body.push(self.trailing(span.end));
            }
        }
        group(concat(vec![text(open), indent(concat(body)), Doc::Softzero, text(close)]))
    }

    /// `{ a, b }` on one line, or one field per line without commas.
    fn record_body(&mut self, items: Vec<(Span, Doc)>) -> Doc {
        if items.is_empty() {
            return text("{}");
        }
        let count = items.len();
        let mut body = vec![Doc::Softline];
        for (index, (span, doc)) in items.into_iter().enumerate() {
            body.push(self.leading(span.start));
            body.push(doc);
            if index + 1 < count {
                body.push(if_break(concat(Vec::new()), text(",")));
                body.push(self.trailing(span.end));
                body.push(Doc::Softline);
            } else {
                body.push(self.trailing(span.end));
            }
        }
        group(concat(vec![text("{"), indent(concat(body)), Doc::Softline, text("}")]))
    }
}

fn strip_last_hardline(doc: Doc) -> Doc {
    match doc {
        Doc::Concat(mut docs) => {
            while matches!(docs.last(), Some(Doc::Hardline)) {
                docs.pop();
            }
            Doc::Concat(docs)
        }
        doc => doc,
    }
}

fn path_text(segments: &[Ident]) -> String {
    segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(".")
}

fn is_unit(ty: &TypeExpr) -> bool {
    matches!(&ty.kind, TypeKind::Path { segments, args } if args.is_empty() && path_text(segments) == "Unit")
}

/// The precedence level of a binary operator, as the parser reads it.
fn level(op: BinOp) -> usize {
    match op {
        BinOp::Or => 0,
        BinOp::And => 1,
        BinOp::Eq | BinOp::Ne => 2,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::In => 3,
        BinOp::Range | BinOp::RangeInclusive => 4,
        BinOp::BitOr => 5,
        BinOp::BitXor => 6,
        BinOp::BitAnd => 7,
        BinOp::Shl | BinOp::Shr => 8,
        BinOp::Add | BinOp::Sub => 9,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 10,
    }
}

/// Annotations, each on its own line before what they annotate.
fn annotations(annotations: &[Annotation]) -> Doc {
    let mut parts = Vec::new();
    for annotation in annotations {
        let argument = match &annotation.argument {
            None => String::new(),
            Some(AnnotationArg::Text(text)) => format!("({text:?})"),
            Some(AnnotationArg::Int(value)) => format!("({value})"),
        };
        parts.push(text(format!("@{}{argument}", annotation.name.name)));
        parts.push(Doc::Hardline);
    }
    concat(parts)
}

fn item_span(item: &Item) -> Span {
    match item {
        Item::Const(c) => c.span,
        Item::Fn(f) => f.span,
        Item::Type(t) => t.span,
        Item::Alias(a) => a.span,
        Item::Enum(e) => e.span,
        Item::Trait(t) => t.span,
        Item::Impl(i) => i.span,
        Item::Test(node) => node.span(),
        Item::Derive(d) => d.span,
    }
}

#[cfg(test)]
mod tests {
    use crate::diag::Diagnostics;
    use crate::source::SourceMap;
    use std::path::Path;

    fn format(source: &str) -> String {
        let mut sources = SourceMap::default();
        let file = sources.add(Path::new("test.to"), source.to_string());
        let mut diags = Diagnostics::default();
        super::format(sources.file(file), &mut diags).unwrap_or_else(|| panic!("{}", diags.render(&sources)))
    }

    /// The pairs of 21.7.
    #[test]
    fn corpus() {
        let pairs = [
            ("fn add( a:Int,b:Int):Int {a+b}\n", "fn add(a: Int, b: Int): Int {\n    a + b\n}\n"),
            ("let values=[\n1,\n2\n]\n", "let values = [1, 2]\n"),
            ("import zeta\nimport alpha as a\nfn main(){}\n", "import alpha as a\nimport zeta\n\nfn main() {}\n"),
            ("let values=[1, // first\n2]\n", "let values = [\n    1,  // first\n    2,\n]\n"),
            ("type Loader=fn():Result[Option[Int],IoError]\n", "type Loader = fn(): Int? ! IoError\n"),
            (
                "fn make():impl Iterator[Int]+Send{build()}\n",
                "fn make(): impl Iterator[Int] + Send {\n    build()\n}\n",
            ),
            ("let inverse=- -value\nlet nested=value? ?\n", "let inverse = -(-value)\nlet nested = (value?)?\n"),
            ("type Nested=(T!E)!F\n", "type Nested = (T ! E) ! F\n"),
            (
                "enum E {\n/// One.\nPair(Int, Int)\n/// Two.\nPlain\n}\n",
                "enum E {\n    /// One.\n    Pair(Int, Int)\n    /// Two.\n    Plain\n}\n",
            ),
            (
                "let double=(n:Int):Int{n*2}\nlet larger=if a>b{a}else{b}\n",
                "let double = (n: Int): Int { n * 2 }\nlet larger = if a > b { a } else { b }\n",
            ),
            (
                "@symbol( \"printf\" ) pub extern fn print(format:CString,...):CInt\n",
                "@symbol(\"printf\")\npub extern fn print(format: CString, ...): CInt\n",
            ),
            (
                "@packed @align(8) extern type Header={@count(4) name:Byte\nnext:Pointer[Header]?}\nextern type Handle\n",
                "@packed\n@align(8)\nextern type Header = {\n    @count(4)\n    name: Byte\n    next: Pointer[Header]?\n}\n\nextern type Handle\n",
            ),
            ("type Compare=extern fn(Int,Int,...):Int\n", "type Compare = extern fn(Int, Int, ...): Int\n"),
            ("pub const  Limit:Int=1<<4\nconst Name=\"x\"\n", "pub const Limit: Int = 1 << 4\n\nconst Name = \"x\"\n"),
        ];
        for (input, output) in pairs {
            assert_eq!(format(input), output, "formatting {input:?}");
        }
    }

    /// A list stays flat when it ends at column 99 or 100, and breaks at 101.
    #[test]
    fn width() {
        for (end, flat) in [(99, true), (100, true), (101, false)] {
            // `let values = [` is 14 columns and `]` one more.
            let item = "x".repeat(end - 15);
            let source = format!("let values = [{item}]\n");
            let expected = if flat { source.clone() } else { format!("let values = [\n    {item},\n]\n") };
            assert_eq!(format(&source), expected, "a list ending at column {end}");
        }
    }

    /// The standard modules written in Tondo are formatted.
    #[test]
    fn standard_modules() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("std");
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert_eq!(format(&source), source, "{} is not formatted: run `tondo fmt` on it", path.display());
        }
    }

    /// Formatting the test programs twice changes nothing the second time.
    #[test]
    fn idempotent() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases");
        let mut paths = Vec::new();
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "to") {
                    paths.push(path);
                }
            }
        }
        for path in paths {
            let source = std::fs::read_to_string(&path).unwrap();
            let mut sources = SourceMap::default();
            let file = sources.add(&path, source);
            let Some(once) = super::format(sources.file(file), &mut Diagnostics::default()) else { continue };
            assert_eq!(format(&once), once, "{} is not formatted the same twice", path.display());
        }
    }
}
