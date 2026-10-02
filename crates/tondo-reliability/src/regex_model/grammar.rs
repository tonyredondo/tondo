use super::{
    MAX_REFERENCE_CAPTURES, MAX_REFERENCE_DEPTH, MAX_REFERENCE_NODES, MAX_REFERENCE_PATTERN_BYTES,
    MAX_REFERENCE_REPEAT, Node, ReferenceError, ReferenceErrorKind as Kind, ReferenceOptions,
    ReferenceRegex, error,
};

struct Parser<'a> {
    pattern: &'a str,
    offset: usize,
    nodes: usize,
    names: Vec<Option<String>>,
    options: ReferenceOptions,
    ascii_tables: bool,
    non_ascii_literal: bool,
}

pub(super) fn parse(
    pattern: &str,
    options: ReferenceOptions,
) -> Result<ReferenceRegex, ReferenceError> {
    if pattern.len() > MAX_REFERENCE_PATTERN_BYTES {
        return Err(error(Kind::Limit, 0));
    }
    let mut parser = Parser {
        pattern,
        offset: 0,
        nodes: 0,
        names: vec![None],
        options,
        ascii_tables: false,
        non_ascii_literal: false,
    };
    let root = parser.alternation(0)?;
    if parser.offset != pattern.len() {
        return Err(error(Kind::Syntax, parser.offset));
    }
    if options.case_insensitive && parser.non_ascii_literal {
        return Err(error(Kind::OutsideDomain, 0));
    }
    Ok(ReferenceRegex {
        root,
        options,
        names: parser.names,
        ascii_tables: parser.ascii_tables,
    })
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.pattern[self.offset..].chars().next()
    }

    fn take(&mut self) -> Option<char> {
        let scalar = self.peek()?;
        self.offset += scalar.len_utf8();
        Some(scalar)
    }

    fn syntax(&self) -> ReferenceError {
        error(Kind::Syntax, self.offset)
    }

    fn node(&mut self, node: Node) -> Result<Node, ReferenceError> {
        if self.nodes == MAX_REFERENCE_NODES {
            return Err(error(Kind::Limit, self.offset));
        }
        self.nodes += 1;
        Ok(node)
    }

    fn literal(&mut self, value: char) -> Result<Node, ReferenceError> {
        self.non_ascii_literal |= !value.is_ascii();
        self.node(Node::Literal(value))
    }

    fn alternation(&mut self, depth: usize) -> Result<Node, ReferenceError> {
        if depth >= MAX_REFERENCE_DEPTH {
            return Err(error(Kind::Limit, self.offset));
        }
        let mut branches = vec![self.sequence(depth)?];
        while self.peek() == Some('|') {
            self.take();
            branches.push(self.sequence(depth)?);
        }
        if branches.len() == 1 {
            Ok(branches.remove(0))
        } else {
            self.node(Node::Alternative(branches))
        }
    }

    fn sequence(&mut self, depth: usize) -> Result<Node, ReferenceError> {
        let mut nodes = Vec::new();
        while !matches!(self.peek(), None | Some('|' | ')')) {
            let start = self.offset;
            let atom = self.atom(depth)?;
            nodes.push(self.quantifier(atom, start)?);
        }
        match nodes.len() {
            0 => self.node(Node::Empty),
            1 => Ok(nodes.remove(0)),
            _ => self.node(Node::Sequence(nodes)),
        }
    }

    fn atom(&mut self, depth: usize) -> Result<Node, ReferenceError> {
        let start = self.offset;
        match self.take().ok_or_else(|| self.syntax())? {
            '.' => self.node(Node::Dot),
            '^' => self.node(Node::Start),
            '$' => self.node(Node::End),
            '\\' => self.escape(false),
            '[' => self.class(),
            '(' => self.group(depth),
            '*' | '+' | '?' | '{' | '}' | ']' => Err(error(Kind::Syntax, start)),
            value => self.literal(value),
        }
    }

    fn group(&mut self, depth: usize) -> Result<Node, ReferenceError> {
        let capture = if self.peek() == Some('?') {
            self.take();
            match self.take() {
                Some(':') => None,
                Some('<') if !matches!(self.peek(), Some('=' | '!')) => {
                    let start = self.offset;
                    while !matches!(self.peek(), Some('>') | None) {
                        self.take();
                    }
                    let name = &self.pattern[start..self.offset];
                    if self.take() != Some('>')
                        || name.is_empty()
                        || !name.as_bytes().iter().enumerate().all(|(index, byte)| {
                            *byte == b'_'
                                || byte.is_ascii_alphabetic()
                                || (index > 0 && byte.is_ascii_digit())
                        })
                        || self.names.iter().any(|item| item.as_deref() == Some(name))
                    {
                        return Err(error(Kind::Syntax, start));
                    }
                    Some(self.capture(Some(name.to_owned()))?)
                }
                _ => return Err(error(Kind::OutsideDomain, self.offset)),
            }
        } else {
            Some(self.capture(None)?)
        };
        let body = self.alternation(depth + 1)?;
        if self.take() != Some(')') {
            return Err(self.syntax());
        }
        self.node(match capture {
            Some(index) => Node::Capture(index, Box::new(body)),
            None => Node::Group(Box::new(body)),
        })
    }

    fn capture(&mut self, name: Option<String>) -> Result<usize, ReferenceError> {
        if self.names.len() > MAX_REFERENCE_CAPTURES {
            return Err(error(Kind::Limit, self.offset));
        }
        let index = self.names.len();
        self.names.push(name);
        Ok(index)
    }

    fn quantifier(&mut self, atom: Node, start: usize) -> Result<Node, ReferenceError> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.take();
                (0, None)
            }
            Some('+') => {
                self.take();
                (1, None)
            }
            Some('?') => {
                self.take();
                (0, Some(1))
            }
            Some('{') => {
                self.take();
                let min = self.number()?;
                let max = if self.peek() == Some(',') {
                    self.take();
                    if self.peek() == Some('}') {
                        None
                    } else {
                        Some(self.number()?)
                    }
                } else {
                    Some(min)
                };
                if self.take() != Some('}')
                    || max.is_some_and(|max| min > max)
                    || (max == Some(0) && atom.empty_atom())
                {
                    return Err(error(Kind::Syntax, start));
                }
                (min, max)
            }
            _ => return Ok(atom),
        };
        let lazy = self.peek() == Some('?');
        if lazy {
            self.take();
        }
        if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return Err(self.syntax());
        }
        self.node(Node::Repeat {
            body: Box::new(atom),
            min,
            max,
            greedy: lazy == self.options.ungreedy,
        })
    }

    fn number(&mut self) -> Result<usize, ReferenceError> {
        let start = self.offset;
        while self.peek().is_some_and(|value| value.is_ascii_digit()) {
            self.take();
        }
        let value = self.pattern[start..self.offset]
            .parse::<usize>()
            .map_err(|_| self.syntax())?;
        if value > MAX_REFERENCE_REPEAT {
            Err(error(Kind::Limit, start))
        } else {
            Ok(value)
        }
    }

    fn escape(&mut self, in_class: bool) -> Result<Node, ReferenceError> {
        let start = self.offset.saturating_sub(1);
        match self.take().ok_or_else(|| self.syntax())? {
            value @ ('\\' | '.' | '^' | '$' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | '*'
            | '+' | '?' | '-') => self.literal(value),
            'a' => self.literal('\u{7}'),
            'f' => self.literal('\u{c}'),
            'n' => self.literal('\n'),
            'r' => self.literal('\r'),
            't' => self.literal('\t'),
            'v' => self.literal('\u{b}'),
            'x' => {
                if self.take() != Some('{') {
                    return Err(error(Kind::Syntax, start));
                }
                let digits = self.offset;
                while self.peek().is_some_and(|value| value.is_ascii_hexdigit()) {
                    self.take();
                }
                let hex = &self.pattern[digits..self.offset];
                if hex.is_empty() || hex.len() > 6 || self.take() != Some('}') {
                    return Err(error(Kind::Syntax, start));
                }
                let scalar = u32::from_str_radix(hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| error(Kind::Syntax, start))?;
                self.literal(scalar)
            }
            'b' if !in_class => {
                self.ascii_tables = true;
                self.node(Node::WordBoundary)
            }
            'A' if !in_class => self.node(Node::AbsoluteStart),
            'z' if !in_class => self.node(Node::AbsoluteEnd),
            value @ ('d' | 'D' | 's' | 'S' | 'w' | 'W') => {
                self.ascii_tables = true;
                let lower = value.to_ascii_lowercase();
                let ranges = (0..=127u8)
                    .filter_map(|byte| {
                        let scalar = char::from(byte);
                        let accepts = match lower {
                            'd' => scalar.is_ascii_digit(),
                            's' => matches!(scalar, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}'),
                            'w' => scalar.is_ascii_alphanumeric() || scalar == '_',
                            _ => unreachable!("shorthand"),
                        };
                        accepts.then_some((scalar, scalar))
                    })
                    .collect();
                self.node(Node::Class {
                    negated: value.is_ascii_uppercase(),
                    ranges,
                })
            }
            'p' | 'P' | 'B' | '0'..='9' => Err(error(Kind::OutsideDomain, start)),
            _ => Err(error(Kind::Syntax, start)),
        }
    }

    fn class_item(&mut self) -> Result<Node, ReferenceError> {
        match self.take().ok_or_else(|| self.syntax())? {
            '\\' => self.escape(true),
            '[' => Err(error(Kind::OutsideDomain, self.offset - 1)),
            value => self.literal(value),
        }
    }

    fn class(&mut self) -> Result<Node, ReferenceError> {
        let negated = self.peek() == Some('^');
        if negated {
            self.take();
        }
        let mut ranges = Vec::new();
        while !matches!(self.peek(), Some(']') | None) {
            if ["&&", "--", "~~"]
                .iter()
                .any(|prefix| self.pattern[self.offset..].starts_with(prefix))
            {
                return Err(error(Kind::OutsideDomain, self.offset));
            }
            if self.peek() == Some('-')
                && !ranges.is_empty()
                && !self.pattern[self.offset..].starts_with("-]")
            {
                return Err(self.syntax());
            }
            let item = self.class_item()?;
            match item {
                Node::Literal(start) => {
                    if self.peek() == Some('-') && !self.pattern[self.offset..].starts_with("-]") {
                        self.take();
                        let Node::Literal(end) = self.class_item()? else {
                            return Err(self.syntax());
                        };
                        if start > end {
                            return Err(self.syntax());
                        }
                        ranges.push((start, end));
                    } else {
                        ranges.push((start, start));
                    }
                }
                Node::Class {
                    negated: false,
                    ranges: items,
                } => ranges.extend(items),
                Node::Class {
                    negated: true,
                    ranges: items,
                } => {
                    ranges.extend((0..=127u8).filter_map(|byte| {
                        let value = char::from(byte);
                        (!items
                            .iter()
                            .any(|(start, end)| *start <= value && value <= *end))
                        .then_some((value, value))
                    }));
                }
                _ => return Err(self.syntax()),
            }
        }
        if ranges.is_empty() || self.take() != Some(']') {
            return Err(self.syntax());
        }
        self.node(Node::Class { negated, ranges })
    }
}
