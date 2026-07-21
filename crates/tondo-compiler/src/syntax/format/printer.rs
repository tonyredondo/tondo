use std::borrow::Cow;
use std::cell::Cell;
use std::error::Error;
use std::fmt;
use std::str;

use crate::source::{FileId, SourceDatabase, SourceError};

use super::{Doc, render};
use crate::syntax::{
    Cst, Parsed, SyntaxElement, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenId, TokenKind,
};

const WIDTH: usize = 100;
const INDENT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeShape {
    Primary,
    Optional,
    Result,
    Union,
}

#[derive(Debug)]
pub enum FormatError {
    Source(SourceError),
    InvalidUtf8,
    InvalidSyntax,
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::InvalidUtf8 => formatter.write_str("formatter input is not valid UTF-8"),
            Self::InvalidSyntax => {
                formatter.write_str("formatter input contains lexical or syntax diagnostics")
            }
        }
    }
}

impl Error for FormatError {}

impl From<SourceError> for FormatError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedSource(Vec<u8>);

impl FormattedSource {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

pub fn format_parsed(
    sources: &SourceDatabase,
    file: FileId,
    parsed: &Parsed,
) -> Result<FormattedSource, FormatError> {
    if !parsed.diagnostics().is_empty() {
        return Err(FormatError::InvalidSyntax);
    }
    let source = sources.get(file)?;
    let source = str::from_utf8(source.bytes()).map_err(|_| FormatError::InvalidUtf8)?;
    let formatter = Formatter::new(source, parsed.cst());
    let document = formatter.format_root();
    let mut output = render(&document, WIDTH, INDENT);
    while output.ends_with('\n') {
        output.pop();
    }
    output.push('\n');
    Ok(FormattedSource(output.into_bytes()))
}

#[derive(Debug, Clone)]
struct Piece<'a> {
    doc: Doc<'a>,
    first_kind: Option<TokenKind>,
    last_kind: Option<TokenKind>,
    first_token: Option<TokenId>,
    last_token: Option<TokenId>,
    starts_with_leading_comment: bool,
    ends_with_break: bool,
}

#[derive(Debug, Clone)]
struct ListItem<'a> {
    piece: Piece<'a>,
    trailing_comments: Option<Doc<'a>>,
    trailing_comments_break: bool,
}

impl<'a> Piece<'a> {
    fn nil() -> Self {
        Self {
            doc: Doc::Nil,
            first_kind: None,
            last_kind: None,
            first_token: None,
            last_token: None,
            starts_with_leading_comment: false,
            ends_with_break: false,
        }
    }

    fn generated(doc: Doc<'a>, first_kind: TokenKind, last_kind: TokenKind) -> Self {
        Self {
            doc,
            first_kind: Some(first_kind),
            last_kind: Some(last_kind),
            first_token: None,
            last_token: None,
            starts_with_leading_comment: false,
            ends_with_break: false,
        }
    }
}

#[derive(Debug, Clone)]
struct CommentRun {
    comments: Vec<TokenId>,
    blank_before: bool,
    blank_after: bool,
    inline_with_previous: bool,
    break_after: bool,
}

#[derive(Debug)]
struct CommentMap {
    leading: Vec<Vec<CommentRun>>,
    trailing: Vec<Vec<CommentRun>>,
    section_after: Vec<Vec<CommentRun>>,
    header: Vec<CommentRun>,
}

impl CommentMap {
    fn new(source: &str, cst: &Cst) -> Self {
        let mut map = Self {
            leading: vec![Vec::new(); cst.original_token_count()],
            trailing: vec![Vec::new(); cst.original_token_count()],
            section_after: vec![Vec::new(); cst.original_token_count()],
            header: Vec::new(),
        };
        let comments = (0..cst.original_token_count())
            .filter(|&index| is_comment(cst.tokens()[index].kind()))
            .collect::<Vec<_>>();
        let mut cursor = 0;
        while cursor < comments.len() {
            let start = cursor;
            let first_comment = comments[start];
            let previous_significant = (0..first_comment)
                .rev()
                .find(|&index| is_significant(cst.tokens()[index].kind()));
            let starts_inline = previous_significant.is_some_and(|previous| {
                line_breaks(
                    source,
                    cst.tokens()[previous].range().end(),
                    cst.tokens()[first_comment].range().start(),
                ) == 0
            });
            let starts_as_doc = cst.tokens()[first_comment].kind() == TokenKind::DocComment;
            cursor += 1;
            while cursor < comments.len() {
                let previous = &cst.tokens()[comments[cursor - 1]];
                let next = &cst.tokens()[comments[cursor]];
                let breaks_between =
                    line_breaks(source, previous.range().end(), next.range().start());
                let significant_between = (comments[cursor - 1] + 1..comments[cursor])
                    .any(|index| is_significant(cst.tokens()[index].kind()));
                if significant_between
                    || breaks_between > 1
                    || (starts_inline && breaks_between > 0)
                    || (next.kind() == TokenKind::DocComment) != starts_as_doc
                {
                    break;
                }
                cursor += 1;
            }
            let indices = &comments[start..cursor];
            let first = indices[0];
            let last = *indices.last().expect("a comment run is not empty");
            let previous = (0..first)
                .rev()
                .find(|&index| is_significant(cst.tokens()[index].kind()));
            let next = (last + 1..cst.original_token_count())
                .find(|&index| is_significant(cst.tokens()[index].kind()));
            let previous_relevant = (0..first).rev().find(|&index| {
                is_comment(cst.tokens()[index].kind()) || is_significant(cst.tokens()[index].kind())
            });
            let next_relevant = (last + 1..cst.original_token_count()).find(|&index| {
                is_comment(cst.tokens()[index].kind()) || is_significant(cst.tokens()[index].kind())
            });
            let has_doc = indices
                .iter()
                .any(|&index| cst.tokens()[index].kind() == TokenKind::DocComment);
            let same_line_as_previous = previous.is_some_and(|previous| {
                line_breaks(
                    source,
                    cst.tokens()[previous].range().end(),
                    cst.tokens()[first].range().start(),
                ) == 0
            });
            let blank_after = next_relevant.is_some_and(|next| {
                line_breaks(
                    source,
                    cst.tokens()[last].range().end(),
                    cst.tokens()[next].range().start(),
                ) > 1
            });
            let run = CommentRun {
                comments: indices
                    .iter()
                    .map(|&index| TokenId::from_original_index(index))
                    .collect(),
                blank_before: previous_relevant.is_some_and(|previous| {
                    line_breaks(
                        source,
                        cst.tokens()[previous].range().end(),
                        cst.tokens()[first].range().start(),
                    ) > 1
                }),
                blank_after,
                inline_with_previous: same_line_as_previous,
                break_after: next_relevant.is_none_or(|next| {
                    line_breaks(
                        source,
                        cst.tokens()[last].range().end(),
                        cst.tokens()[next].range().start(),
                    ) > 0
                }),
            };
            if has_doc {
                if let Some(next) = next {
                    map.leading[next].push(run);
                } else if let Some(previous) = previous {
                    map.section_after[previous].push(run);
                } else {
                    map.header.push(run);
                }
            } else if same_line_as_previous {
                map.trailing[previous.expect("same-line comments have a predecessor")].push(run);
            } else if blank_after || next.is_none() {
                if let Some(previous) = previous {
                    map.section_after[previous].push(run);
                } else {
                    map.header.push(run);
                }
            } else if let Some(next) = next {
                map.leading[next].push(run);
            } else if let Some(previous) = previous {
                map.section_after[previous].push(run);
            } else {
                map.header.push(run);
            }
        }
        map
    }
}

struct Formatter<'a> {
    source: &'a str,
    cst: &'a Cst,
    comments: CommentMap,
    suppressed_trailing: Cell<Option<TokenId>>,
}

impl<'a> Formatter<'a> {
    fn new(source: &'a str, cst: &'a Cst) -> Self {
        Self {
            source,
            cst,
            comments: CommentMap::new(source, cst),
            suppressed_trailing: Cell::new(None),
        }
    }

    fn format_root(&self) -> Doc<'a> {
        let root = self.cst.root_node();
        let shebang = self.cst.tokens()[..self.cst.original_token_count()]
            .iter()
            .position(|token| token.kind() == TokenKind::Shebang)
            .map(|index| {
                Piece::generated(
                    self.token_text(self.cst.token_ref(TokenId::from_original_index(index))),
                    TokenKind::Shebang,
                    TokenKind::Shebang,
                )
            });
        let header = (!self.comments.header.is_empty()).then(|| Piece {
            doc: Doc::concat(self.comment_runs_doc(&self.comments.header, true)),
            first_kind: Some(TokenKind::LineComment),
            last_kind: Some(TokenKind::LineComment),
            first_token: None,
            last_token: None,
            starts_with_leading_comment: true,
            ends_with_break: true,
        });

        let mut imports = Vec::new();
        let mut units = Vec::new();
        for child in root.child_nodes() {
            if child.kind() == SyntaxKind::ImportDecl {
                imports.push(child);
            } else {
                units.push(child);
            }
        }
        let formatted_imports = self.format_import_groups(&imports);
        let formatted_units = units
            .iter()
            .map(|unit| self.with_section_comments(*unit, self.format_node(*unit)))
            .collect::<Vec<_>>();
        let mut body = Vec::new();
        for (index, import) in formatted_imports.iter().enumerate() {
            if index > 0 {
                self.push_separation(&mut body, &formatted_imports[index - 1], 2);
            }
            body.push(import.doc.clone());
        }
        if let (Some(last_import), Some(_)) = (formatted_imports.last(), formatted_units.first()) {
            self.push_separation(&mut body, last_import, 2);
        }
        for (index, unit) in formatted_units.iter().enumerate() {
            if index > 0 {
                let lines = if self.top_level_blank_between(units[index - 1], units[index]) {
                    2
                } else {
                    1
                };
                self.push_separation(&mut body, &formatted_units[index - 1], lines);
            }
            body.push(unit.doc.clone());
        }

        let prefix = match (shebang, header) {
            (Some(shebang), Some(header)) => {
                let mut docs = vec![shebang.doc.clone()];
                self.push_separation(&mut docs, &shebang, 2);
                docs.push(header.doc);
                Some(Piece {
                    doc: Doc::concat(docs),
                    first_kind: shebang.first_kind,
                    last_kind: header.last_kind,
                    first_token: shebang.first_token,
                    last_token: header.last_token,
                    starts_with_leading_comment: false,
                    ends_with_break: header.ends_with_break,
                })
            }
            (Some(shebang), None) => Some(shebang),
            (None, Some(header)) => Some(header),
            (None, None) => None,
        };
        let mut document = Vec::new();
        if let Some(prefix) = prefix {
            document.push(prefix.doc.clone());
            if !body.is_empty() {
                self.push_separation(&mut document, &prefix, 2);
            }
        }
        document.extend(body);
        Doc::concat(document)
    }

    fn format_import_groups(&self, imports: &[SyntaxNodeRef<'a>]) -> Vec<Piece<'a>> {
        let mut groups = Vec::new();
        let mut start = 0;
        while start < imports.len() {
            let mut end = start + 1;
            while end < imports.len() && !self.blank_between_nodes(imports[end - 1], imports[end]) {
                end += 1;
            }
            let original_last = imports[end - 1];
            let mut sorted = imports[start..end].to_vec();
            sorted.sort_by_key(|node| self.import_key(*node));
            let pieces = sorted
                .iter()
                .map(|node| self.format_node(*node))
                .collect::<Vec<_>>();
            let first = pieces
                .first()
                .expect("an import group contains at least one import");
            let last = pieces
                .last()
                .expect("an import group contains at least one import");
            let mut docs = Vec::new();
            for (index, piece) in pieces.iter().enumerate() {
                if index > 0 {
                    self.push_separation(&mut docs, &pieces[index - 1], 1);
                }
                docs.push(piece.doc.clone());
            }
            let group = Piece {
                doc: Doc::concat(docs),
                first_kind: first.first_kind,
                last_kind: last.last_kind,
                first_token: first.first_token,
                last_token: last.last_token,
                starts_with_leading_comment: first.starts_with_leading_comment,
                ends_with_break: last.ends_with_break,
            };
            groups.push(self.with_section_comments(original_last, group));
            start = end;
        }
        groups
    }

    fn format_node(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        match node.kind() {
            SyntaxKind::Block => self.format_block(node),
            SyntaxKind::RecordBody
            | SyntaxKind::EnumDecl
            | SyntaxKind::TraitDecl
            | SyntaxKind::ImplDecl
            | SyntaxKind::MatchExpr => self.format_forced_braces(node),
            SyntaxKind::RecordLikeExpr
            | SyntaxKind::RecordUpdateBody
            | SyntaxKind::RecordPattern => self.format_flexible_record(node),
            SyntaxKind::ParameterList
            | SyntaxKind::GenericParams
            | SyntaxKind::GenericArgs
            | SyntaxKind::TuplePayload
            | SyntaxKind::ClosureParameterList
            | SyntaxKind::CallSuffix
            | SyntaxKind::TupleExpr
            | SyntaxKind::TupleType
            | SyntaxKind::TuplePattern
            | SyntaxKind::TupleAssignmentPattern
            | SyntaxKind::ConstructorPattern
            | SyntaxKind::ArrayPattern
            | SyntaxKind::BracketLiteralExpr
            | SyntaxKind::SetLiteralExpr => self.format_list_node(node, true),
            SyntaxKind::BracketPostfix => self.format_list_node(node, false),
            SyntaxKind::FunctionType => self.format_function_type(node),
            SyntaxKind::FunctionTypeItem => self.format_function_type_item(node),
            SyntaxKind::BinaryExpr => self.format_binary(node),
            SyntaxKind::Assignment => self.format_assignment(node),
            SyntaxKind::MatchArm => self.format_match_arm(node),
            SyntaxKind::PostfixExpr => self.format_postfix(node),
            SyntaxKind::PathExpr => self.format_path_expression(node),
            SyntaxKind::PrefixExpr => self.format_prefix(node),
            SyntaxKind::UnionType => self.format_union_type(node),
            SyntaxKind::ResultType => self.format_result_type(node),
            SyntaxKind::OptionalType => self.format_optional_type(node),
            SyntaxKind::GenericBound => self.format_operator_sequence(node),
            SyntaxKind::OutcomeAnnotation if self.is_omittable_unit_outcome(node) => Piece::nil(),
            SyntaxKind::PathType => self.format_path_type(node),
            _ => self.format_generic(node),
        }
    }

    fn format_generic(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let pieces = self.element_pieces(node.elements());
        self.join_pieces(pieces, node.kind()).with_group()
    }

    fn format_block(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        self.format_braces(node, true, false)
    }

    fn format_forced_braces(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        self.format_braces(node, true, true)
    }

    fn format_flexible_record(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        self.format_braces(node, false, false)
    }

    fn format_braces(
        &self,
        node: SyntaxNodeRef<'a>,
        force_break: bool,
        break_empty: bool,
    ) -> Piece<'a> {
        let elements = self.clean_elements(node.elements());
        let Some(open_index) = elements
            .iter()
            .position(|element| self.element_token_kind(*element) == Some(TokenKind::LBrace))
        else {
            return self.format_generic(node);
        };
        let Some(close_index) = elements
            .iter()
            .rposition(|element| self.element_token_kind(*element) == Some(TokenKind::RBrace))
        else {
            return self.format_generic(node);
        };
        let prefix = self.join_pieces(self.element_pieces(&elements[..open_index]), node.kind());
        let open = self.format_element(elements[open_index]);
        let close = self.format_element(elements[close_index]);
        let mut item_entries = Vec::new();
        for element in &elements[open_index + 1..close_index] {
            match *element {
                SyntaxElement::Node(id) => item_entries.push((self.cst.node_ref(id), None)),
                SyntaxElement::Token(id)
                    if self.cst.token(id).kind() == TokenKind::Comma
                        && !item_entries.is_empty() =>
                {
                    item_entries
                        .last_mut()
                        .expect("a record separator follows an item")
                        .1 = Some(self.cst.token_ref(id));
                }
                SyntaxElement::Token(_) => {}
            }
        }
        let item_nodes = item_entries
            .iter()
            .map(|(node, _)| *node)
            .collect::<Vec<_>>();
        let items = item_entries
            .iter()
            .map(|(node, separator)| {
                let piece = self.with_section_comments(*node, self.format_node(*node));
                separator
                    .map(|token| self.with_dropped_token_comments(token, piece.clone()))
                    .unwrap_or(piece)
            })
            .filter(|piece| piece.first_kind.is_some())
            .collect::<Vec<_>>();

        let mut header = prefix;
        if header.first_kind.is_some() {
            header = self.concat_with_separator(header, Doc::text(" "), open);
        } else {
            header = open;
        }
        if items.is_empty() {
            if !break_empty {
                return self.concat_with_separator(header, Doc::Nil, close);
            }
            return self.piece_for_node(node, Doc::concat([header.doc, Doc::HardLine, close.doc]));
        }

        let broken_items = if node.kind() == SyntaxKind::Block {
            self.join_block_units(&item_nodes, &items)
        } else {
            self.join_structural_units(&item_nodes, &items)
        };
        let flat_items = Doc::concat(items.iter().enumerate().flat_map(|(index, item)| {
            let separator = (index > 0).then_some(Doc::text(", "));
            separator
                .into_iter()
                .chain(std::iter::once(item.doc.clone()))
        }));
        let force_break = force_break || self.node_has_comments_or_associations(node);
        let braces = if force_break {
            Doc::concat([
                header.doc,
                Doc::concat([Doc::HardLine, broken_items]).indent(),
                Doc::HardLine,
                close.doc,
            ])
        } else {
            Doc::concat([
                header.doc,
                Doc::concat([Doc::SoftLine, Doc::if_break(broken_items, flat_items)]).indent(),
                Doc::SoftLine,
                close.doc,
            ])
            .group()
        };
        Piece {
            doc: braces,
            first_kind: header.first_kind,
            last_kind: close.last_kind,
            first_token: header.first_token,
            last_token: close.last_token,
            starts_with_leading_comment: header.starts_with_leading_comment,
            ends_with_break: close.ends_with_break,
        }
    }

    fn join_block_units(&self, nodes: &[SyntaxNodeRef<'a>], items: &[Piece<'a>]) -> Doc<'a> {
        debug_assert_eq!(nodes.len(), items.len());
        let mut docs = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                let lines = if self.blank_between_nodes(nodes[index - 1], nodes[index]) {
                    2
                } else {
                    1
                };
                self.push_separation(&mut docs, &items[index - 1], lines);
            }
            docs.push(item.doc.clone());
        }
        Doc::concat(docs)
    }

    fn join_structural_units(&self, nodes: &[SyntaxNodeRef<'a>], items: &[Piece<'a>]) -> Doc<'a> {
        debug_assert_eq!(nodes.len(), items.len());
        let mut docs = Vec::new();
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                let lines = if self.has_section_comments_after(nodes[index - 1]) {
                    2
                } else {
                    1
                };
                self.push_separation(&mut docs, &items[index - 1], lines);
            }
            docs.push(item.doc.clone());
        }
        Doc::concat(docs)
    }

    fn push_separation(&self, docs: &mut Vec<Doc<'a>>, previous: &Piece<'a>, lines: usize) {
        let already_present = usize::from(previous.ends_with_break);
        docs.extend(std::iter::repeat_n(
            Doc::HardLine,
            lines.saturating_sub(already_present),
        ));
    }

    fn format_list_node(
        &self,
        node: SyntaxNodeRef<'a>,
        allow_generated_trailing: bool,
    ) -> Piece<'a> {
        let elements = self.clean_elements(node.elements());
        let delimiter = elements.iter().enumerate().find_map(|(index, element)| {
            let kind = self.element_token_kind(*element)?;
            matches!(kind, TokenKind::LParen | TokenKind::LBracket).then_some((index, kind))
        });
        let Some((open_index, open_kind)) = delimiter else {
            return self.format_generic(node);
        };
        let close_kind = match open_kind {
            TokenKind::LParen => TokenKind::RParen,
            TokenKind::LBracket => TokenKind::RBracket,
            _ => unreachable!(),
        };
        let Some(close_index) = elements
            .iter()
            .rposition(|element| self.element_token_kind(*element) == Some(close_kind))
        else {
            return self.format_generic(node);
        };
        let prefix = self.join_pieces(self.element_pieces(&elements[..open_index]), node.kind());
        let open = self.format_element(elements[open_index]);
        let close = self.format_element(elements[close_index]);
        let (items, commas, trailing_comma) =
            self.split_comma_items(&elements[open_index + 1..close_index], node.kind());

        let mut opening = prefix;
        if opening.first_kind.is_some() {
            opening = self.concat_with_separator(opening, Doc::Nil, open);
        } else {
            opening = open;
        }
        if items.is_empty() {
            return self.concat_with_separator(opening, Doc::Nil, close);
        }

        let (content, content_ends_with_break) =
            self.format_list_content(&items, &commas, trailing_comma, allow_generated_trailing);
        let doc = Doc::concat([
            opening.doc,
            Doc::concat([Doc::SoftZero, content]).indent(),
            if content_ends_with_break {
                Doc::Nil
            } else {
                Doc::SoftZero
            },
            close.doc,
        ])
        .group();
        Piece {
            doc,
            first_kind: opening.first_kind,
            last_kind: close.last_kind,
            first_token: opening.first_token,
            last_token: close.last_token,
            starts_with_leading_comment: opening.starts_with_leading_comment,
            ends_with_break: close.ends_with_break,
        }
    }

    fn format_function_type(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let elements = self.clean_elements(node.elements());
        let expanded = elements
            .into_iter()
            .flat_map(|element| match element {
                SyntaxElement::Node(id)
                    if self.cst.node(id).kind() == SyntaxKind::FunctionTypeList =>
                {
                    self.clean_elements(self.cst.node(id).children())
                }
                other => vec![other],
            })
            .collect::<Vec<_>>();
        self.format_list_from_elements(node, &expanded, true)
    }

    fn format_function_type_item(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let elements = self.clean_elements(node.elements());
        let Some(modifier_index) = elements.iter().position(|element| {
            self.element_token_kind(*element).is_some_and(|kind| {
                matches!(kind, TokenKind::Ref | TokenKind::Mut | TokenKind::Var)
            })
        }) else {
            return self.format_generic(node);
        };
        let Some(type_node) =
            elements[modifier_index + 1..]
                .iter()
                .find_map(|element| match *element {
                    SyntaxElement::Node(id) => Some(self.cst.node_ref(id)),
                    SyntaxElement::Token(_) => None,
                })
        else {
            return self.format_generic(node);
        };
        let modifier = self.format_element(elements[modifier_index]);
        let value = self.format_node(type_node);
        let value_doc = if matches!(
            self.canonical_type_shape(type_node),
            TypeShape::Result | TypeShape::Union
        ) {
            Doc::concat([Doc::text("("), value.doc, Doc::text(")")])
        } else {
            value.doc
        };
        self.piece_for_node(node, Doc::concat([modifier.doc, Doc::text(" "), value_doc]))
    }

    fn format_list_from_elements(
        &self,
        node: SyntaxNodeRef<'a>,
        elements: &[SyntaxElement],
        allow_generated_trailing: bool,
    ) -> Piece<'a> {
        let delimiter_node = TemporaryElements { elements };
        self.format_list_elements(node.kind(), delimiter_node, allow_generated_trailing)
            .unwrap_or_else(|| self.format_generic(node))
    }

    fn format_list_elements(
        &self,
        parent: SyntaxKind,
        temporary: TemporaryElements<'_>,
        allow_generated_trailing: bool,
    ) -> Option<Piece<'a>> {
        let elements = temporary.elements;
        let (open_index, open_kind) =
            elements.iter().enumerate().find_map(|(index, element)| {
                let kind = self.element_token_kind(*element)?;
                matches!(kind, TokenKind::LParen | TokenKind::LBracket).then_some((index, kind))
            })?;
        let close_kind = if open_kind == TokenKind::LParen {
            TokenKind::RParen
        } else {
            TokenKind::RBracket
        };
        let close_index = elements
            .iter()
            .rposition(|element| self.element_token_kind(*element) == Some(close_kind))?;
        let prefix = self.join_pieces(self.element_pieces(&elements[..open_index]), parent);
        let open = self.format_element(elements[open_index]);
        let close = self.format_element(elements[close_index]);
        let suffix = self.join_pieces(self.element_pieces(&elements[close_index + 1..]), parent);
        let (items, commas, trailing_comma) =
            self.split_comma_items(&elements[open_index + 1..close_index], parent);
        let mut opening = prefix;
        opening = if opening.first_kind.is_some() {
            self.concat_with_separator(opening, Doc::Nil, open)
        } else {
            open
        };
        let list = if items.is_empty() {
            self.concat_with_separator(opening, Doc::Nil, close)
        } else {
            let (content, content_ends_with_break) =
                self.format_list_content(&items, &commas, trailing_comma, allow_generated_trailing);
            Piece {
                doc: Doc::concat([
                    opening.doc,
                    Doc::concat([Doc::SoftZero, content]).indent(),
                    if content_ends_with_break {
                        Doc::Nil
                    } else {
                        Doc::SoftZero
                    },
                    close.doc,
                ])
                .group(),
                first_kind: opening.first_kind,
                last_kind: close.last_kind,
                first_token: opening.first_token,
                last_token: close.last_token,
                starts_with_leading_comment: opening.starts_with_leading_comment,
                ends_with_break: close.ends_with_break,
            }
        };
        if suffix.first_kind.is_some() {
            let separator = self.spacing(list.last_kind, suffix.first_kind, parent);
            Some(self.concat_with_separator(list, separator, suffix))
        } else {
            Some(list)
        }
    }

    fn split_comma_items(
        &self,
        elements: &[SyntaxElement],
        parent: SyntaxKind,
    ) -> (Vec<ListItem<'a>>, Vec<Piece<'a>>, Option<Piece<'a>>) {
        let mut items = Vec::new();
        let mut commas = Vec::new();
        let mut segment = Vec::new();
        for element in elements {
            if self.element_token_kind(*element) == Some(TokenKind::Comma) {
                if !segment.is_empty() {
                    items.push(self.format_list_item(&segment, parent));
                    segment.clear();
                }
                let token = match *element {
                    SyntaxElement::Token(id) => self.cst.token_ref(id),
                    SyntaxElement::Node(_) => unreachable!("a comma is a token"),
                };
                let comma = self.format_element(*element);
                commas.push(self.with_token_section_comments(token, comma, true));
            } else {
                segment.push(*element);
            }
        }
        if !segment.is_empty() {
            items.push(self.format_list_item(&segment, parent));
        }
        let trailing = (!commas.is_empty() && commas.len() == items.len()).then(|| {
            commas
                .pop()
                .expect("equal nonzero list lengths contain a trailing comma")
        });
        (items, commas, trailing)
    }

    fn format_list_item(&self, elements: &[SyntaxElement], parent: SyntaxKind) -> ListItem<'a> {
        let last = self.last_significant_token_in_elements(elements);
        let trailing: &[CommentRun] = last
            .map(|token| self.comments.trailing[token.id().index() as usize].as_slice())
            .unwrap_or(&[]);
        let piece = if trailing.is_empty() {
            self.join_pieces(self.element_pieces(elements), parent)
        } else {
            let token = last.expect("a trailing comment has a preceding token");
            let previous = self.suppressed_trailing.replace(Some(token.id()));
            let piece = self.join_pieces(self.element_pieces(elements), parent);
            self.suppressed_trailing.set(previous);
            piece
        };
        let (trailing_comments, trailing_comments_break) = self.trailing_comment_runs_doc(trailing);
        ListItem {
            piece,
            trailing_comments: (!trailing.is_empty()).then_some(trailing_comments),
            trailing_comments_break,
        }
    }

    fn format_list_content(
        &self,
        items: &[ListItem<'a>],
        commas: &[Piece<'a>],
        trailing_comma: Option<Piece<'a>>,
        allow_generated_trailing: bool,
    ) -> (Doc<'a>, bool) {
        let mut content = Vec::new();
        let mut last_ends_with_break = false;
        for (index, item) in items.iter().enumerate() {
            last_ends_with_break = false;
            content.push(item.piece.doc.clone());
            let is_last = index + 1 == items.len();
            if is_last {
                let trailing = if let Some(comma) = trailing_comma.clone() {
                    last_ends_with_break = comma.ends_with_break;
                    Doc::if_break(comma.doc, Doc::Nil)
                } else if allow_generated_trailing {
                    Doc::if_break(Doc::text(","), Doc::Nil)
                } else {
                    Doc::Nil
                };
                content.push(trailing);
            } else {
                let comma = commas.get(index).cloned().unwrap_or_else(|| {
                    Piece::generated(Doc::text(","), TokenKind::Comma, TokenKind::Comma)
                });
                content.push(comma.doc);
                last_ends_with_break = comma.ends_with_break;
            }
            if let Some(comments) = &item.trailing_comments {
                content.push(comments.clone());
                last_ends_with_break = item.trailing_comments_break;
            }
            if !is_last && !last_ends_with_break {
                content.push(Doc::SoftLine);
            }
        }
        (Doc::concat(content), last_ends_with_break)
    }

    fn last_significant_token_in_elements(
        &self,
        elements: &[SyntaxElement],
    ) -> Option<SyntaxTokenRef<'a>> {
        elements.iter().rev().find_map(|element| match *element {
            SyntaxElement::Token(id) => {
                let token = self.cst.token_ref(id);
                is_significant(token.kind()).then_some(token)
            }
            SyntaxElement::Node(id) => self
                .cst
                .node_ref(id)
                .descendant_tokens()
                .filter(|token| is_significant(token.kind()))
                .last(),
        })
    }

    fn format_binary(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let Some(level) = self.binary_level(node) else {
            return self.format_generic(node);
        };
        let mut operands = Vec::new();
        let mut operators = Vec::new();
        self.collect_binary(node, level, &mut operands, &mut operators);
        if operands.is_empty() {
            return self.format_generic(node);
        }
        let first = operands.remove(0);
        let mut docs = vec![first.doc.clone()];
        let mut previous_ends_break = first.ends_with_break;
        for (operator, operand) in operators.into_iter().zip(operands) {
            if !previous_ends_break {
                docs.push(Doc::text(" "));
            }
            docs.push(operator.doc.clone());
            if !operator.ends_with_break {
                docs.push(Doc::concat([Doc::SoftLine, operand.doc.clone()]).indent());
            } else {
                docs.push(operand.doc.clone());
            }
            previous_ends_break = operand.ends_with_break;
        }
        self.piece_for_node(node, Doc::concat(docs).group())
    }

    fn collect_binary(
        &self,
        node: SyntaxNodeRef<'a>,
        level: u8,
        operands: &mut Vec<Piece<'a>>,
        operators: &mut Vec<Piece<'a>>,
    ) {
        let elements = self.clean_elements(node.elements());
        let Some(operator_index) = elements.iter().position(|element| {
            self.element_token_kind(*element)
                .is_some_and(is_binary_operator)
        }) else {
            operands.push(self.format_node(node));
            return;
        };
        let left = elements[..operator_index]
            .iter()
            .find_map(|element| match *element {
                SyntaxElement::Node(id) => Some(self.cst.node_ref(id)),
                SyntaxElement::Token(_) => None,
            });
        if let Some(left) = left {
            if left.kind() == SyntaxKind::BinaryExpr && self.binary_level(left) == Some(level) {
                self.collect_binary(left, level, operands, operators);
            } else {
                operands.push(self.format_node(left));
            }
        }
        operators.push(self.format_element(elements[operator_index]));
        let right = self.join_pieces(
            self.element_pieces(&elements[operator_index + 1..]),
            node.kind(),
        );
        operands.push(right);
    }

    fn binary_level(&self, node: SyntaxNodeRef<'a>) -> Option<u8> {
        node.child_tokens()
            .map(|token| token.kind())
            .find_map(binary_level)
    }

    fn format_assignment(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        self.format_break_after_operator(node, is_assignment_operator)
    }

    fn format_match_arm(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        self.format_break_after_operator(node, |kind| kind == TokenKind::FatArrow)
    }

    fn format_break_after_operator(
        &self,
        node: SyntaxNodeRef<'a>,
        predicate: impl Fn(TokenKind) -> bool,
    ) -> Piece<'a> {
        let elements = self.clean_elements(node.elements());
        let Some(index) = elements
            .iter()
            .position(|element| self.element_token_kind(*element).is_some_and(&predicate))
        else {
            return self.format_generic(node);
        };
        let left = self.join_pieces(self.element_pieces(&elements[..index]), node.kind());
        let operator = self.format_element(elements[index]);
        let right = self.join_pieces(self.element_pieces(&elements[index + 1..]), node.kind());
        let mut docs = vec![left.doc];
        if !left.ends_with_break {
            docs.push(Doc::text(" "));
        }
        docs.push(operator.doc);
        if !operator.ends_with_break {
            docs.push(Doc::concat([Doc::SoftLine, right.doc]).indent());
        } else {
            docs.push(right.doc);
        }
        self.piece_for_node(node, Doc::concat(docs).group())
    }

    fn format_postfix(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let mut base = None;
        let mut suffixes = Vec::new();
        self.collect_postfix(node, &mut base, &mut suffixes);
        let Some(mut base) = base else {
            return self.format_generic(node);
        };
        let mut continuation = Vec::new();
        let mut previous_question = false;
        for suffix in suffixes {
            let is_question = suffix.first_kind == Some(TokenKind::Question);
            if previous_question && is_question {
                if !continuation.is_empty() {
                    base.doc = Doc::concat([
                        base.doc,
                        Doc::concat(std::mem::take(&mut continuation)).indent(),
                    ])
                    .group();
                }
                base.doc = Doc::concat([Doc::text("("), base.doc, Doc::text(")")]);
            }
            if suffix.first_kind == Some(TokenKind::Dot) {
                continuation.push(Doc::SoftZero);
            }
            continuation.push(suffix.doc);
            previous_question = is_question;
        }
        if !continuation.is_empty() {
            base.doc = Doc::concat([base.doc, Doc::concat(continuation).indent()]).group();
        }
        self.piece_for_node(node, base.doc)
    }

    fn format_path_expression(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let (mut base, suffixes) = self.split_path_expression(node);
        if suffixes.is_empty() {
            return base;
        }
        let mut continuation = Vec::new();
        for suffix in suffixes {
            continuation.push(Doc::SoftZero);
            continuation.push(suffix.doc);
        }
        base.doc = Doc::concat([base.doc, Doc::concat(continuation).indent()]).group();
        self.piece_for_node(node, base.doc)
    }

    fn split_path_expression(&self, node: SyntaxNodeRef<'a>) -> (Piece<'a>, Vec<Piece<'a>>) {
        let elements = self.clean_elements(node.elements());
        let mut segments = Vec::new();
        let mut start = 0;
        for (index, element) in elements.iter().enumerate() {
            if self.element_token_kind(*element) == Some(TokenKind::Dot) {
                if index > start {
                    segments.push(
                        self.join_pieces(self.element_pieces(&elements[start..index]), node.kind()),
                    );
                }
                start = index;
            }
        }
        if start < elements.len() {
            segments.push(self.join_pieces(self.element_pieces(&elements[start..]), node.kind()));
        }
        if segments.is_empty() {
            return (self.format_generic(node), Vec::new());
        }
        let base = segments.remove(0);
        (base, segments)
    }

    fn collect_postfix(
        &self,
        node: SyntaxNodeRef<'a>,
        base: &mut Option<Piece<'a>>,
        suffixes: &mut Vec<Piece<'a>>,
    ) {
        let children = node.child_nodes().collect::<Vec<_>>();
        if let Some(left) = children.first().copied() {
            if left.kind() == SyntaxKind::PostfixExpr {
                self.collect_postfix(left, base, suffixes);
            } else if left.kind() == SyntaxKind::PathExpr {
                let (path_base, path_suffixes) = self.split_path_expression(left);
                *base = Some(path_base);
                suffixes.extend(path_suffixes);
            } else {
                *base = Some(self.format_node(left));
            }
        }
        suffixes.extend(
            children
                .into_iter()
                .skip(1)
                .map(|suffix| self.format_node(suffix)),
        );
    }

    fn format_prefix(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        let elements = self.clean_elements(node.elements());
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        if self.element_token_kind(elements[0]) == Some(TokenKind::Minus)
            && elements.get(1).is_some_and(|element| match *element {
                SyntaxElement::Node(id) => {
                    let child = self.cst.node_ref(id);
                    child.kind() == SyntaxKind::PrefixExpr
                        && child
                            .descendant_tokens()
                            .find(|token| is_significant(token.kind()))
                            .map(|token| token.kind())
                            == Some(TokenKind::Minus)
                }
                SyntaxElement::Token(_) => false,
            })
        {
            let operator = self.format_element(elements[0]);
            let operand = self.format_element(elements[1]);
            return self.piece_for_node(
                node,
                Doc::concat([operator.doc, Doc::text("("), operand.doc, Doc::text(")")]),
            );
        }
        self.format_generic(node)
    }

    fn format_union_type(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let operands = node.child_nodes().collect::<Vec<_>>();
        let operators = node
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Pipe)
            .collect::<Vec<_>>();
        if operators.is_empty() {
            return operands
                .first()
                .map(|operand| self.format_node(*operand))
                .unwrap_or_else(|| self.format_generic(node));
        }
        let operands = operands
            .into_iter()
            .map(|operand| {
                let mut piece = self.format_node(operand);
                if self.canonical_type_shape(operand) == TypeShape::Result {
                    piece.doc = Doc::concat([Doc::text("("), piece.doc, Doc::text(")")]);
                }
                piece
            })
            .collect::<Vec<_>>();
        let operators = operators
            .into_iter()
            .map(|operator| self.format_token(operator))
            .collect::<Vec<_>>();
        self.format_operator_chain(node, operands, operators)
    }

    fn format_result_type(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let bang = node
            .child_tokens()
            .find(|token| token.kind() == TokenKind::Bang);
        let Some(bang) = bang else {
            return node
                .child_nodes()
                .next()
                .map(|child| self.format_node(child))
                .unwrap_or_else(|| self.format_generic(node));
        };
        let operands = node.child_nodes().collect::<Vec<_>>();
        let starts_with_bang = node
            .descendant_tokens()
            .find(|token| is_significant(token.kind()))
            .is_some_and(|token| token.kind() == TokenKind::Bang);
        let (success, error) = if starts_with_bang {
            (None, operands.first().copied())
        } else {
            (operands.first().copied(), operands.get(1).copied())
        };
        let Some(error) = error else {
            return self.format_generic(node);
        };
        let error_piece = self.format_node(error);
        let error_doc = self.parenthesize_type_if_needed(
            error_piece.doc,
            self.canonical_type_shape(error),
            &[TypeShape::Result, TypeShape::Union],
        );
        let bang = self.format_token(bang);
        let Some(success) = success else {
            return self.piece_for_node(node, Doc::concat([bang.doc, error_doc]));
        };
        if self.type_node_is_named(success, "Unit") {
            return self.piece_for_node(node, Doc::concat([bang.doc, error_doc]));
        }
        let success_piece = self.format_node(success);
        let success_doc = self.parenthesize_type_if_needed(
            success_piece.doc,
            self.canonical_type_shape(success),
            &[TypeShape::Result, TypeShape::Union],
        );
        self.piece_for_node(
            node,
            Doc::concat([
                success_doc,
                Doc::text(" "),
                bang.doc,
                Doc::concat([Doc::SoftLine, error_doc]).indent(),
            ])
            .group(),
        )
    }

    fn format_optional_type(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let Some(value_node) = node.child_nodes().next() else {
            return self.format_generic(node);
        };
        let value = self.format_node(value_node);
        if !node
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Question)
        {
            return value;
        }
        let value_doc = self.parenthesize_type_if_needed(
            value.doc,
            self.canonical_type_shape(value_node),
            &[TypeShape::Optional, TypeShape::Result, TypeShape::Union],
        );
        self.piece_for_node(node, Doc::concat([value_doc, Doc::text("?")]))
    }

    fn format_operator_chain(
        &self,
        node: SyntaxNodeRef<'a>,
        mut operands: Vec<Piece<'a>>,
        operators: Vec<Piece<'a>>,
    ) -> Piece<'a> {
        if operands.len() != operators.len().saturating_add(1) {
            return self.format_generic(node);
        }
        let first = operands.remove(0);
        let mut docs = vec![first.doc];
        for (operator, operand) in operators.into_iter().zip(operands) {
            docs.push(Doc::text(" "));
            docs.push(operator.doc);
            docs.push(Doc::concat([Doc::SoftLine, operand.doc]).indent());
        }
        self.piece_for_node(node, Doc::concat(docs).group())
    }

    fn format_operator_sequence(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let operands = node
            .child_nodes()
            .map(|operand| self.format_node(operand))
            .collect::<Vec<_>>();
        let operators = node
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Plus)
            .map(|operator| self.format_token(operator))
            .collect::<Vec<_>>();
        if operators.is_empty() {
            return operands
                .into_iter()
                .next()
                .unwrap_or_else(|| self.format_generic(node));
        }
        self.format_operator_chain(node, operands, operators)
    }

    fn format_path_type(&self, node: SyntaxNodeRef<'a>) -> Piece<'a> {
        if self.node_has_comments_or_associations(node) {
            return self.format_generic(node);
        }
        let Some((name, arguments)) = self.long_type_constructor(node) else {
            return self.format_generic(node);
        };
        if name == "Option" {
            let args = arguments
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::TypeExpr)
                .collect::<Vec<_>>();
            if args.len() == 1 {
                let argument = self.format_node(args[0]);
                let argument_doc = self.parenthesize_type_if_needed(
                    argument.doc,
                    self.canonical_type_shape(args[0]),
                    &[TypeShape::Optional, TypeShape::Result, TypeShape::Union],
                );
                return self.piece_for_node(node, Doc::concat([argument_doc, Doc::text("?")]));
            }
        }
        if name == "Result" {
            let args = arguments
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::TypeExpr)
                .collect::<Vec<_>>();
            if args.len() == 2 {
                let success_is_unit = self.type_expr_is_named(args[0], "Unit");
                let success = self.format_node(args[0]);
                let error = self.format_node(args[1]);
                let error_doc = self.parenthesize_type_if_needed(
                    error.doc,
                    self.canonical_type_shape(args[1]),
                    &[TypeShape::Result, TypeShape::Union],
                );
                let doc = if success_is_unit {
                    Doc::concat([Doc::text("!"), error_doc])
                } else {
                    let success_doc = self.parenthesize_type_if_needed(
                        success.doc,
                        self.canonical_type_shape(args[0]),
                        &[TypeShape::Result, TypeShape::Union],
                    );
                    Doc::concat([
                        success_doc,
                        Doc::text(" !"),
                        Doc::concat([Doc::SoftLine, error_doc]).indent(),
                    ])
                    .group()
                };
                return self.piece_for_node(node, doc);
            }
        }
        self.format_generic(node)
    }

    fn long_type_constructor(
        &self,
        node: SyntaxNodeRef<'a>,
    ) -> Option<(&'a str, SyntaxNodeRef<'a>)> {
        if node.kind() != SyntaxKind::PathType {
            return None;
        }
        let path = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::TypePath)?;
        if path
            .child_tokens()
            .any(|token| token.kind() == TokenKind::Dot)
        {
            return None;
        }
        let mut identifiers = path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier);
        let identifier = identifiers.next()?;
        if identifiers.next().is_some() {
            return None;
        }
        let name = identifier.token().normalized_identifier()?;
        let arguments = path
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::GenericArgs)?;
        Some((name, arguments))
    }

    fn canonical_type_shape(&self, node: SyntaxNodeRef<'a>) -> TypeShape {
        match node.kind() {
            SyntaxKind::TypeExpr => node
                .child_nodes()
                .next()
                .map(|child| self.canonical_type_shape(child))
                .unwrap_or(TypeShape::Primary),
            SyntaxKind::UnionType => {
                if node
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Pipe)
                {
                    TypeShape::Union
                } else {
                    node.child_nodes()
                        .next()
                        .map(|child| self.canonical_type_shape(child))
                        .unwrap_or(TypeShape::Primary)
                }
            }
            SyntaxKind::ResultType => {
                if node
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Bang)
                {
                    TypeShape::Result
                } else {
                    node.child_nodes()
                        .next()
                        .map(|child| self.canonical_type_shape(child))
                        .unwrap_or(TypeShape::Primary)
                }
            }
            SyntaxKind::OptionalType => {
                if node
                    .child_tokens()
                    .any(|token| token.kind() == TokenKind::Question)
                {
                    TypeShape::Optional
                } else {
                    node.child_nodes()
                        .next()
                        .map(|child| self.canonical_type_shape(child))
                        .unwrap_or(TypeShape::Primary)
                }
            }
            SyntaxKind::PathType if !self.node_has_comments_or_associations(node) => self
                .long_type_constructor(node)
                .map(|(name, _)| match name {
                    "Option" => TypeShape::Optional,
                    "Result" => TypeShape::Result,
                    _ => TypeShape::Primary,
                })
                .unwrap_or(TypeShape::Primary),
            _ => TypeShape::Primary,
        }
    }

    fn parenthesize_type_if_needed(
        &self,
        doc: Doc<'a>,
        shape: TypeShape,
        required: &[TypeShape],
    ) -> Doc<'a> {
        if required.contains(&shape) {
            Doc::concat([Doc::text("("), doc, Doc::text(")")])
        } else {
            doc
        }
    }

    fn is_omittable_unit_outcome(&self, node: SyntaxNodeRef<'a>) -> bool {
        !self.node_has_comments_or_associations(node)
            && node
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypeExpr)
                .is_some_and(|type_expr| self.type_expr_is_named(type_expr, "Unit"))
    }

    fn type_expr_is_named(&self, node: SyntaxNodeRef<'a>, name: &str) -> bool {
        self.type_node_is_named(node, name)
    }

    fn type_node_is_named(&self, node: SyntaxNodeRef<'a>, name: &str) -> bool {
        let significant = node
            .descendant_tokens()
            .filter(|token| is_significant(token.kind()))
            .collect::<Vec<_>>();
        significant.len() == 1
            && significant[0].kind() == TokenKind::Identifier
            && significant[0].token().normalized_identifier() == Some(name)
    }

    fn element_pieces(&self, elements: &[SyntaxElement]) -> Vec<Piece<'a>> {
        elements
            .iter()
            .filter_map(|element| {
                let piece = self.format_element(*element);
                piece.first_kind.is_some().then_some(piece)
            })
            .collect()
    }

    fn format_element(&self, element: SyntaxElement) -> Piece<'a> {
        match element {
            SyntaxElement::Node(id) => self.format_node(self.cst.node_ref(id)),
            SyntaxElement::Token(id) => {
                let token = self.cst.token_ref(id);
                if is_ignored(token.kind()) {
                    Piece::nil()
                } else {
                    self.format_token(token)
                }
            }
        }
    }

    fn format_token(&self, token: SyntaxTokenRef<'a>) -> Piece<'a> {
        let index = token.id().index() as usize;
        let leading = self
            .comments
            .leading
            .get(index)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let trailing = if self.suppressed_trailing.get() == Some(token.id()) {
            &[][..]
        } else {
            self.comments
                .trailing
                .get(index)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        };
        let mut docs = self.comment_runs_doc(leading, true);
        docs.push(self.token_text(token));
        let (trailing_doc, ends_with_break) = self.trailing_comment_runs_doc(trailing);
        docs.push(trailing_doc);
        Piece {
            doc: Doc::concat(docs),
            first_kind: Some(token.kind()),
            last_kind: Some(token.kind()),
            first_token: Some(token.id()),
            last_token: Some(token.id()),
            starts_with_leading_comment: !leading.is_empty(),
            ends_with_break,
        }
    }

    fn trailing_comment_runs_doc(&self, trailing: &[CommentRun]) -> (Doc<'a>, bool) {
        let mut docs = Vec::new();
        let mut ends_with_break = false;
        for run in trailing {
            if ends_with_break {
                if run.blank_before {
                    docs.push(Doc::HardLine);
                }
            } else if run.inline_with_previous {
                let first_kind = self.cst.token(run.comments[0]).kind();
                docs.push(Doc::text(if first_kind == TokenKind::LineComment {
                    "  "
                } else {
                    " "
                }));
            } else {
                docs.push(Doc::HardLine);
                if run.blank_before {
                    docs.push(Doc::HardLine);
                }
                ends_with_break = true;
            }
            for (comment_index, comment) in run.comments.iter().enumerate() {
                let kind = self.cst.token(*comment).kind();
                let text = self.token_text(self.cst.token_ref(*comment));
                docs.push(text);
                let breaks_before_next = run.comments.get(comment_index + 1).is_some_and(|next| {
                    line_breaks(
                        self.source,
                        self.cst.token(*comment).range().end(),
                        self.cst.token(*next).range().start(),
                    ) > 0
                });
                let must_break = kind == TokenKind::LineComment
                    || breaks_before_next
                    || (comment_index + 1 == run.comments.len() && run.break_after);
                if must_break {
                    docs.push(Doc::HardLine);
                    ends_with_break = true;
                } else if comment_index + 1 < run.comments.len() {
                    docs.push(Doc::text(" "));
                    ends_with_break = false;
                }
            }
        }
        (Doc::concat(docs), ends_with_break)
    }

    fn comment_runs_doc(&self, runs: &[CommentRun], leading: bool) -> Vec<Doc<'a>> {
        let mut docs = Vec::new();
        for (run_index, run) in runs.iter().enumerate() {
            if leading && run.blank_before && run_index > 0 {
                docs.push(Doc::HardLine);
            }
            for comment in &run.comments {
                docs.push(self.token_text(self.cst.token_ref(*comment)));
                docs.push(Doc::HardLine);
            }
        }
        docs
    }

    fn with_section_comments(&self, node: SyntaxNodeRef<'a>, mut piece: Piece<'a>) -> Piece<'a> {
        let Some(last) = self.last_significant_token(node) else {
            return piece;
        };
        let runs = &self.comments.section_after[last.id().index() as usize];
        if runs.is_empty() {
            return piece;
        }
        let lines = if runs[0].blank_before { 2 } else { 1 };
        let mut docs = vec![piece.doc.clone()];
        self.push_separation(&mut docs, &piece, lines);
        docs.extend(self.comment_runs_doc(runs, true));
        piece.doc = Doc::concat(docs);
        piece.ends_with_break = true;
        piece
    }

    fn with_token_section_comments(
        &self,
        token: SyntaxTokenRef<'a>,
        mut piece: Piece<'a>,
        preserve_blank_after: bool,
    ) -> Piece<'a> {
        let runs = &self.comments.section_after[token.id().index() as usize];
        if runs.is_empty() {
            return piece;
        }
        let lines = if runs[0].blank_before { 2 } else { 1 };
        let mut docs = vec![piece.doc.clone()];
        self.push_separation(&mut docs, &piece, lines);
        docs.extend(self.comment_runs_doc(runs, true));
        if preserve_blank_after {
            docs.push(Doc::HardLine);
        }
        piece.doc = Doc::concat(docs);
        piece.ends_with_break = true;
        piece
    }

    fn with_dropped_token_comments(
        &self,
        token: SyntaxTokenRef<'a>,
        mut piece: Piece<'a>,
    ) -> Piece<'a> {
        let trailing = &self.comments.trailing[token.id().index() as usize];
        let sections = &self.comments.section_after[token.id().index() as usize];
        if trailing.is_empty() && sections.is_empty() {
            return piece;
        }
        let mut docs = vec![piece.doc.clone()];
        let (trailing_doc, trailing_break) = self.trailing_comment_runs_doc(trailing);
        docs.push(trailing_doc);
        piece.ends_with_break = piece.ends_with_break || trailing_break;
        if !sections.is_empty() {
            let lines = if sections[0].blank_before { 2 } else { 1 };
            self.push_separation(&mut docs, &piece, lines);
            docs.extend(self.comment_runs_doc(sections, true));
            docs.push(Doc::HardLine);
            piece.ends_with_break = true;
        }
        piece.doc = Doc::concat(docs);
        piece
    }

    fn last_significant_token(&self, node: SyntaxNodeRef<'a>) -> Option<SyntaxTokenRef<'a>> {
        node.descendant_tokens()
            .filter(|token| is_significant(token.kind()))
            .last()
    }

    fn has_section_comments_after(&self, node: SyntaxNodeRef<'a>) -> bool {
        self.last_significant_token(node).is_some_and(|token| {
            !self.comments.section_after[token.id().index() as usize].is_empty()
        })
    }

    fn token_text(&self, token: SyntaxTokenRef<'a>) -> Doc<'a> {
        if token.kind() == TokenKind::Identifier
            && let Some(normalized) = token.token().normalized_identifier()
        {
            return Doc::text(normalized);
        }
        let range = token.range();
        let text = &self.source[range.start() as usize..range.end() as usize];
        if text.contains("\r\n") {
            Doc::text(Cow::Owned(text.replace("\r\n", "\n")))
        } else {
            Doc::text(text)
        }
    }

    fn join_pieces(&self, pieces: Vec<Piece<'a>>, parent: SyntaxKind) -> Piece<'a> {
        let mut iterator = pieces.into_iter();
        let Some(mut combined) = iterator.next() else {
            return Piece::nil();
        };
        for next in iterator {
            let separator = if combined.ends_with_break {
                Doc::Nil
            } else if next.starts_with_leading_comment {
                Doc::HardLine
            } else {
                self.spacing(combined.last_kind, next.first_kind, parent)
            };
            combined = self.concat_with_separator(combined, separator, next);
        }
        combined
    }

    fn concat_with_separator(
        &self,
        left: Piece<'a>,
        separator: Doc<'a>,
        right: Piece<'a>,
    ) -> Piece<'a> {
        Piece {
            doc: Doc::concat([left.doc, separator, right.doc]),
            first_kind: left.first_kind.or(right.first_kind),
            last_kind: right.last_kind.or(left.last_kind),
            first_token: left.first_token.or(right.first_token),
            last_token: right.last_token.or(left.last_token),
            starts_with_leading_comment: left.starts_with_leading_comment
                || (left.first_kind.is_none() && right.starts_with_leading_comment),
            ends_with_break: right.ends_with_break
                || (right.last_kind.is_none() && left.ends_with_break),
        }
    }

    fn spacing(
        &self,
        left: Option<TokenKind>,
        right: Option<TokenKind>,
        parent: SyntaxKind,
    ) -> Doc<'a> {
        let (Some(left), Some(right)) = (left, right) else {
            return Doc::Nil;
        };
        if parent == SyntaxKind::SliceSpec
            && (left == TokenKind::Colon || right == TokenKind::Colon)
        {
            return Doc::Nil;
        }
        if is_spaced_operator(left) {
            if (left == TokenKind::Bang && parent == SyntaxKind::ResultType)
                || (matches!(left, TokenKind::Minus | TokenKind::Tilde)
                    && parent == SyntaxKind::PrefixExpr)
            {
                return Doc::Nil;
            }
            return Doc::text(" ");
        }
        if is_spaced_operator(right) {
            if matches!(left, TokenKind::LParen | TokenKind::LBracket)
                && matches!(right, TokenKind::Minus | TokenKind::Tilde | TokenKind::Bang)
            {
                return Doc::Nil;
            }
            return Doc::text(" ");
        }
        if matches!(
            right,
            TokenKind::RParen | TokenKind::RBracket | TokenKind::Comma
        ) || matches!(
            left,
            TokenKind::LParen | TokenKind::LBracket | TokenKind::Dot
        ) || matches!(
            right,
            TokenKind::Dot | TokenKind::Question | TokenKind::Colon
        ) || left == TokenKind::Ellipsis
        {
            return Doc::Nil;
        }
        if left == TokenKind::Comma || left == TokenKind::Colon {
            return Doc::text(" ");
        }
        if right == TokenKind::LBrace || left == TokenKind::RBrace {
            return Doc::text(" ");
        }
        if left.is_keyword()
            && matches!(right, TokenKind::LParen | TokenKind::LBracket)
            && !matches!(
                left,
                TokenKind::Fn | TokenKind::Some | TokenKind::Ok | TokenKind::Err
            )
        {
            return Doc::text(" ");
        }
        if right == TokenKind::LParen || right == TokenKind::LBracket {
            return Doc::Nil;
        }
        if word_like(left) && word_like(right) {
            return Doc::text(" ");
        }
        Doc::Nil
    }

    fn clean_elements(&self, elements: &[SyntaxElement]) -> Vec<SyntaxElement> {
        elements
            .iter()
            .copied()
            .filter(|element| match *element {
                SyntaxElement::Node(_) => true,
                SyntaxElement::Token(id) => !is_ignored(self.cst.token(id).kind()),
            })
            .collect()
    }

    fn element_token_kind(&self, element: SyntaxElement) -> Option<TokenKind> {
        match element {
            SyntaxElement::Token(id) => Some(self.cst.token(id).kind()),
            SyntaxElement::Node(_) => None,
        }
    }

    fn piece_for_node(&self, node: SyntaxNodeRef<'a>, doc: Doc<'a>) -> Piece<'a> {
        let mut tokens = node
            .descendant_tokens()
            .filter(|token| !is_ignored(token.kind()));
        let first = tokens.next();
        let last = tokens.last().or(first);
        Piece {
            doc,
            first_kind: first.map(SyntaxTokenRef::kind),
            last_kind: last.map(SyntaxTokenRef::kind),
            first_token: first.map(SyntaxTokenRef::id),
            last_token: last.map(SyntaxTokenRef::id),
            starts_with_leading_comment: first.is_some_and(|token| {
                !self.comments.leading[token.id().index() as usize].is_empty()
            }),
            ends_with_break: last.is_some_and(|token| {
                self.comments.trailing[token.id().index() as usize]
                    .iter()
                    .any(|run| run.break_after)
            }),
        }
    }

    fn node_has_comments(&self, node: SyntaxNodeRef<'a>) -> bool {
        node.descendant_tokens()
            .any(|token| is_comment(token.kind()))
    }

    fn node_has_comments_or_associations(&self, node: SyntaxNodeRef<'a>) -> bool {
        self.node_has_comments(node)
            || node
                .descendant_tokens()
                .filter(|token| is_significant(token.kind()))
                .any(|token| {
                    let index = token.id().index() as usize;
                    !self.comments.leading[index].is_empty()
                        || !self.comments.trailing[index].is_empty()
                        || !self.comments.section_after[index].is_empty()
                })
    }

    fn import_key(&self, node: SyntaxNodeRef<'a>) -> (String, Option<String>, u32) {
        let mut path = String::new();
        let mut alias = None;
        let mut after_as = false;
        for token in node.descendant_tokens() {
            match token.kind() {
                TokenKind::As => after_as = true,
                TokenKind::Identifier => {
                    let normalized = token.token().normalized_identifier().unwrap_or_default();
                    if after_as {
                        alias = Some(normalized.to_owned());
                    } else {
                        if !path.is_empty() {
                            path.push('.');
                        }
                        path.push_str(normalized);
                    }
                }
                _ => {}
            }
        }
        (path, alias, node.range().start())
    }

    fn top_level_blank_between(&self, left: SyntaxNodeRef<'a>, right: SyntaxNodeRef<'a>) -> bool {
        if is_declaration(left.kind()) || is_declaration(right.kind()) {
            return true;
        }
        self.blank_between_nodes(left, right)
    }

    fn blank_between_nodes(&self, left: SyntaxNodeRef<'a>, right: SyntaxNodeRef<'a>) -> bool {
        let mut left_end = left.range().end();
        if let Some(last) = self.last_significant_token(left) {
            left_end = last.range().end();
            let sections = &self.comments.section_after[last.id().index() as usize];
            if let Some(run) = sections.last() {
                return run.blank_after;
            }
            let trailing = &self.comments.trailing[last.id().index() as usize];
            if let Some(run) = trailing.last() {
                return run.blank_after;
            }
        }
        let mut right_start = right.range().start();
        if let Some(first) = right
            .descendant_tokens()
            .find(|token| is_significant(token.kind()))
        {
            right_start = first.range().start();
            let leading = &self.comments.leading[first.id().index() as usize];
            if let Some(run) = leading.first() {
                return run.blank_before;
            }
        }
        line_breaks(self.source, left_end, right_start) > 1
    }
}

impl Piece<'_> {
    fn with_group(mut self) -> Self {
        self.doc = self.doc.group();
        self
    }
}

struct TemporaryElements<'a> {
    elements: &'a [SyntaxElement],
}

fn line_breaks(source: &str, start: u32, end: u32) -> usize {
    source.as_bytes()[start as usize..end as usize]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
}

fn is_comment(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LineComment | TokenKind::DocComment | TokenKind::BlockComment
    )
}

fn is_significant(kind: TokenKind) -> bool {
    !kind.is_trivia() && !matches!(kind, TokenKind::Nl | TokenKind::Eof)
}

fn is_ignored(kind: TokenKind) -> bool {
    kind.is_trivia() || matches!(kind, TokenKind::Nl | TokenKind::Eof)
}

fn word_like(kind: TokenKind) -> bool {
    kind == TokenKind::Identifier
        || kind.is_keyword()
        || matches!(
            kind,
            TokenKind::IntegerLiteral
                | TokenKind::FloatLiteral
                | TokenKind::CharLiteral
                | TokenKind::RawStringLiteral
                | TokenKind::RawMultilineStringLiteral
        )
}

fn is_spaced_operator(kind: TokenKind) -> bool {
    is_binary_operator(kind)
        || is_assignment_operator(kind)
        || matches!(kind, TokenKind::FatArrow | TokenKind::Eq | TokenKind::Bang)
}

fn is_binary_operator(kind: TokenKind) -> bool {
    binary_level(kind).is_some()
}

fn binary_level(kind: TokenKind) -> Option<u8> {
    Some(match kind {
        TokenKind::With => 1,
        TokenKind::Or => 2,
        TokenKind::And => 3,
        TokenKind::EqEq | TokenKind::BangEq => 4,
        TokenKind::Less
        | TokenKind::LessEq
        | TokenKind::Greater
        | TokenKind::GreaterEq
        | TokenKind::In => 5,
        TokenKind::DotDot | TokenKind::DotDotEq => 6,
        TokenKind::Pipe => 7,
        TokenKind::Caret => 8,
        TokenKind::Amp => 9,
        TokenKind::Shl | TokenKind::Shr => 10,
        TokenKind::Plus | TokenKind::Minus => 11,
        TokenKind::Star | TokenKind::Slash | TokenKind::Percent => 12,
        _ => return None,
    })
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

fn is_declaration(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::ConstDecl
            | SyntaxKind::TypeDecl
            | SyntaxKind::AliasDecl
            | SyntaxKind::EnumDecl
            | SyntaxKind::TraitDecl
            | SyntaxKind::ImplDecl
            | SyntaxKind::FunctionDecl
            | SyntaxKind::FunctionSignature
    )
}
