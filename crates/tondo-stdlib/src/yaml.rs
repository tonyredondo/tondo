//! Bounded YAML 1.2 Core owner for the hosted standard-library boundary.
//!
//! This module intentionally implements a small, deterministic subset.  It
//! has no ambient lookup, custom tags, includes or execution hooks.  Parsing
//! is driven by explicit source-line/flow state and every materialisation is
//! checked against the published limits before a value is returned.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;

use crate::serialization::{
    self, Decode, Decoder, Encode, Encoder, Event, SerializationError, Yaml as YamlCodec,
};

/// Codec identity used by the common typed serialization protocol.
pub type Yaml = YamlCodec;

#[derive(Debug, Clone, PartialEq)]
pub struct YamlMember {
    pub key: String,
    pub value: YamlValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum YamlValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Array(Vec<YamlValue>),
    Object(Vec<YamlMember>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YamlValueView<'a> {
    input: &'a [u8],
    options: YamlOptions,
}

impl<'a> YamlValueView<'a> {
    pub fn bytes(self) -> &'a [u8] {
        self.input
    }

    pub fn clone_value(self) -> Result<YamlValue, YamlError> {
        parse_with_options(self.input, self.options)
    }
}

pub type ValueView<'a> = YamlValueView<'a>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YamlTag {
    Null,
    Bool,
    Int,
    Float,
    Str,
    Binary,
    Seq,
    Map,
}

#[derive(Debug, Clone, PartialEq)]
pub enum YamlScalar {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum YamlEvent {
    StreamStart,
    DocumentStart,
    DocumentEnd,
    Scalar(YamlScalar),
    SequenceStart(Option<String>),
    SequenceEnd,
    MappingStart(Option<String>),
    MappingKey,
    MappingEnd,
    Anchor(String),
    Alias(String),
    Tag(YamlTag),
    StreamEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YamlLimits {
    pub max_input_bytes: usize,
    pub max_documents: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_expanded_nodes: usize,
    pub max_aliases: usize,
    pub max_scalar_bytes: usize,
    pub max_collection_entries: usize,
    pub max_anchor_name_bytes: usize,
}

impl Default for YamlLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_documents: 256,
            max_depth: 256,
            max_nodes: 1_048_576,
            max_expanded_nodes: 4_194_304,
            max_aliases: 65_536,
            max_scalar_bytes: 64 * 1024 * 1024,
            max_collection_entries: 1_048_576,
            max_anchor_name_bytes: 256,
        }
    }
}

impl YamlLimits {
    pub fn defaults() -> Self {
        Self::default()
    }

    pub fn create(
        max_input_bytes: usize,
        max_documents: usize,
        max_depth: usize,
        max_nodes: usize,
        max_expanded_nodes: usize,
        max_aliases: usize,
        max_scalar_bytes: usize,
        max_collection_entries: usize,
        max_anchor_name_bytes: usize,
    ) -> Result<Self, YamlError> {
        let limits = Self {
            max_input_bytes,
            max_documents,
            max_depth,
            max_nodes,
            max_expanded_nodes,
            max_aliases,
            max_scalar_bytes,
            max_collection_entries,
            max_anchor_name_bytes,
        };
        if limits.valid() {
            Ok(limits)
        } else {
            Err(YamlError::at_zero(YamlErrorKind::InvalidLimit))
        }
    }

    fn valid(self) -> bool {
        self.max_input_bytes > 0
            && self.max_documents > 0
            && self.max_depth > 0
            && self.max_nodes > 0
            && self.max_expanded_nodes >= self.max_nodes
            && self.max_aliases > 0
            && self.max_scalar_bytes > 0
            && self.max_collection_entries > 0
            && self.max_anchor_name_bytes > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YamlOptions {
    pub limits: YamlLimits,
}

impl Default for YamlOptions {
    fn default() -> Self {
        Self {
            limits: YamlLimits::default(),
        }
    }
}

impl YamlOptions {
    pub fn defaults() -> Self {
        Self::default()
    }

    pub fn create(limits: YamlLimits) -> Self {
        Self { limits }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YamlErrorKind {
    InvalidLimit,
    InvalidUtf8,
    InvalidDirective,
    InvalidDocument,
    InvalidIndentation,
    InvalidScalar,
    InvalidEscape,
    InvalidTag,
    InvalidAnchor,
    UndefinedAlias,
    AliasCycle,
    AliasLimit,
    MergeKeyForbidden,
    DuplicateKey,
    NonStringKey,
    NumberOutOfRange,
    NonFiniteNumber,
    InvalidBinary,
    DepthLimit,
    NodeLimit,
    ExpandedNodeLimit,
    ScalarLimit,
    CollectionLimit,
    DocumentLimit,
    TypeMismatch,
    MissingField,
    UnknownField,
    UnexpectedEvent,
    TrailingDocument,
    Io(String),
    Closed,
    NoProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YamlError {
    pub kind: YamlErrorKind,
    pub offset: usize,
    pub line: usize,
    pub column: usize,
    pub path: Vec<YamlPathSegment>,
}

impl YamlError {
    fn at_zero(kind: YamlErrorKind) -> Self {
        Self {
            kind,
            offset: 0,
            line: 1,
            column: 1,
            path: Vec::new(),
        }
    }

    fn at(kind: YamlErrorKind, input: &[u8], offset: usize) -> Self {
        let offset = offset.min(input.len());
        let mut line = 1;
        let mut column = 1;
        for byte in &input[..offset] {
            if *byte == b'\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        Self {
            kind,
            offset,
            line,
            column,
            path: Vec::new(),
        }
    }

    fn with_path(mut self, path: &[YamlPathSegment]) -> Self {
        self.path = path.to_vec();
        self
    }
}

impl fmt::Display for YamlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "YAML {:?} at {}:{}",
            self.kind, self.line, self.column
        )
    }
}

impl std::error::Error for YamlError {}

impl From<SerializationError> for YamlError {
    fn from(error: SerializationError) -> Self {
        let kind = match error {
            SerializationError::EndOfInput => YamlErrorKind::UnexpectedEvent,
            SerializationError::LimitExceeded => YamlErrorKind::NodeLimit,
            SerializationError::DuplicateField => YamlErrorKind::DuplicateKey,
            SerializationError::MissingField => YamlErrorKind::MissingField,
            SerializationError::UnknownField => YamlErrorKind::UnknownField,
            SerializationError::TypeMismatch => YamlErrorKind::TypeMismatch,
            SerializationError::UnexpectedEvent
            | SerializationError::UnbalancedContainer
            | SerializationError::InvalidContainerLength => YamlErrorKind::UnexpectedEvent,
        };
        Self::at_zero(kind)
    }
}

#[derive(Debug, Clone)]
struct SourceLine {
    offset: usize,
    text: String,
}

#[derive(Debug, Clone)]
enum Node {
    Scalar {
        value: YamlScalar,
        spelling: String,
        tag: Option<YamlTag>,
        anchor: Option<String>,
    },
    Sequence {
        values: Vec<Node>,
        tag: Option<YamlTag>,
        anchor: Option<String>,
    },
    Mapping {
        entries: Vec<(Node, Node)>,
        tag: Option<YamlTag>,
        anchor: Option<String>,
    },
    Alias(String),
}

struct Parser {
    input: Vec<u8>,
    lines: Vec<SourceLine>,
    index: usize,
    options: YamlOptions,
    anchors: HashMap<String, Node>,
    in_progress: Vec<String>,
    nodes: usize,
}

impl Parser {
    fn new(input: &[u8], options: YamlOptions) -> Result<Self, YamlError> {
        if !options.limits.valid() {
            return Err(YamlError::at_zero(YamlErrorKind::InvalidLimit));
        }
        if input.len() > options.limits.max_input_bytes {
            return Err(YamlError::at_zero(YamlErrorKind::NodeLimit));
        }
        let text = std::str::from_utf8(input)
            .map_err(|_| YamlError::at(YamlErrorKind::InvalidUtf8, input, 0))?;
        let mut lines = Vec::new();
        let mut offset = 0;
        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            lines.push(SourceLine {
                offset,
                text: line.to_owned(),
            });
            offset = offset.saturating_add(raw.len()).saturating_add(1);
        }
        Ok(Self {
            input: input.to_vec(),
            lines,
            index: 0,
            options,
            anchors: HashMap::new(),
            in_progress: Vec::new(),
            nodes: 0,
        })
    }

    fn parse_stream(mut self) -> Result<Vec<(Node, HashMap<String, Node>)>, YamlError> {
        let mut documents = Vec::new();
        while self.skip_ignored() {
            let explicit_start = self.current_content() == Some("---");
            if explicit_start {
                self.index += 1;
                self.skip_ignored();
                if !self.has_significant() {
                    documents.push((self.null_node(0)?, HashMap::new()));
                    continue;
                }
            }
            if self
                .current_content()
                .is_some_and(|content| content.starts_with('%'))
            {
                return Err(self.error(YamlErrorKind::InvalidDirective));
            }
            self.anchors.clear();
            self.in_progress.clear();
            self.nodes = 0;
            let root = if self.has_significant() {
                let indent = self.current_indent()?;
                self.parse_node(indent)?
            } else {
                self.null_node(0)?
            };
            documents.push((root, self.anchors.clone()));
            self.skip_ignored();
            if self.current_content() == Some("...") {
                self.index += 1;
                self.skip_ignored();
            }
            if self.has_significant() && self.current_content() != Some("---") {
                return Err(self.error(YamlErrorKind::TrailingDocument));
            }
            if documents.len() > self.options.limits.max_documents {
                return Err(self.error(YamlErrorKind::DocumentLimit));
            }
            if !explicit_start && self.has_significant() {
                continue;
            }
        }
        if documents.is_empty() {
            documents.push((self.null_node(0)?, HashMap::new()));
        }
        Ok(documents)
    }

    fn has_significant(&self) -> bool {
        let mut index = self.index;
        while let Some(line) = self.lines.get(index) {
            let stripped = strip_comment(&line.text);
            let content = stripped.trim();
            if !content.is_empty() {
                return true;
            }
            index += 1;
        }
        false
    }

    fn skip_ignored(&mut self) -> bool {
        while let Some(line) = self.lines.get(self.index) {
            let stripped = strip_comment(&line.text);
            let content = stripped.trim();
            if content.is_empty() {
                self.index += 1;
            } else {
                return true;
            }
        }
        false
    }

    fn current_content(&self) -> Option<&str> {
        self.lines
            .get(self.index)
            .map(|line| strip_comment(&line.text).trim())
    }

    fn current_indent(&self) -> Result<usize, YamlError> {
        let line = self
            .lines
            .get(self.index)
            .ok_or_else(|| self.error(YamlErrorKind::InvalidDocument))?;
        let spaces = line.text.bytes().take_while(|byte| *byte == b' ').count();
        if line.text.as_bytes().get(spaces) == Some(&b'\t') {
            return Err(self.error(YamlErrorKind::InvalidIndentation));
        }
        Ok(spaces)
    }

    fn parse_node(&mut self, indent: usize) -> Result<Node, YamlError> {
        if self.nodes >= self.options.limits.max_nodes {
            return Err(self.error(YamlErrorKind::NodeLimit));
        }
        let actual = self.current_indent()?;
        if actual < indent {
            return self.null_node(self.current_offset());
        }
        if actual > indent {
            return Err(self.error(YamlErrorKind::InvalidIndentation));
        }
        let content = self.current_content().unwrap_or_default().to_owned();
        if is_sequence_marker(&content) {
            return self.parse_sequence(indent);
        }
        if split_mapping(&content).is_some() {
            return self.parse_mapping(indent);
        }
        let offset = self.current_offset();
        self.index += 1;
        self.parse_value_text(&content, indent, offset)
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Node, YamlError> {
        let mut values = Vec::new();
        while self.skip_ignored() {
            let actual = self.current_indent()?;
            let content = self.current_content().unwrap_or_default().to_owned();
            if actual != indent || !is_sequence_marker(&content) {
                break;
            }
            if values.len() >= self.options.limits.max_collection_entries {
                return Err(self.error(YamlErrorKind::CollectionLimit));
            }
            let offset = self.current_offset();
            let rest = content.strip_prefix('-').unwrap_or_default().trim_start();
            self.index += 1;
            if rest.is_empty() {
                self.skip_ignored();
                let value = if self.has_significant() && self.current_indent()? > indent {
                    self.parse_node(self.current_indent()?)?
                } else {
                    self.null_node(offset)?
                };
                values.push(value);
            } else if split_mapping(rest).is_some() {
                values.push(self.parse_sequence_mapping(indent, rest, offset)?);
            } else {
                values.push(self.parse_value_text(rest, indent + 1, offset)?);
            }
        }
        self.bump_node()?;
        Ok(Node::Sequence {
            values,
            tag: None,
            anchor: None,
        })
    }

    fn parse_sequence_mapping(
        &mut self,
        sequence_indent: usize,
        first: &str,
        offset: usize,
    ) -> Result<Node, YamlError> {
        let mut entries = Vec::new();
        self.parse_mapping_entry(first, sequence_indent + 2, offset, &mut entries)?;
        loop {
            if !self.skip_ignored() {
                break;
            }
            let actual = self.current_indent()?;
            if actual <= sequence_indent {
                break;
            }
            let content = self.current_content().unwrap_or_default().to_owned();
            if split_mapping(&content).is_none() {
                break;
            }
            let entry_offset = self.current_offset();
            self.index += 1;
            self.parse_mapping_entry_after_consumed(&content, actual, entry_offset, &mut entries)?;
        }
        self.bump_node()?;
        Ok(Node::Mapping {
            entries,
            tag: None,
            anchor: None,
        })
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Node, YamlError> {
        let mut entries = Vec::new();
        while self.skip_ignored() {
            let actual = self.current_indent()?;
            let content = self.current_content().unwrap_or_default().to_owned();
            if actual != indent || split_mapping(&content).is_none() {
                break;
            }
            let offset = self.current_offset();
            self.index += 1;
            self.parse_mapping_entry_after_consumed(&content, indent, offset, &mut entries)?;
            if entries.len() > self.options.limits.max_collection_entries {
                return Err(self.error(YamlErrorKind::CollectionLimit));
            }
        }
        self.bump_node()?;
        Ok(Node::Mapping {
            entries,
            tag: None,
            anchor: None,
        })
    }

    fn parse_mapping_entry(
        &mut self,
        content: &str,
        indent: usize,
        offset: usize,
        entries: &mut Vec<(Node, Node)>,
    ) -> Result<(), YamlError> {
        self.parse_mapping_entry_after_consumed(content, indent, offset, entries)
    }

    fn parse_mapping_entry_after_consumed(
        &mut self,
        content: &str,
        indent: usize,
        offset: usize,
        entries: &mut Vec<(Node, Node)>,
    ) -> Result<(), YamlError> {
        let (key_text, value_text) =
            split_mapping(content).ok_or_else(|| self.error(YamlErrorKind::InvalidDocument))?;
        let key = self.parse_key(key_text.trim(), offset)?;
        let value = if value_text.trim().is_empty() {
            self.skip_ignored();
            if self.has_significant() && self.current_indent()? > indent {
                self.parse_node(self.current_indent()?)?
            } else {
                self.null_node(offset)?
            }
        } else {
            self.parse_value_text(value_text.trim(), indent + 1, offset)?
        };
        if entries
            .iter()
            .any(|(candidate, _)| node_key(candidate) == node_key(&key))
        {
            return Err(self.error(YamlErrorKind::DuplicateKey));
        }
        entries.push((key, value));
        Ok(())
    }

    fn parse_key(&mut self, text: &str, offset: usize) -> Result<Node, YamlError> {
        let key = self.parse_value_text(text, 0, offset)?;
        if text.trim() == "<<" {
            return Err(self.error(YamlErrorKind::MergeKeyForbidden));
        }
        if !matches!(
            key,
            Node::Scalar {
                value: YamlScalar::Text(_),
                ..
            }
        ) {
            return Err(self.error(YamlErrorKind::NonStringKey));
        }
        Ok(key)
    }

    fn parse_value_text(
        &mut self,
        text: &str,
        indent: usize,
        offset: usize,
    ) -> Result<Node, YamlError> {
        let (tag, anchor, remainder) = self.parse_prefixes(text, offset)?;
        let node = if remainder.is_empty() {
            self.skip_ignored();
            if self.has_significant() && self.current_indent()? > indent {
                let name = anchor.clone();
                if let Some(name) = &name {
                    self.begin_anchor(name)?;
                }
                let child = self.parse_node(self.current_indent()?)?;
                let child = apply_container_tag(child, tag, anchor.clone());
                if let Some(name) = &name {
                    self.end_anchor(name, &child)?;
                }
                child
            } else {
                let child = self.null_node(offset)?;
                let child = apply_container_tag(child, tag, anchor.clone());
                if let Some(name) = anchor {
                    self.register_anchor(name, &child)?;
                }
                child
            }
        } else if remainder.starts_with('|') || remainder.starts_with('>') {
            let literal = remainder.as_bytes()[0] == b'|';
            let value = self.parse_block_scalar(indent, &remainder, offset)?;
            let scalar = Node::Scalar {
                value: YamlScalar::Text(value.clone()),
                spelling: value,
                tag,
                anchor: anchor.clone(),
            };
            if let Some(name) = anchor {
                self.register_anchor(name, &scalar)?;
            }
            self.bump_node()?;
            let _ = literal;
            scalar
        } else if remainder.starts_with('*') && remainder[1..].find(char::is_whitespace).is_none() {
            let name = remainder[1..].to_owned();
            if !valid_anchor_name(&name, self.options.limits.max_anchor_name_bytes) {
                return Err(self.error(YamlErrorKind::InvalidAnchor));
            }
            if self.in_progress.iter().any(|candidate| candidate == &name) {
                return Err(self.error(YamlErrorKind::AliasCycle));
            }
            let alias = Node::Alias(name);
            if let Some(anchor_name) = anchor {
                self.register_anchor(anchor_name, &alias)?;
            }
            self.bump_node()?;
            alias
        } else if remainder.starts_with('[') || remainder.starts_with('{') {
            let mut flow = FlowParser::new(self, &remainder, offset);
            let mut value = flow.parse_value()?;
            flow.skip_space();
            if flow.position() != remainder.len() {
                return Err(self.error(YamlErrorKind::InvalidDocument));
            }
            let outer_anchor = anchor.clone();
            if tag.is_some() || anchor.is_some() {
                value = apply_container_tag(value, tag, anchor);
            }
            if let Some(name) = outer_anchor {
                self.register_anchor(name, &value)?;
            }
            value
        } else if remainder.starts_with('\'') || remainder.starts_with('"') {
            let mut flow = FlowParser::new(self, &remainder, offset);
            let value = flow.parse_value()?;
            flow.skip_space();
            if flow.position() != remainder.len() {
                return Err(self.error(YamlErrorKind::InvalidDocument));
            }
            if tag.is_some() || anchor.is_some() {
                let value = apply_container_tag(value, tag, anchor.clone());
                if let Some(name) = anchor {
                    self.register_anchor(name, &value)?;
                }
                value
            } else {
                value
            }
        } else {
            let (value, spelling) =
                scalar_from_text(&remainder, false, self.options.limits.max_scalar_bytes)
                    .map_err(|kind| self.error(kind))?;
            let scalar = Node::Scalar {
                value,
                spelling,
                tag,
                anchor: anchor.clone(),
            };
            if let Some(name) = anchor {
                self.register_anchor(name, &scalar)?;
            }
            self.bump_node()?;
            scalar
        };
        Ok(node)
    }

    fn parse_prefixes(
        &mut self,
        text: &str,
        _offset: usize,
    ) -> Result<(Option<YamlTag>, Option<String>, String), YamlError> {
        let mut rest = text.trim().to_owned();
        let mut tag = None;
        let mut anchor = None;
        loop {
            let Some(token) = rest.split_whitespace().next() else {
                break;
            };
            if token.starts_with('&') {
                let name = token[1..].to_owned();
                if !valid_anchor_name(&name, self.options.limits.max_anchor_name_bytes)
                    || anchor.is_some()
                    || self.anchors.contains_key(&name)
                    || self.in_progress.iter().any(|candidate| candidate == &name)
                {
                    return Err(self.error(YamlErrorKind::InvalidAnchor));
                }
                anchor = Some(name);
                rest = rest[token.len()..].trim_start().to_owned();
            } else if token.starts_with('!') {
                if tag.is_some() {
                    return Err(self.error(YamlErrorKind::InvalidTag));
                }
                tag = Some(parse_tag(token).ok_or_else(|| self.error(YamlErrorKind::InvalidTag))?);
                rest = rest[token.len()..].trim_start().to_owned();
            } else {
                break;
            }
        }
        Ok((tag, anchor, rest))
    }

    fn parse_block_scalar(
        &mut self,
        parent_indent: usize,
        header: &str,
        offset: usize,
    ) -> Result<String, YamlError> {
        let bytes = header.as_bytes();
        let folded = bytes.first() == Some(&b'>');
        let strip = header.contains('-');
        let keep = header.contains('+');
        let mut collected = Vec::new();
        while self.index < self.lines.len() {
            let line = &self.lines[self.index];
            let content = strip_comment(&line.text);
            let indent = line.text.bytes().take_while(|byte| *byte == b' ').count();
            if !content.trim().is_empty() && indent <= parent_indent {
                break;
            }
            if line.text.as_bytes().get(indent) == Some(&b'\t') {
                return Err(self.error(YamlErrorKind::InvalidIndentation));
            }
            collected.push((indent, line.text.clone()));
            self.index += 1;
        }
        let content_indent = collected
            .iter()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(indent, _)| *indent)
            .min()
            .unwrap_or(parent_indent + 1);
        let mut output = String::new();
        for (index, (indent, line)) in collected.iter().enumerate() {
            let start = content_indent.min(*indent);
            let text = line.get(start..).unwrap_or_default();
            if folded && index > 0 && !text.is_empty() && !output.ends_with('\n') {
                output.push(' ');
            }
            output.push_str(text);
            output.push('\n');
        }
        if strip {
            while output.ends_with('\n') {
                output.pop();
            }
        } else if !keep {
            while output.ends_with("\n\n") {
                output.pop();
            }
        }
        if output.len() > self.options.limits.max_scalar_bytes {
            return Err(YamlError::at(
                YamlErrorKind::ScalarLimit,
                &self.input,
                offset,
            ));
        }
        Ok(output)
    }

    fn begin_anchor(&mut self, name: &str) -> Result<(), YamlError> {
        if self.anchors.contains_key(name)
            || self.in_progress.iter().any(|candidate| candidate == name)
        {
            return Err(self.error(YamlErrorKind::InvalidAnchor));
        }
        self.in_progress.push(name.to_owned());
        Ok(())
    }

    fn end_anchor(&mut self, name: &str, node: &Node) -> Result<(), YamlError> {
        self.in_progress.retain(|candidate| candidate != name);
        self.register_anchor(name.to_owned(), node)
    }

    fn register_anchor(&mut self, name: String, node: &Node) -> Result<(), YamlError> {
        if self.anchors.insert(name, node.clone()).is_some() {
            return Err(self.error(YamlErrorKind::InvalidAnchor));
        }
        Ok(())
    }

    fn bump_node(&mut self) -> Result<(), YamlError> {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.options.limits.max_nodes {
            Err(self.error(YamlErrorKind::NodeLimit))
        } else {
            Ok(())
        }
    }

    fn null_node(&mut self, _offset: usize) -> Result<Node, YamlError> {
        self.bump_node()?;
        Ok(Node::Scalar {
            value: YamlScalar::Null,
            spelling: String::new(),
            tag: None,
            anchor: None,
        })
    }

    fn current_offset(&self) -> usize {
        self.lines
            .get(self.index)
            .map(|line| line.offset)
            .unwrap_or(self.input.len())
    }

    fn error(&self, kind: YamlErrorKind) -> YamlError {
        YamlError::at(kind, &self.input, self.current_offset())
    }
}

fn strip_comment(line: &str) -> &str {
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    for (index, byte) in line.bytes().enumerate() {
        if double && escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if double => escaped = true,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'#' if !single
                && !double
                && (index == 0 || line.as_bytes()[index - 1].is_ascii_whitespace()) =>
            {
                return &line[..index];
            }
            _ => {}
        }
    }
    line
}

fn is_sequence_marker(content: &str) -> bool {
    content == "-"
        || content
            .strip_prefix('-')
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

fn split_mapping(content: &str) -> Option<(&str, &str)> {
    let bytes = content.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    let mut flow = 0usize;
    for index in 0..bytes.len() {
        let byte = bytes[index];
        if double && escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if double => escaped = true,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'[' | b'{' if !single && !double => flow = flow.saturating_add(1),
            b']' | b'}' if !single && !double => flow = flow.saturating_sub(1),
            b':' if !single
                && !double
                && flow == 0
                && (index + 1 == bytes.len() || bytes[index + 1].is_ascii_whitespace()) =>
            {
                return Some((&content[..index], &content[index + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn node_key(node: &Node) -> String {
    match node {
        Node::Scalar {
            value: YamlScalar::Text(value),
            ..
        } => value.clone(),
        _ => String::new(),
    }
}

fn valid_anchor_name(name: &str, max_bytes: usize) -> bool {
    if name.len() > max_bytes || name.is_empty() || !name.is_ascii() {
        return false;
    }
    let bytes = name.as_bytes();
    (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
}

fn parse_tag(token: &str) -> Option<YamlTag> {
    let name = match token {
        "!!null" | "!<tag:yaml.org,2002:null>" => YamlTag::Null,
        "!!bool" | "!<tag:yaml.org,2002:bool>" => YamlTag::Bool,
        "!!int" | "!<tag:yaml.org,2002:int>" => YamlTag::Int,
        "!!float" | "!<tag:yaml.org,2002:float>" => YamlTag::Float,
        "!!str" | "!<tag:yaml.org,2002:str>" => YamlTag::Str,
        "!!binary" | "!<tag:yaml.org,2002:binary>" => YamlTag::Binary,
        "!!seq" | "!<tag:yaml.org,2002:seq>" => YamlTag::Seq,
        "!!map" | "!<tag:yaml.org,2002:map>" => YamlTag::Map,
        _ => return None,
    };
    Some(name)
}

fn apply_container_tag(node: Node, tag: Option<YamlTag>, anchor: Option<String>) -> Node {
    match node {
        Node::Scalar {
            value,
            spelling,
            tag: existing,
            anchor: old_anchor,
        } => Node::Scalar {
            value,
            spelling,
            tag: tag.or(existing),
            anchor: anchor.or(old_anchor),
        },
        Node::Sequence {
            values,
            tag: existing,
            anchor: old_anchor,
        } => Node::Sequence {
            values,
            tag: tag.or(existing),
            anchor: anchor.or(old_anchor),
        },
        Node::Mapping {
            entries,
            tag: existing,
            anchor: old_anchor,
        } => Node::Mapping {
            entries,
            tag: tag.or(existing),
            anchor: anchor.or(old_anchor),
        },
        Node::Alias(name) => Node::Alias(name),
    }
}

fn scalar_from_text(
    text: &str,
    quoted: bool,
    max_bytes: usize,
) -> Result<(YamlScalar, String), YamlErrorKind> {
    if text.len() > max_bytes {
        return Err(YamlErrorKind::ScalarLimit);
    }
    if quoted {
        return Ok((YamlScalar::Text(text.to_owned()), text.to_owned()));
    }
    let spelling = text.trim().to_owned();
    if spelling.is_empty() || spelling == "~" || spelling.eq_ignore_ascii_case("null") {
        return Ok((YamlScalar::Null, spelling));
    }
    if spelling.eq_ignore_ascii_case("true") {
        return Ok((YamlScalar::Bool(true), spelling));
    }
    if spelling.eq_ignore_ascii_case("false") {
        return Ok((YamlScalar::Bool(false), spelling));
    }
    if spelling.eq_ignore_ascii_case(".nan")
        || spelling.eq_ignore_ascii_case(".inf")
        || spelling.eq_ignore_ascii_case("+.inf")
        || spelling.eq_ignore_ascii_case("-.inf")
    {
        return Err(YamlErrorKind::NonFiniteNumber);
    }
    if looks_like_integer(&spelling) {
        return parse_integer(&spelling).ok_or(YamlErrorKind::NumberOutOfRange);
    }
    if looks_like_float(&spelling) {
        let value = spelling
            .parse::<f64>()
            .map_err(|_| YamlErrorKind::InvalidScalar)?;
        if !value.is_finite() {
            return Err(YamlErrorKind::NonFiniteNumber);
        }
        return Ok((YamlScalar::Float(value), spelling));
    }
    Ok((YamlScalar::Text(spelling.clone()), spelling))
}

fn parse_integer(text: &str) -> Option<(YamlScalar, String)> {
    let negative = text.starts_with('-');
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (radix, body) = if let Some(value) = digits.strip_prefix("0b") {
        (2, value)
    } else if let Some(value) = digits.strip_prefix("0o") {
        (8, value)
    } else if let Some(value) = digits.strip_prefix("0x") {
        (16, value)
    } else if digits.bytes().all(|byte| byte.is_ascii_digit()) {
        (10, digits)
    } else {
        return None;
    };
    if body.is_empty() || body.contains('_') {
        return None;
    }
    let magnitude = u128::from_str_radix(body, radix).ok()?;
    if negative {
        if magnitude > (i64::MAX as u128) + 1 {
            return None;
        }
        let value = if magnitude == (i64::MAX as u128) + 1 {
            i64::MIN
        } else {
            -(magnitude as i64)
        };
        Some((YamlScalar::Int(value), text.to_owned()))
    } else if magnitude <= i64::MAX as u128 {
        Some((YamlScalar::Int(magnitude as i64), text.to_owned()))
    } else if magnitude <= u64::MAX as u128 {
        Some((YamlScalar::UInt(magnitude as u64), text.to_owned()))
    } else {
        None
    }
}

fn looks_like_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if digits.is_empty() {
        return false;
    }
    if digits.starts_with("0b") || digits.starts_with("0o") || digits.starts_with("0x") {
        return digits[2..].bytes().all(|byte| byte.is_ascii_alphanumeric());
    }
    digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn looks_like_float(text: &str) -> bool {
    (text.contains('.') || text.contains('e') || text.contains('E')) && text.parse::<f64>().is_ok()
}

struct FlowParser<'a, 'p> {
    parser: &'p mut Parser,
    text: &'a str,
    cursor: usize,
    offset: usize,
}

impl<'a, 'p> FlowParser<'a, 'p> {
    fn new(parser: &'p mut Parser, text: &'a str, offset: usize) -> Self {
        Self {
            parser,
            text,
            cursor: 0,
            offset,
        }
    }

    fn parse_value(&mut self) -> Result<Node, YamlError> {
        self.skip_space();
        let mut tag = None;
        let mut anchor = None;
        loop {
            self.skip_space();
            let rest = &self.text[self.cursor..];
            let Some(token) = rest
                .split(|ch: char| ch.is_ascii_whitespace() || ",[]{}".contains(ch))
                .next()
            else {
                break;
            };
            if token.starts_with('&') {
                let name = token[1..].to_owned();
                if !valid_anchor_name(&name, self.parser.options.limits.max_anchor_name_bytes)
                    || anchor.is_some()
                {
                    return Err(self.error(YamlErrorKind::InvalidAnchor));
                }
                self.parser.begin_anchor(&name)?;
                anchor = Some(name);
                self.cursor += token.len();
            } else if token.starts_with('!') {
                tag = Some(parse_tag(token).ok_or_else(|| self.error(YamlErrorKind::InvalidTag))?);
                self.cursor += token.len();
            } else {
                break;
            }
        }
        self.skip_space();
        let mut node = match self.text.as_bytes().get(self.cursor).copied() {
            Some(b'[') => self.parse_sequence()?,
            Some(b'{') => self.parse_mapping()?,
            Some(b'\'') => {
                let value = self.parse_single_quoted()?;
                Node::Scalar {
                    value: YamlScalar::Text(value.clone()),
                    spelling: value,
                    tag: None,
                    anchor: None,
                }
            }
            Some(b'"') => {
                let value = self.parse_double_quoted()?;
                Node::Scalar {
                    value: YamlScalar::Text(value.clone()),
                    spelling: value,
                    tag: None,
                    anchor: None,
                }
            }
            Some(b'*') => {
                self.cursor += 1;
                let name = self.take_token();
                if !valid_anchor_name(&name, self.parser.options.limits.max_anchor_name_bytes) {
                    return Err(self.error(YamlErrorKind::InvalidAnchor));
                }
                if self
                    .parser
                    .in_progress
                    .iter()
                    .any(|candidate| candidate == &name)
                {
                    return Err(self.error(YamlErrorKind::AliasCycle));
                }
                self.parser.bump_node()?;
                Node::Alias(name)
            }
            Some(_) => {
                let value = self.take_scalar();
                let (value, spelling) =
                    scalar_from_text(&value, false, self.parser.options.limits.max_scalar_bytes)
                        .map_err(|kind| self.error(kind))?;
                Node::Scalar {
                    value,
                    spelling,
                    tag: None,
                    anchor: None,
                }
            }
            None => return Err(self.error(YamlErrorKind::InvalidDocument)),
        };
        if tag.is_some() || anchor.is_some() {
            node = apply_container_tag(node, tag, anchor.clone());
        }
        if let Some(name) = anchor {
            self.parser.end_anchor(&name, &node)?;
        }
        if matches!(&node, Node::Scalar { .. }) {
            self.parser.bump_node()?;
        }
        Ok(node)
    }

    fn parse_sequence(&mut self) -> Result<Node, YamlError> {
        self.cursor += 1;
        let mut values = Vec::new();
        loop {
            self.skip_space();
            if self.text.as_bytes().get(self.cursor) == Some(&b']') {
                self.cursor += 1;
                break;
            }
            if values.len() >= self.parser.options.limits.max_collection_entries {
                return Err(self.error(YamlErrorKind::CollectionLimit));
            }
            values.push(self.parse_value()?);
            self.skip_space();
            match self.text.as_bytes().get(self.cursor).copied() {
                Some(b',') => {
                    self.cursor += 1;
                }
                Some(b']') => {
                    self.cursor += 1;
                    break;
                }
                _ => return Err(self.error(YamlErrorKind::InvalidDocument)),
            }
        }
        self.parser.bump_node()?;
        Ok(Node::Sequence {
            values,
            tag: None,
            anchor: None,
        })
    }

    fn parse_mapping(&mut self) -> Result<Node, YamlError> {
        self.cursor += 1;
        let mut entries = Vec::new();
        loop {
            self.skip_space();
            if self.text.as_bytes().get(self.cursor) == Some(&b'}') {
                self.cursor += 1;
                break;
            }
            let key = self.parse_value()?;
            if !matches!(
                key,
                Node::Scalar {
                    value: YamlScalar::Text(_),
                    ..
                }
            ) {
                return Err(self.error(YamlErrorKind::NonStringKey));
            }
            if node_key(&key) == "<<" {
                return Err(self.error(YamlErrorKind::MergeKeyForbidden));
            }
            self.skip_space();
            if self.text.as_bytes().get(self.cursor) != Some(&b':') {
                return Err(self.error(YamlErrorKind::InvalidDocument));
            }
            self.cursor += 1;
            self.skip_space();
            let value = if matches!(
                self.text.as_bytes().get(self.cursor),
                Some(b',' | b'}') | None
            ) {
                Node::Scalar {
                    value: YamlScalar::Null,
                    spelling: String::new(),
                    tag: None,
                    anchor: None,
                }
            } else {
                self.parse_value()?
            };
            if entries
                .iter()
                .any(|(candidate, _)| node_key(candidate) == node_key(&key))
            {
                return Err(self.error(YamlErrorKind::DuplicateKey));
            }
            entries.push((key, value));
            if entries.len() > self.parser.options.limits.max_collection_entries {
                return Err(self.error(YamlErrorKind::CollectionLimit));
            }
            self.skip_space();
            match self.text.as_bytes().get(self.cursor).copied() {
                Some(b',') => self.cursor += 1,
                Some(b'}') => {
                    self.cursor += 1;
                    break;
                }
                _ => return Err(self.error(YamlErrorKind::InvalidDocument)),
            }
        }
        self.parser.bump_node()?;
        Ok(Node::Mapping {
            entries,
            tag: None,
            anchor: None,
        })
    }

    fn parse_single_quoted(&mut self) -> Result<String, YamlError> {
        self.cursor += 1;
        let mut value = String::new();
        while self.cursor < self.text.len() {
            let character = self.text[self.cursor..]
                .chars()
                .next()
                .ok_or_else(|| self.error(YamlErrorKind::InvalidScalar))?;
            self.cursor += character.len_utf8();
            if character == '\'' {
                if self.text.as_bytes().get(self.cursor) == Some(&b'\'') {
                    self.cursor += 1;
                    value.push('\'');
                } else {
                    return Ok(value);
                }
            } else {
                value.push(character);
            }
        }
        Err(self.error(YamlErrorKind::InvalidScalar))
    }

    fn parse_double_quoted(&mut self) -> Result<String, YamlError> {
        self.cursor += 1;
        let mut value = String::new();
        while self.cursor < self.text.len() {
            let character = self.text[self.cursor..]
                .chars()
                .next()
                .ok_or_else(|| self.error(YamlErrorKind::InvalidScalar))?;
            self.cursor += character.len_utf8();
            match character {
                '"' => return Ok(value),
                '\\' => {
                    let escaped = *self
                        .text
                        .as_bytes()
                        .get(self.cursor)
                        .ok_or_else(|| self.error(YamlErrorKind::InvalidEscape))?;
                    self.cursor += 1;
                    match escaped {
                        b'0' => value.push('\0'),
                        b'a' => value.push('\u{7}'),
                        b'b' => value.push('\u{8}'),
                        b't' => value.push('\t'),
                        b'n' => value.push('\n'),
                        b'v' => value.push('\u{b}'),
                        b'f' => value.push('\u{c}'),
                        b'r' => value.push('\r'),
                        b'e' => value.push('\u{1b}'),
                        b'"' | b'\\' | b'/' => value.push(escaped as char),
                        b'x' => value.push(parse_hex_char(self)?),
                        b'u' => value.push(parse_hex_char_n(self, 4)?),
                        b'U' => value.push(parse_hex_char_n(self, 8)?),
                        _ => return Err(self.error(YamlErrorKind::InvalidEscape)),
                    }
                }
                _ => value.push(character),
            }
        }
        Err(self.error(YamlErrorKind::InvalidScalar))
    }

    fn take_token(&mut self) -> String {
        let start = self.cursor;
        while let Some(byte) = self.text.as_bytes().get(self.cursor) {
            if byte.is_ascii_whitespace() || b",[]{}".contains(byte) {
                break;
            }
            self.cursor += 1;
        }
        self.text[start..self.cursor].to_owned()
    }

    fn take_scalar(&mut self) -> String {
        let start = self.cursor;
        let mut depth = 0usize;
        while let Some(byte) = self.text.as_bytes().get(self.cursor) {
            match byte {
                b'[' | b'{' => depth += 1,
                b']' | b'}' if depth > 0 => depth -= 1,
                b',' | b']' | b'}' if depth == 0 => break,
                b':' if depth == 0
                    && self
                        .text
                        .as_bytes()
                        .get(self.cursor + 1)
                        .is_some_and(|next| {
                            next.is_ascii_whitespace() || *next == b',' || *next == b'}'
                        }) =>
                {
                    break;
                }
                _ => {}
            }
            self.cursor += 1;
        }
        self.text[start..self.cursor].trim().to_owned()
    }

    fn skip_space(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.cursor += 1;
        }
    }

    fn position(&self) -> usize {
        self.cursor
    }

    fn error(&self, kind: YamlErrorKind) -> YamlError {
        YamlError::at(kind, &self.parser.input, self.offset + self.cursor)
    }
}

fn parse_hex_char(parser: &mut FlowParser<'_, '_>) -> Result<char, YamlError> {
    parse_hex_char_n(parser, 2)
}

fn parse_hex_char_n(parser: &mut FlowParser<'_, '_>, width: usize) -> Result<char, YamlError> {
    let end = parser.cursor.saturating_add(width);
    let text = parser
        .text
        .get(parser.cursor..end)
        .ok_or_else(|| parser.error(YamlErrorKind::InvalidEscape))?;
    parser.cursor = end;
    let value =
        u32::from_str_radix(text, 16).map_err(|_| parser.error(YamlErrorKind::InvalidEscape))?;
    char::from_u32(value).ok_or_else(|| parser.error(YamlErrorKind::InvalidEscape))
}

#[derive(Default)]
struct MaterializeState {
    aliases: usize,
    expanded_nodes: usize,
    alias_stack: Vec<String>,
}

fn materialize_documents(
    documents: Vec<(Node, HashMap<String, Node>)>,
    options: YamlOptions,
) -> Result<Vec<YamlValue>, YamlError> {
    let mut values = Vec::with_capacity(documents.len());
    for (root, anchors) in documents {
        let mut state = MaterializeState::default();
        let mut path = Vec::new();
        values.push(materialize_node(
            &root, &anchors, options, 0, &mut state, &mut path,
        )?);
    }
    Ok(values)
}

fn materialize_node(
    node: &Node,
    anchors: &HashMap<String, Node>,
    options: YamlOptions,
    depth: usize,
    state: &mut MaterializeState,
    path: &mut Vec<YamlPathSegment>,
) -> Result<YamlValue, YamlError> {
    if depth > options.limits.max_depth {
        return Err(YamlError::at_zero(YamlErrorKind::DepthLimit).with_path(path));
    }
    state.expanded_nodes = state.expanded_nodes.saturating_add(1);
    if state.expanded_nodes > options.limits.max_expanded_nodes {
        return Err(YamlError::at_zero(YamlErrorKind::ExpandedNodeLimit).with_path(path));
    }
    match node {
        Node::Alias(name) => {
            state.aliases = state.aliases.saturating_add(1);
            if state.aliases > options.limits.max_aliases {
                return Err(YamlError::at_zero(YamlErrorKind::AliasLimit).with_path(path));
            }
            if state.alias_stack.iter().any(|candidate| candidate == name) {
                return Err(YamlError::at_zero(YamlErrorKind::AliasCycle).with_path(path));
            }
            let target = anchors
                .get(name)
                .ok_or_else(|| YamlError::at_zero(YamlErrorKind::UndefinedAlias).with_path(path))?;
            state.alias_stack.push(name.clone());
            let value = materialize_node(target, anchors, options, depth + 1, state, path);
            state.alias_stack.pop();
            value
        }
        Node::Scalar {
            value,
            spelling,
            tag,
            ..
        } => materialize_scalar(value, spelling, *tag, path),
        Node::Sequence { values, tag, .. } => {
            if values.len() > options.limits.max_collection_entries {
                return Err(YamlError::at_zero(YamlErrorKind::CollectionLimit).with_path(path));
            }
            let mut output = Vec::with_capacity(values.len());
            for (index, value) in values.iter().enumerate() {
                path.push(YamlPathSegment::Index(index));
                let result = materialize_node(value, anchors, options, depth + 1, state, path);
                path.pop();
                output.push(result?);
            }
            let output = YamlValue::Array(output);
            apply_tag(output, *tag, path)
        }
        Node::Mapping { entries, tag, .. } => {
            if entries.len() > options.limits.max_collection_entries {
                return Err(YamlError::at_zero(YamlErrorKind::CollectionLimit).with_path(path));
            }
            let mut output = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key_value = materialize_node(key, anchors, options, depth + 1, state, path)?;
                let YamlValue::Text(key_text) = key_value else {
                    return Err(YamlError::at_zero(YamlErrorKind::NonStringKey).with_path(path));
                };
                if output
                    .iter()
                    .any(|member: &YamlMember| member.key == key_text)
                {
                    return Err(YamlError::at_zero(YamlErrorKind::DuplicateKey).with_path(path));
                }
                path.push(YamlPathSegment::Key(key_text.clone()));
                let result = materialize_node(value, anchors, options, depth + 1, state, path);
                path.pop();
                output.push(YamlMember {
                    key: key_text,
                    value: result?,
                });
            }
            let output = YamlValue::Object(output);
            apply_tag(output, *tag, path)
        }
    }
}

fn materialize_scalar(
    value: &YamlScalar,
    spelling: &str,
    tag: Option<YamlTag>,
    path: &[YamlPathSegment],
) -> Result<YamlValue, YamlError> {
    let output = match tag {
        None => scalar_to_value(value),
        Some(YamlTag::Null) if matches!(value, YamlScalar::Null) => YamlValue::Null,
        Some(YamlTag::Bool) if matches!(value, YamlScalar::Bool(_)) => scalar_to_value(value),
        Some(YamlTag::Int) => match value {
            YamlScalar::Int(value) => YamlValue::Int(*value),
            YamlScalar::UInt(value) => YamlValue::UInt(*value),
            _ => return Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path)),
        },
        Some(YamlTag::Float) => match value {
            YamlScalar::Float(value) => YamlValue::Float(*value),
            YamlScalar::Int(value) => YamlValue::Float(*value as f64),
            YamlScalar::UInt(value) => YamlValue::Float(*value as f64),
            _ => return Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path)),
        },
        Some(YamlTag::Str) => YamlValue::Text(spelling.to_owned()),
        Some(YamlTag::Binary) => {
            let compact = spelling
                .chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect::<String>();
            let bytes = serialization::base64_decode(&compact)
                .map_err(|_| YamlError::at_zero(YamlErrorKind::InvalidBinary).with_path(path))?;
            YamlValue::Bytes(bytes)
        }
        Some(YamlTag::Seq) | Some(YamlTag::Map) => {
            return Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path));
        }
        Some(YamlTag::Null | YamlTag::Bool) => {
            return Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path));
        }
    };
    Ok(output)
}

fn scalar_to_value(value: &YamlScalar) -> YamlValue {
    match value {
        YamlScalar::Null => YamlValue::Null,
        YamlScalar::Bool(value) => YamlValue::Bool(*value),
        YamlScalar::Int(value) => YamlValue::Int(*value),
        YamlScalar::UInt(value) => YamlValue::UInt(*value),
        YamlScalar::Float(value) => YamlValue::Float(*value),
        YamlScalar::Text(value) => YamlValue::Text(value.clone()),
        YamlScalar::Bytes(value) => YamlValue::Bytes(value.clone()),
    }
}

fn apply_tag(
    value: YamlValue,
    tag: Option<YamlTag>,
    path: &[YamlPathSegment],
) -> Result<YamlValue, YamlError> {
    match tag {
        None => Ok(value),
        Some(YamlTag::Seq) if matches!(value, YamlValue::Array(_)) => Ok(value),
        Some(YamlTag::Map) if matches!(value, YamlValue::Object(_)) => Ok(value),
        Some(YamlTag::Str) if matches!(value, YamlValue::Text(_)) => Ok(value),
        Some(YamlTag::Null) if matches!(value, YamlValue::Null) => Ok(value),
        Some(YamlTag::Bool) if matches!(value, YamlValue::Bool(_)) => Ok(value),
        Some(YamlTag::Int) if matches!(value, YamlValue::Int(_) | YamlValue::UInt(_)) => Ok(value),
        Some(YamlTag::Float)
            if matches!(
                value,
                YamlValue::Float(_) | YamlValue::Int(_) | YamlValue::UInt(_)
            ) =>
        {
            Ok(value)
        }
        Some(YamlTag::Binary) => {
            Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path))
        }
        _ => Err(YamlError::at_zero(YamlErrorKind::TypeMismatch).with_path(path)),
    }
}

/// Parse one bounded YAML document using the default options.
pub fn parse(input: &[u8]) -> Result<YamlValue, YamlError> {
    parse_with_options(input, YamlOptions::default())
}

/// Parse one bounded YAML document. `parse` rejects a stream containing more
/// than one document; use [`parse_all`] when a stream is intentional.
pub fn parse_with_options(input: &[u8], options: YamlOptions) -> Result<YamlValue, YamlError> {
    let mut values = parse_all_with_options(input, options)?;
    if values.len() != 1 {
        return Err(YamlError::at_zero(YamlErrorKind::TrailingDocument));
    }
    Ok(values.pop().expect("one YAML document"))
}

pub fn parse_all(input: &[u8]) -> Result<Vec<YamlValue>, YamlError> {
    parse_all_with_options(input, YamlOptions::default())
}

pub fn parse_all_with_options(
    input: &[u8],
    options: YamlOptions,
) -> Result<Vec<YamlValue>, YamlError> {
    let parser = Parser::new(input, options)?;
    materialize_documents(parser.parse_stream()?, options)
}

pub fn parse_view<'a>(
    input: &'a [u8],
    options: YamlOptions,
) -> Result<YamlValueView<'a>, YamlError> {
    let _ = parse_with_options(input, options)?;
    Ok(YamlValueView { input, options })
}

pub fn validate(input: &[u8], options: YamlOptions) -> Result<(), YamlError> {
    let _ = parse_with_options(input, options)?;
    Ok(())
}

fn validate_value(
    value: &YamlValue,
    limits: YamlLimits,
    depth: usize,
    nodes: &mut usize,
    scalar_bytes: &mut usize,
) -> Result<(), YamlError> {
    if depth > limits.max_depth {
        return Err(YamlError::at_zero(YamlErrorKind::DepthLimit));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > limits.max_nodes || *nodes > limits.max_expanded_nodes {
        return Err(YamlError::at_zero(YamlErrorKind::NodeLimit));
    }
    match value {
        YamlValue::Text(text) => {
            *scalar_bytes = scalar_bytes.saturating_add(text.len());
            if *scalar_bytes > limits.max_scalar_bytes {
                return Err(YamlError::at_zero(YamlErrorKind::ScalarLimit));
            }
        }
        YamlValue::Bytes(bytes) => {
            *scalar_bytes = scalar_bytes.saturating_add(bytes.len());
            if *scalar_bytes > limits.max_scalar_bytes {
                return Err(YamlError::at_zero(YamlErrorKind::ScalarLimit));
            }
        }
        YamlValue::Float(value) if !value.is_finite() => {
            return Err(YamlError::at_zero(YamlErrorKind::NonFiniteNumber));
        }
        YamlValue::Float(_) => {}
        YamlValue::Array(values) => {
            if values.len() > limits.max_collection_entries {
                return Err(YamlError::at_zero(YamlErrorKind::CollectionLimit));
            }
            for value in values {
                validate_value(value, limits, depth + 1, nodes, scalar_bytes)?;
            }
        }
        YamlValue::Object(members) => {
            if members.len() > limits.max_collection_entries {
                return Err(YamlError::at_zero(YamlErrorKind::CollectionLimit));
            }
            let mut keys = Vec::with_capacity(members.len());
            for member in members {
                if keys.iter().any(|key: &String| key == &member.key) {
                    return Err(YamlError::at_zero(YamlErrorKind::DuplicateKey));
                }
                *scalar_bytes = scalar_bytes.saturating_add(member.key.len());
                keys.push(member.key.clone());
                validate_value(&member.value, limits, depth + 1, nodes, scalar_bytes)?;
            }
            if *scalar_bytes > limits.max_scalar_bytes {
                return Err(YamlError::at_zero(YamlErrorKind::ScalarLimit));
            }
        }
        YamlValue::Null | YamlValue::Bool(_) | YamlValue::Int(_) | YamlValue::UInt(_) => {}
    }
    Ok(())
}

/// Encode one dynamic YAML document in the deterministic block style.
pub fn encode(value: &YamlValue, options: YamlOptions) -> Result<Vec<u8>, YamlError> {
    encode_inner(value, options, false)
}

pub fn encode_canonical(value: &YamlValue, limits: YamlLimits) -> Result<Vec<u8>, YamlError> {
    encode_inner(value, YamlOptions::create(limits), true)
}

fn encode_inner(
    value: &YamlValue,
    options: YamlOptions,
    canonical: bool,
) -> Result<Vec<u8>, YamlError> {
    if !options.limits.valid() {
        return Err(YamlError::at_zero(YamlErrorKind::InvalidLimit));
    }
    let mut nodes = 0;
    let mut scalar_bytes = 0;
    validate_value(value, options.limits, 0, &mut nodes, &mut scalar_bytes)?;
    let mut output = render_node(value, 0, canonical);
    output.push('\n');
    if output.len() > options.limits.max_input_bytes {
        return Err(YamlError::at_zero(YamlErrorKind::NodeLimit));
    }
    Ok(output.into_bytes())
}

fn render_node(value: &YamlValue, indent: usize, canonical: bool) -> String {
    match value {
        YamlValue::Null
        | YamlValue::Bool(_)
        | YamlValue::Int(_)
        | YamlValue::UInt(_)
        | YamlValue::Float(_)
        | YamlValue::Text(_)
        | YamlValue::Bytes(_) => render_scalar(value),
        YamlValue::Array(values) => {
            if values.is_empty() {
                return "[]".into();
            }
            let mut output = String::new();
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                output.push_str(&" ".repeat(indent));
                output.push_str("- ");
                if is_inline(value) {
                    output.push_str(&render_node(value, indent + 2, canonical));
                } else {
                    output.push('\n');
                    output.push_str(&render_node(value, indent + 2, canonical));
                }
            }
            output
        }
        YamlValue::Object(members) => {
            if members.is_empty() {
                return "{}".into();
            }
            let mut order = (0..members.len()).collect::<Vec<_>>();
            if canonical {
                order.sort_by(|left, right| {
                    members[*left]
                        .key
                        .as_bytes()
                        .cmp(members[*right].key.as_bytes())
                });
            }
            let mut output = String::new();
            for (position, index) in order.into_iter().enumerate() {
                if position > 0 {
                    output.push('\n');
                }
                let member = &members[index];
                output.push_str(&" ".repeat(indent));
                output.push_str(&render_text(&member.key));
                output.push(':');
                if is_inline(&member.value) {
                    output.push(' ');
                    output.push_str(&render_node(&member.value, indent + 2, canonical));
                } else {
                    output.push('\n');
                    output.push_str(&render_node(&member.value, indent + 2, canonical));
                }
            }
            output
        }
    }
}

fn is_inline(value: &YamlValue) -> bool {
    matches!(
        value,
        YamlValue::Null
            | YamlValue::Bool(_)
            | YamlValue::Int(_)
            | YamlValue::UInt(_)
            | YamlValue::Float(_)
            | YamlValue::Text(_)
            | YamlValue::Bytes(_)
    ) || matches!(value, YamlValue::Array(values) if values.is_empty())
        || matches!(value, YamlValue::Object(values) if values.is_empty())
}

fn render_scalar(value: &YamlValue) -> String {
    match value {
        YamlValue::Null => "null".into(),
        YamlValue::Bool(value) => value.to_string(),
        YamlValue::Int(value) => value.to_string(),
        YamlValue::UInt(value) => value.to_string(),
        YamlValue::Float(value) => value.to_string(),
        YamlValue::Text(value) => render_text(value),
        YamlValue::Bytes(value) => format!("!!binary {}", serialization::base64_encode(value)),
        YamlValue::Array(_) | YamlValue::Object(_) => unreachable!("containers are not scalar"),
    }
}

fn render_text(value: &str) -> String {
    if plain_scalar_safe(value) {
        return value.to_owned();
    }
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn plain_scalar_safe(value: &str) -> bool {
    if value.is_empty() || value.trim() != value || value.contains(['\n', '\r', '\t']) {
        return false;
    }
    if value.starts_with([
        '-', '?', ':', '!', '&', '*', '#', '{', '}', '[', ']', ',', '|', '>', '@', '`', '%',
    ]) {
        return false;
    }
    if value.contains(": ") || value.contains(" #") || value == "-" || value == "?" || value == ":"
    {
        return false;
    }
    matches!(
        scalar_from_text(value, false, usize::MAX),
        Ok((YamlScalar::Text(ref parsed), ref spelling)) if parsed == value && spelling == value
    )
}

fn value_to_events(value: &YamlValue, events: &mut Vec<YamlEvent>) {
    match value {
        YamlValue::Null => events.push(YamlEvent::Scalar(YamlScalar::Null)),
        YamlValue::Bool(value) => events.push(YamlEvent::Scalar(YamlScalar::Bool(*value))),
        YamlValue::Int(value) => events.push(YamlEvent::Scalar(YamlScalar::Int(*value))),
        YamlValue::UInt(value) => events.push(YamlEvent::Scalar(YamlScalar::UInt(*value))),
        YamlValue::Float(value) => events.push(YamlEvent::Scalar(YamlScalar::Float(*value))),
        YamlValue::Text(value) => events.push(YamlEvent::Scalar(YamlScalar::Text(value.clone()))),
        YamlValue::Bytes(value) => events.push(YamlEvent::Scalar(YamlScalar::Bytes(value.clone()))),
        YamlValue::Array(values) => {
            events.push(YamlEvent::SequenceStart(None));
            for value in values {
                value_to_events(value, events);
            }
            events.push(YamlEvent::SequenceEnd);
        }
        YamlValue::Object(members) => {
            events.push(YamlEvent::MappingStart(None));
            for member in members {
                events.push(YamlEvent::MappingKey);
                events.push(YamlEvent::Scalar(YamlScalar::Text(member.key.clone())));
                value_to_events(&member.value, events);
            }
            events.push(YamlEvent::MappingEnd);
        }
    }
}

fn events_for_values(values: &[YamlValue]) -> Vec<YamlEvent> {
    let mut events = Vec::new();
    events.push(YamlEvent::StreamStart);
    for value in values {
        events.push(YamlEvent::DocumentStart);
        value_to_events(value, &mut events);
        events.push(YamlEvent::DocumentEnd);
    }
    events.push(YamlEvent::StreamEnd);
    events
}

fn node_to_events(node: &Node, events: &mut Vec<YamlEvent>) {
    match node {
        Node::Alias(name) => events.push(YamlEvent::Alias(name.clone())),
        Node::Scalar {
            value, tag, anchor, ..
        } => {
            if let Some(anchor) = anchor {
                events.push(YamlEvent::Anchor(anchor.clone()));
            }
            if let Some(tag) = tag {
                events.push(YamlEvent::Tag(*tag));
            }
            events.push(YamlEvent::Scalar(value.clone()));
        }
        Node::Sequence {
            values,
            tag,
            anchor,
        } => {
            if let Some(anchor) = anchor {
                events.push(YamlEvent::Anchor(anchor.clone()));
            }
            if let Some(tag) = tag {
                events.push(YamlEvent::Tag(*tag));
            }
            events.push(YamlEvent::SequenceStart(None));
            for value in values {
                node_to_events(value, events);
            }
            events.push(YamlEvent::SequenceEnd);
        }
        Node::Mapping {
            entries,
            tag,
            anchor,
        } => {
            if let Some(anchor) = anchor {
                events.push(YamlEvent::Anchor(anchor.clone()));
            }
            if let Some(tag) = tag {
                events.push(YamlEvent::Tag(*tag));
            }
            events.push(YamlEvent::MappingStart(None));
            for (key, value) in entries {
                events.push(YamlEvent::MappingKey);
                node_to_events(key, events);
                node_to_events(value, events);
            }
            events.push(YamlEvent::MappingEnd);
        }
    }
}

fn events_for_nodes(documents: &[(Node, HashMap<String, Node>)]) -> Vec<YamlEvent> {
    let mut events = vec![YamlEvent::StreamStart];
    for (root, _) in documents {
        events.push(YamlEvent::DocumentStart);
        node_to_events(root, &mut events);
        events.push(YamlEvent::DocumentEnd);
    }
    events.push(YamlEvent::StreamEnd);
    events
}

#[derive(Clone)]
pub struct YamlReader {
    events: Vec<YamlEvent>,
    index: usize,
    options: YamlOptions,
    eof_returned: bool,
    finished: bool,
    terminal: Option<YamlError>,
}

impl YamlReader {
    pub fn from_bytes(input: &[u8], options: YamlOptions) -> Result<Self, YamlError> {
        let parser = Parser::new(input, options)?;
        let documents = parser.parse_stream()?;
        let _ = materialize_documents(documents.clone(), options)?;
        Self::from_events(events_for_nodes(&documents), options)
    }

    pub fn from_reader<R: Read>(mut input: R, options: YamlOptions) -> Result<Self, YamlError> {
        if !options.limits.valid() {
            return Err(YamlError::at_zero(YamlErrorKind::InvalidLimit));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = input
                .read(&mut chunk)
                .map_err(|error| YamlError::at_zero(YamlErrorKind::Io(error.to_string())))?;
            if read == 0 {
                break;
            }
            if bytes
                .len()
                .checked_add(read)
                .is_none_or(|length| length > options.limits.max_input_bytes)
            {
                return Err(YamlError::at_zero(YamlErrorKind::NodeLimit));
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        Self::from_bytes(&bytes, options)
    }

    fn from_values(values: Vec<YamlValue>, options: YamlOptions) -> Result<Self, YamlError> {
        let events = events_for_values(&values);
        if events.len() > options.limits.max_expanded_nodes.saturating_mul(4) {
            return Err(YamlError::at_zero(YamlErrorKind::ExpandedNodeLimit));
        }
        Ok(Self {
            events,
            index: 0,
            options,
            eof_returned: false,
            finished: false,
            terminal: None,
        })
    }

    #[allow(dead_code)]
    fn from_events(events: Vec<YamlEvent>, options: YamlOptions) -> Result<Self, YamlError> {
        if !options.limits.valid()
            || events.len() > options.limits.max_expanded_nodes.saturating_mul(4)
        {
            return Err(YamlError::at_zero(YamlErrorKind::InvalidLimit));
        }
        Ok(Self {
            events,
            index: 0,
            options,
            eof_returned: false,
            finished: false,
            terminal: None,
        })
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<YamlEvent>, YamlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished || self.eof_returned {
            return Err(YamlError::at_zero(YamlErrorKind::Closed));
        }
        if let Some(event) = self.events.get(self.index).cloned() {
            self.index += 1;
            return Ok(Some(event));
        }
        self.eof_returned = true;
        Ok(None)
    }

    pub fn own(&mut self, event: YamlEvent) -> Result<YamlEvent, YamlError> {
        if self.finished || self.eof_returned {
            return Err(YamlError::at_zero(YamlErrorKind::Closed));
        }
        if let YamlEvent::Scalar(YamlScalar::Text(text)) = &event {
            if text.len() > self.options.limits.max_scalar_bytes {
                return self.fail(YamlErrorKind::ScalarLimit);
            }
        }
        Ok(event)
    }

    pub fn finish(&mut self) -> Result<(), YamlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return Err(YamlError::at_zero(YamlErrorKind::Closed));
        }
        if self.eof_returned {
            return Err(YamlError::at_zero(YamlErrorKind::Closed));
        }
        if self.index != self.events.len() {
            return self.fail(YamlErrorKind::UnexpectedEvent);
        }
        self.eof_returned = true;
        self.finished = true;
        Ok(())
    }

    fn fail<T>(&mut self, kind: YamlErrorKind) -> Result<T, YamlError> {
        let error = YamlError::at_zero(kind);
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn next_data_event(&mut self) -> Result<Option<YamlEvent>, YamlError> {
        loop {
            match self.next()? {
                Some(
                    YamlEvent::StreamStart | YamlEvent::DocumentStart | YamlEvent::DocumentEnd,
                ) => {}
                Some(YamlEvent::StreamEnd) => return Ok(None),
                other => return Ok(other),
            }
        }
    }

    fn serialization_limits(&self) -> serialization::Limits {
        serialization::Limits {
            max_depth: self.options.limits.max_depth,
            max_events: self.options.limits.max_expanded_nodes,
            max_bytes: self.options.limits.max_input_bytes,
            max_container_items: self.options.limits.max_collection_entries,
        }
    }
}

#[derive(Debug)]
enum WriterFrame {
    Array(Vec<YamlValue>),
    Mapping {
        members: Vec<YamlMember>,
        pending_key: Option<String>,
        expecting_key: bool,
    },
}

pub struct YamlWriter {
    options: YamlOptions,
    stack: Vec<WriterFrame>,
    root: Option<YamlValue>,
    finished: bool,
    terminal: Option<YamlError>,
}

impl YamlWriter {
    pub fn to_writer(options: YamlOptions) -> Result<Self, YamlError> {
        if !options.limits.valid() {
            return Err(YamlError::at_zero(YamlErrorKind::InvalidLimit));
        }
        Ok(Self {
            options,
            stack: Vec::new(),
            root: None,
            finished: false,
            terminal: None,
        })
    }

    pub fn write(&mut self, event: YamlEvent) -> Result<(), YamlError> {
        let result = self.write_inner(event);
        if let Err(error) = &result {
            self.terminal = Some(error.clone());
        }
        result
    }

    fn write_inner(&mut self, event: YamlEvent) -> Result<(), YamlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return self.fail(YamlErrorKind::Closed);
        }
        match event {
            YamlEvent::StreamStart
            | YamlEvent::DocumentStart
            | YamlEvent::DocumentEnd
            | YamlEvent::StreamEnd => Ok(()),
            YamlEvent::SequenceStart(_) => {
                self.stack.push(WriterFrame::Array(Vec::new()));
                Ok(())
            }
            YamlEvent::SequenceEnd => {
                let Some(WriterFrame::Array(values)) = self.stack.pop() else {
                    return self.fail(YamlErrorKind::UnexpectedEvent);
                };
                self.attach(YamlValue::Array(values))
            }
            YamlEvent::MappingStart(_) => {
                self.stack.push(WriterFrame::Mapping {
                    members: Vec::new(),
                    pending_key: None,
                    expecting_key: false,
                });
                Ok(())
            }
            YamlEvent::MappingKey => {
                let Some(WriterFrame::Mapping {
                    expecting_key,
                    pending_key,
                    ..
                }) = self.stack.last_mut()
                else {
                    return self.fail(YamlErrorKind::UnexpectedEvent);
                };
                if *expecting_key || pending_key.is_some() {
                    return self.fail(YamlErrorKind::UnexpectedEvent);
                }
                *expecting_key = true;
                Ok(())
            }
            YamlEvent::MappingEnd => {
                let Some(WriterFrame::Mapping {
                    members,
                    pending_key,
                    expecting_key,
                }) = self.stack.pop()
                else {
                    return self.fail(YamlErrorKind::UnexpectedEvent);
                };
                if pending_key.is_some() || expecting_key {
                    return self.fail(YamlErrorKind::UnexpectedEvent);
                }
                self.attach(YamlValue::Object(members))
            }
            YamlEvent::Scalar(scalar) => {
                let value = scalar_to_value(&scalar);
                if let Some(WriterFrame::Mapping {
                    expecting_key,
                    pending_key,
                    ..
                }) = self.stack.last_mut()
                {
                    if *expecting_key {
                        let YamlValue::Text(key) = value else {
                            return self.fail(YamlErrorKind::NonStringKey);
                        };
                        *pending_key = Some(key);
                        *expecting_key = false;
                        return Ok(());
                    }
                }
                self.attach(value)
            }
            YamlEvent::Anchor(_) | YamlEvent::Alias(_) | YamlEvent::Tag(_) => {
                self.fail(YamlErrorKind::UnexpectedEvent)
            }
        }
    }

    fn attach(&mut self, value: YamlValue) -> Result<(), YamlError> {
        if let Some(frame) = self.stack.last_mut() {
            match frame {
                WriterFrame::Array(values) => values.push(value),
                WriterFrame::Mapping {
                    members,
                    pending_key,
                    expecting_key,
                } => {
                    if *expecting_key {
                        return self.fail(YamlErrorKind::UnexpectedEvent);
                    }
                    let Some(key) = pending_key.take() else {
                        return self.fail(YamlErrorKind::UnexpectedEvent);
                    };
                    if members.iter().any(|member| member.key == key) {
                        return self.fail(YamlErrorKind::DuplicateKey);
                    }
                    members.push(YamlMember { key, value });
                }
            }
            return Ok(());
        }
        if self.root.replace(value).is_some() {
            return self.fail(YamlErrorKind::TrailingDocument);
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, YamlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return Err(YamlError::at_zero(YamlErrorKind::Closed));
        }
        if !self.stack.is_empty() {
            return self.fail(YamlErrorKind::UnexpectedEvent);
        }
        let value = self
            .root
            .take()
            .ok_or_else(|| YamlError::at_zero(YamlErrorKind::InvalidDocument))?;
        let output = encode(&value, self.options)?;
        self.finished = true;
        Ok(output)
    }

    fn fail<T>(&mut self, kind: YamlErrorKind) -> Result<T, YamlError> {
        let error = YamlError::at_zero(kind);
        self.terminal = Some(error.clone());
        Err(error)
    }
}

fn yaml_scalar_to_event(value: YamlScalar) -> Result<Event, YamlError> {
    match value {
        YamlScalar::Null => Ok(Event::Null),
        YamlScalar::Bool(value) => Ok(Event::Bool(value)),
        YamlScalar::Int(value) => Ok(Event::Int(i128::from(value))),
        YamlScalar::UInt(value) => Ok(Event::UInt(u128::from(value))),
        YamlScalar::Float(value) if value.is_finite() => Ok(Event::Float64(value.to_bits())),
        YamlScalar::Float(_) => Err(YamlError::at_zero(YamlErrorKind::NonFiniteNumber)),
        YamlScalar::Text(value) => Ok(Event::String(value)),
        YamlScalar::Bytes(value) => Ok(Event::Bytes(value)),
    }
}

fn yaml_event_to_event(event: YamlEvent) -> Result<Event, YamlError> {
    match event {
        YamlEvent::Scalar(value) => yaml_scalar_to_event(value),
        YamlEvent::SequenceStart(_) => Ok(Event::StartArray(None)),
        YamlEvent::SequenceEnd => Ok(Event::EndArray),
        YamlEvent::MappingStart(_) => Ok(Event::StartMap(None)),
        YamlEvent::MappingKey => Ok(Event::MapKey),
        YamlEvent::MappingEnd => Ok(Event::EndMap),
        YamlEvent::StreamStart
        | YamlEvent::DocumentStart
        | YamlEvent::DocumentEnd
        | YamlEvent::StreamEnd => Err(YamlError::at_zero(YamlErrorKind::UnexpectedEvent)),
        YamlEvent::Anchor(_) | YamlEvent::Alias(_) | YamlEvent::Tag(_) => {
            Err(YamlError::at_zero(YamlErrorKind::UnexpectedEvent))
        }
    }
}

pub fn encode_typed<T: serialization::Serialize>(
    value: &T,
    options: YamlOptions,
) -> Result<Vec<u8>, YamlError> {
    let events = serialization::serialize_value(
        value,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_expanded_nodes,
            max_bytes: options.limits.max_input_bytes,
            max_container_items: options.limits.max_collection_entries,
        },
    )
    .map_err(YamlError::from)?;
    let mut writer = YamlWriter::to_writer(options)?;
    for event in events {
        let converted = match event {
            Event::Null => YamlEvent::Scalar(YamlScalar::Null),
            Event::Bool(value) => YamlEvent::Scalar(YamlScalar::Bool(value)),
            Event::Int(value) => YamlEvent::Scalar(YamlScalar::Int(
                i64::try_from(value)
                    .map_err(|_| YamlError::at_zero(YamlErrorKind::NumberOutOfRange))?,
            )),
            Event::UInt(value) => YamlEvent::Scalar(YamlScalar::UInt(
                u64::try_from(value)
                    .map_err(|_| YamlError::at_zero(YamlErrorKind::NumberOutOfRange))?,
            )),
            Event::Float(value) => YamlEvent::Scalar(YamlScalar::Float(value)),
            Event::Float32(value) => {
                YamlEvent::Scalar(YamlScalar::Float(f32::from_bits(value) as f64))
            }
            Event::Float64(value) => YamlEvent::Scalar(YamlScalar::Float(f64::from_bits(value))),
            Event::String(value) => YamlEvent::Scalar(YamlScalar::Text(value)),
            Event::Bytes(value) => YamlEvent::Scalar(YamlScalar::Bytes(value)),
            Event::StartArray(_) => YamlEvent::SequenceStart(None),
            Event::EndArray => YamlEvent::SequenceEnd,
            Event::StartMap(_) | Event::StartRecord { .. } => YamlEvent::MappingStart(None),
            Event::MapKey => YamlEvent::MappingKey,
            Event::Field(value) => {
                writer.write(YamlEvent::MappingKey)?;
                YamlEvent::Scalar(YamlScalar::Text(value))
            }
            Event::EndMap | Event::EndRecord => YamlEvent::MappingEnd,
            Event::StartEnum { .. } | Event::EndEnum => {
                return Err(YamlError::at_zero(YamlErrorKind::TypeMismatch));
            }
        };
        writer.write(converted)?;
    }
    writer.finish()
}

pub fn decode_typed<T: serialization::Deserialize>(
    input: &[u8],
    options: YamlOptions,
) -> Result<T, YamlError> {
    let values = parse_all_with_options(input, options)?;
    if values.len() != 1 {
        return Err(YamlError::at_zero(YamlErrorKind::TrailingDocument));
    }
    let mut yaml_events = Vec::new();
    value_to_events(&values[0], &mut yaml_events);
    let events = yaml_events
        .into_iter()
        .map(yaml_event_to_event)
        .collect::<Result<Vec<_>, _>>()?;
    serialization::deserialize_value(
        &events,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_expanded_nodes,
            max_bytes: options.limits.max_input_bytes,
            max_container_items: options.limits.max_collection_entries,
        },
    )
    .map_err(YamlError::from)
}

pub fn encode_static<T: Encode<YamlCodec>>(
    value: &T,
    options: YamlOptions,
) -> Result<Vec<u8>, YamlError> {
    let mut writer = YamlWriter::to_writer(options)?;
    value.encode(&mut writer)?;
    writer.finish()
}

pub fn decode_static<T: Decode<YamlCodec>>(
    input: &[u8],
    options: YamlOptions,
) -> Result<T, YamlError> {
    let values = parse_all_with_options(input, options)?;
    if values.len() != 1 {
        return Err(YamlError::at_zero(YamlErrorKind::TrailingDocument));
    }
    let mut reader = YamlReader::from_values(values, options)?;
    let value = T::decode(&mut reader)?;
    while reader.next_data_event()?.is_some() {}
    reader.finish()?;
    Ok(value)
}

impl Encoder<YamlCodec, YamlError> for YamlWriter {
    fn write_event(&mut self, event: Event) -> Result<(), YamlError> {
        match event {
            Event::Field(value) => {
                self.write(YamlEvent::MappingKey)?;
                self.write(YamlEvent::Scalar(YamlScalar::Text(value)))
            }
            Event::Null => self.write(YamlEvent::Scalar(YamlScalar::Null)),
            Event::Bool(value) => self.write(YamlEvent::Scalar(YamlScalar::Bool(value))),
            Event::Int(value) => self.write(YamlEvent::Scalar(YamlScalar::Int(
                i64::try_from(value)
                    .map_err(|_| YamlError::at_zero(YamlErrorKind::NumberOutOfRange))?,
            ))),
            Event::UInt(value) => self.write(YamlEvent::Scalar(YamlScalar::UInt(
                u64::try_from(value)
                    .map_err(|_| YamlError::at_zero(YamlErrorKind::NumberOutOfRange))?,
            ))),
            Event::Float(value) => self.write(YamlEvent::Scalar(YamlScalar::Float(value))),
            Event::Float32(value) => self.write(YamlEvent::Scalar(YamlScalar::Float(
                f32::from_bits(value) as f64,
            ))),
            Event::Float64(value) => {
                self.write(YamlEvent::Scalar(YamlScalar::Float(f64::from_bits(value))))
            }
            Event::String(value) => self.write(YamlEvent::Scalar(YamlScalar::Text(value))),
            Event::Bytes(value) => self.write(YamlEvent::Scalar(YamlScalar::Bytes(value))),
            Event::StartArray(length) => {
                self.write(YamlEvent::SequenceStart(length.map(|_| String::new())))
            }
            Event::EndArray => self.write(YamlEvent::SequenceEnd),
            Event::StartMap(length) => {
                self.write(YamlEvent::MappingStart(length.map(|_| String::new())))
            }
            Event::MapKey => self.write(YamlEvent::MappingKey),
            Event::EndMap => self.write(YamlEvent::MappingEnd),
            Event::StartRecord { .. } => self.write(YamlEvent::MappingStart(None)),
            Event::EndRecord => self.write(YamlEvent::MappingEnd),
            Event::StartEnum { .. } | Event::EndEnum => {
                Err(YamlError::at_zero(YamlErrorKind::TypeMismatch))
            }
        }
    }
}

impl Decoder<YamlCodec, YamlError> for YamlReader {
    fn limits(&self) -> serialization::Limits {
        self.serialization_limits()
    }

    fn peek_event(&mut self) -> Result<Option<Event>, YamlError> {
        let mut clone = self.clone();
        clone
            .next_data_event()?
            .map(yaml_event_to_event)
            .transpose()
    }

    fn next(&mut self) -> Result<Option<Event>, YamlError> {
        self.next_data_event()?.map(yaml_event_to_event).transpose()
    }

    fn reject(&mut self, error: SerializationError) -> YamlError {
        error.into()
    }
}

impl From<YamlValue> for serialization::Value {
    fn from(value: YamlValue) -> Self {
        match value {
            YamlValue::Null => Self::Null,
            YamlValue::Bool(value) => Self::Bool(value),
            YamlValue::Int(value) => Self::Int(value),
            YamlValue::UInt(value) => Self::UInt(value),
            YamlValue::Float(value) => Self::Float64(value.to_bits()),
            YamlValue::Text(value) => Self::String(value),
            YamlValue::Bytes(value) => Self::Bytes(value),
            YamlValue::Array(values) => Self::Array(values.into_iter().map(Self::from).collect()),
            YamlValue::Object(members) => Self::Object(
                members
                    .into_iter()
                    .map(|member| (member.key, Self::from(member.value)))
                    .collect(),
            ),
        }
    }
}

impl TryFrom<serialization::Value> for YamlValue {
    type Error = YamlError;

    fn try_from(value: serialization::Value) -> Result<Self, Self::Error> {
        match value {
            serialization::Value::Null => Ok(Self::Null),
            serialization::Value::Bool(value) => Ok(Self::Bool(value)),
            serialization::Value::Int(value) => Ok(Self::Int(value)),
            serialization::Value::UInt(value) => Ok(Self::UInt(value)),
            serialization::Value::Float32(bits) => Ok(Self::Float(f32::from_bits(bits) as f64)),
            serialization::Value::Float64(bits) => Ok(Self::Float(f64::from_bits(bits))),
            serialization::Value::Number(value) => {
                let (scalar, spelling) = scalar_from_text(&value, false, usize::MAX)
                    .map_err(|kind| YamlError::at_zero(kind))?;
                Ok(scalar_to_value(&scalar_with_spelling(scalar, spelling)))
            }
            serialization::Value::String(value) => Ok(Self::Text(value)),
            serialization::Value::Bytes(value) => Ok(Self::Bytes(value)),
            serialization::Value::Array(values) => Ok(Self::Array(
                values
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?,
            )),
            serialization::Value::Object(members) => Ok(Self::Object(
                members
                    .into_iter()
                    .map(|(key, value)| {
                        Ok(YamlMember {
                            key,
                            value: Self::try_from(value)?,
                        })
                    })
                    .collect::<Result<_, YamlError>>()?,
            )),
            serialization::Value::Map(_) | serialization::Value::Extension { .. } => {
                Err(YamlError::at_zero(YamlErrorKind::TypeMismatch))
            }
        }
    }
}

fn scalar_with_spelling(scalar: YamlScalar, _spelling: String) -> YamlScalar {
    scalar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_block_flow_and_quotes_round_trip() {
        let input = r#"name: Tondo
enabled: true
count: 42
items:
  - one
  - [two, "tres 🚀"]
quoted: 'true'
"#
        .as_bytes();
        let value = parse(input).unwrap();
        assert!(matches!(value, YamlValue::Object(_)));
        let encoded = encode(&value, YamlOptions::default()).unwrap();
        let reparsed = parse(&encoded).unwrap();
        assert_eq!(value, reparsed);
    }

    #[test]
    fn aliases_are_copied_and_cycles_are_rejected() {
        let value = parse(b"base: &item\n  name: foo\ncopy: *item\n").unwrap();
        let YamlValue::Object(members) = value else {
            panic!("mapping")
        };
        assert_eq!(members[0].value, members[1].value);
        assert_eq!(
            parse(b"a: &a *a\n").unwrap_err().kind,
            YamlErrorKind::AliasCycle
        );
        assert_eq!(
            parse(b"a: *missing\n").unwrap_err().kind,
            YamlErrorKind::UndefinedAlias
        );
    }

    #[test]
    fn tags_security_and_limits_are_enforced() {
        assert_eq!(parse(b"yes\n").unwrap(), YamlValue::Text("yes".into()));
        assert!(matches!(
            parse(b"!!binary SGVsbG8=\n").unwrap(),
            YamlValue::Bytes(_)
        ));
        assert_eq!(
            parse(b"!custom value\n").unwrap_err().kind,
            YamlErrorKind::InvalidTag
        );
        assert_eq!(
            parse(b"a: 1\na: 2\n").unwrap_err().kind,
            YamlErrorKind::DuplicateKey
        );
        assert_eq!(
            parse(b"{1: a}\n").unwrap_err().kind,
            YamlErrorKind::NonStringKey
        );
        let options = YamlOptions::create(YamlLimits {
            max_nodes: 1,
            ..YamlLimits::default()
        });
        assert_eq!(
            parse_with_options(b"[1]", options).unwrap_err().kind,
            YamlErrorKind::NodeLimit
        );
    }

    #[test]
    fn canonical_order_and_reader_lifecycle_are_stable() {
        let value = YamlValue::Object(vec![
            YamlMember {
                key: "z".into(),
                value: YamlValue::Int(1),
            },
            YamlMember {
                key: "a".into(),
                value: YamlValue::Int(2),
            },
        ]);
        assert_eq!(
            encode_canonical(&value, YamlLimits::default()).unwrap(),
            b"a: 2\nz: 1\n"
        );
        let mut reader = YamlReader::from_bytes(b"a: 1\n", YamlOptions::default()).unwrap();
        while reader.next().unwrap().is_some() {}
        assert!(matches!(
            reader.next(),
            Err(YamlError {
                kind: YamlErrorKind::Closed,
                ..
            })
        ));
        assert!(matches!(
            reader.finish(),
            Err(YamlError {
                kind: YamlErrorKind::Closed,
                ..
            })
        ));
    }

    #[test]
    fn typed_static_protocol_uses_common_events() {
        let input = encode_static(&vec![1_i64, 2_i64], YamlOptions::default()).unwrap();
        assert_eq!(
            parse(&input).unwrap(),
            YamlValue::Array(vec![YamlValue::Int(1), YamlValue::Int(2)])
        );
        let output = decode_static::<Vec<i64>>(&input, YamlOptions::default()).unwrap();
        assert_eq!(output, vec![1, 2]);
    }
}
