//! Independent finite-domain regex grammar and ordered path oracle.
//!
//! This reference deliberately enumerates bounded paths instead of compiling
//! an automaton. Limit and OutsideDomain never imply production rejection.

#[path = "regex_model/grammar.rs"]
mod grammar;

pub const MAX_REFERENCE_PATTERN_BYTES: usize = 128;
pub const MAX_REFERENCE_NODES: usize = 128;
pub const MAX_REFERENCE_DEPTH: usize = 8;
pub const MAX_REFERENCE_CAPTURES: usize = 8;
pub const MAX_REFERENCE_REPEAT: usize = 8;
pub const MAX_REFERENCE_INPUT_BYTES: usize = 96;
pub const MAX_REFERENCE_INPUT_SCALARS: usize = 32;
pub const MAX_REFERENCE_PATH_STEPS: usize = 65_536;
pub const MAX_REFERENCE_OUTPUT_BYTES: usize = 4096;
pub const MAX_REFERENCE_REPLACEMENT_BYTES: usize = 128;
pub const MAX_REGEX_FUZZ_INPUT_BYTES: usize = 4096;
pub const MAX_REGEX_FUZZ_STEPS: usize = 512;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceOptions {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_newline: bool,
    pub crlf: bool,
    pub ungreedy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
    Syntax,
    InvalidReplacement,
    OutsideDomain,
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceError {
    pub kind: ReferenceErrorKind,
    pub offset: usize,
}

fn error(kind: ReferenceErrorKind, offset: usize) -> ReferenceError {
    ReferenceError { kind, offset }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceMatch {
    pub span: ReferenceSpan,
    pub captures: Vec<Option<ReferenceSpan>>,
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Literal(char),
    Dot,
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Start,
    End,
    AbsoluteStart,
    AbsoluteEnd,
    WordBoundary,
    Sequence(Vec<Self>),
    Alternative(Vec<Self>),
    Capture(usize, Box<Self>),
    Group(Box<Self>),
    Repeat {
        body: Box<Self>,
        min: usize,
        max: Option<usize>,
        greedy: bool,
    },
}

impl Node {
    fn empty_atom(&self) -> bool {
        match self {
            Self::Empty => true,
            Self::Capture(_, body) | Self::Group(body) => body.empty_atom(),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReferenceRegex {
    root: Node,
    options: ReferenceOptions,
    names: Vec<Option<String>>,
    ascii_tables: bool,
}

impl ReferenceRegex {
    pub fn compile(pattern: &str, options: ReferenceOptions) -> Result<Self, ReferenceError> {
        grammar::parse(pattern, options)
    }

    pub fn capture_count(&self) -> usize {
        self.names.len() - 1
    }

    pub fn capture_names(&self) -> &[Option<String>] {
        &self.names
    }

    fn input(&self, text: &str) -> Result<Input, ReferenceError> {
        if text.len() > MAX_REFERENCE_INPUT_BYTES {
            return Err(error(ReferenceErrorKind::Limit, 0));
        }
        if (self.ascii_tables || self.options.case_insensitive) && !text.is_ascii() {
            return Err(error(ReferenceErrorKind::OutsideDomain, 0));
        }
        let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        if offsets.len() > MAX_REFERENCE_INPUT_SCALARS {
            return Err(error(ReferenceErrorKind::Limit, 0));
        }
        offsets.push(text.len());
        Ok(Input {
            scalars: text.chars().collect(),
            offsets,
        })
    }

    pub fn find(&self, text: &str) -> Result<Option<ReferenceMatch>, ReferenceError> {
        self.search(&self.input(text)?, 0, false, &mut Budget::default())
    }

    pub fn full_match(&self, text: &str) -> Result<Option<ReferenceMatch>, ReferenceError> {
        self.search(&self.input(text)?, 0, true, &mut Budget::default())
    }

    fn search(
        &self,
        input: &Input,
        start: usize,
        full: bool,
        budget: &mut Budget,
    ) -> Result<Option<ReferenceMatch>, ReferenceError> {
        let last = if full { 0 } else { input.scalars.len() };
        for position in start..=last {
            let initial = State {
                position,
                captures: vec![None; self.names.len()],
            };
            let evaluation = Evaluation {
                input,
                options: self.options,
            };
            for state in evaluation.node(&self.root, initial, budget)? {
                if full && state.position != input.scalars.len() {
                    continue;
                }
                let span = ReferenceSpan {
                    start: input.offsets[position],
                    end: input.offsets[state.position],
                };
                let mut captures: Vec<_> = state
                    .captures
                    .iter()
                    .map(|capture| {
                        capture.map(|(start, end)| ReferenceSpan {
                            start: input.offsets[start],
                            end: input.offsets[end],
                        })
                    })
                    .collect();
                captures[0] = Some(span);
                return Ok(Some(ReferenceMatch { span, captures }));
            }
        }
        Ok(None)
    }

    pub fn find_all(&self, text: &str) -> Result<Vec<ReferenceMatch>, ReferenceError> {
        let input = self.input(text)?;
        let mut budget = Budget::default();
        let mut cursor = 0;
        let mut matches = Vec::new();
        while let Some(found) = self.search(&input, cursor, false, &mut budget)? {
            let empty = found.span.start == found.span.end;
            let end = input
                .offsets
                .binary_search(&found.span.end)
                .expect("reference scalar offset");
            matches.push(found);
            if empty && end == input.scalars.len() {
                break;
            }
            cursor = end + usize::from(empty);
        }
        Ok(matches)
    }

    pub fn replace(
        &self,
        text: &str,
        replacement: &str,
        all: bool,
    ) -> Result<String, ReferenceError> {
        let tokens = self.replacement(replacement)?;
        let matches = if all {
            self.find_all(text)?
        } else {
            self.find(text)?.into_iter().collect()
        };
        let mut output = String::new();
        let mut cursor = 0;
        for found in matches {
            append(&mut output, &text[cursor..found.span.start])?;
            for token in &tokens {
                match token {
                    Token::Text(value) => append(&mut output, value)?,
                    Token::Capture(index) => {
                        if let Some(span) = found.captures[*index] {
                            append(&mut output, &text[span.start..span.end])?;
                        }
                    }
                }
            }
            cursor = found.span.end;
        }
        append(&mut output, &text[cursor..])?;
        Ok(output)
    }

    fn replacement(&self, text: &str) -> Result<Vec<Token>, ReferenceError> {
        if text.len() > MAX_REFERENCE_REPLACEMENT_BYTES {
            return Err(error(ReferenceErrorKind::Limit, 0));
        }
        let mut tokens = Vec::new();
        let mut offset = 0;
        while offset < text.len() {
            let scalar = text[offset..].chars().next().expect("remaining template");
            if scalar != '$' {
                tokens.push(Token::Text(scalar.to_string()));
                offset += scalar.len_utf8();
                continue;
            }
            let start = offset;
            offset += 1;
            let invalid = || error(ReferenceErrorKind::InvalidReplacement, start);
            match text.as_bytes().get(offset) {
                Some(b'$') => {
                    tokens.push(Token::Text("$".into()));
                    offset += 1;
                }
                Some(b'{') => {
                    offset += 1;
                    let end = text[offset..]
                        .find('}')
                        .map(|end| offset + end)
                        .ok_or_else(invalid)?;
                    let name = &text[offset..end];
                    let index = self
                        .names
                        .iter()
                        .position(|item| item.as_deref() == Some(name))
                        .ok_or_else(invalid)?;
                    tokens.push(Token::Capture(index));
                    offset = end + 1;
                }
                Some(b'0'..=b'9') => {
                    let mut index = 0usize;
                    while let Some(byte @ b'0'..=b'9') = text.as_bytes().get(offset) {
                        index = index
                            .checked_mul(10)
                            .and_then(|index| index.checked_add(usize::from(byte - b'0')))
                            .ok_or_else(invalid)?;
                        offset += 1;
                    }
                    if index >= self.names.len() {
                        return Err(invalid());
                    }
                    tokens.push(Token::Capture(index));
                }
                _ => return Err(invalid()),
            }
        }
        Ok(tokens)
    }
}

enum Token {
    Text(String),
    Capture(usize),
}

fn append(output: &mut String, value: &str) -> Result<(), ReferenceError> {
    if output.len().saturating_add(value.len()) > MAX_REFERENCE_OUTPUT_BYTES {
        return Err(error(ReferenceErrorKind::Limit, 0));
    }
    output.push_str(value);
    Ok(())
}

struct Input {
    scalars: Vec<char>,
    offsets: Vec<usize>,
}

#[derive(Clone)]
struct State {
    position: usize,
    captures: Vec<Option<(usize, usize)>>,
}

#[derive(Default)]
struct Budget {
    steps: usize,
}

impl Budget {
    fn charge(&mut self) -> Result<(), ReferenceError> {
        if self.steps == MAX_REFERENCE_PATH_STEPS {
            return Err(error(ReferenceErrorKind::Limit, 0));
        }
        self.steps += 1;
        Ok(())
    }
}

struct Evaluation<'a> {
    input: &'a Input,
    options: ReferenceOptions,
}

impl Evaluation<'_> {
    fn node(
        &self,
        node: &Node,
        state: State,
        budget: &mut Budget,
    ) -> Result<Vec<State>, ReferenceError> {
        budget.charge()?;
        let position = state.position;
        let scalar = self.input.scalars.get(position).copied();
        let consuming = match node {
            Node::Literal(expected) => Some(scalar.is_some_and(|value| {
                if self.options.case_insensitive {
                    value.eq_ignore_ascii_case(expected)
                } else {
                    value == *expected
                }
            })),
            Node::Dot => Some(scalar.is_some_and(|value| {
                self.options.dot_matches_newline
                    || (value != '\n' && (!self.options.crlf || value != '\r'))
            })),
            Node::Class { negated, ranges } => Some(scalar.is_some_and(|value| {
                let member = |value| {
                    ranges
                        .iter()
                        .any(|(start, end)| *start <= value && value <= *end)
                };
                let present = member(value)
                    || (self.options.case_insensitive
                        && (member(value.to_ascii_lowercase())
                            || member(value.to_ascii_uppercase())));
                present != *negated
            })),
            _ => None,
        };
        if let Some(accepts) = consuming {
            return Ok(if accepts {
                vec![State {
                    position: position + 1,
                    ..state
                }]
            } else {
                Vec::new()
            });
        }
        match node {
            Node::Empty => Ok(vec![state]),
            Node::Start
            | Node::End
            | Node::AbsoluteStart
            | Node::AbsoluteEnd
            | Node::WordBoundary => Ok(if self.anchor(node, position) {
                vec![state]
            } else {
                Vec::new()
            }),
            Node::Group(body) => self.node(body, state, budget),
            Node::Capture(index, body) => {
                let mut states = self.node(body, state, budget)?;
                for state in &mut states {
                    state.captures[*index] = Some((position, state.position));
                }
                Ok(states)
            }
            Node::Sequence(nodes) => {
                let mut states = vec![state];
                for node in nodes {
                    let mut next = Vec::new();
                    for state in states {
                        next.extend(self.node(node, state, budget)?);
                    }
                    states = next;
                }
                Ok(states)
            }
            Node::Alternative(nodes) => {
                let mut states = Vec::new();
                for node in nodes {
                    states.extend(self.node(node, state.clone(), budget)?);
                }
                Ok(states)
            }
            Node::Repeat {
                body,
                min,
                max,
                greedy,
            } => self.repeat(
                body,
                state,
                Repeat {
                    count: 0,
                    min: *min,
                    max: *max,
                    greedy: *greedy,
                    blocked: false,
                },
                budget,
            ),
            Node::Literal(_) | Node::Dot | Node::Class { .. } => unreachable!("consuming branch"),
        }
    }

    fn anchor(&self, node: &Node, position: usize) -> bool {
        let current = self.input.scalars.get(position).copied();
        let previous = position
            .checked_sub(1)
            .and_then(|index| self.input.scalars.get(index))
            .copied();
        let following = self.input.scalars.get(position + 1).copied();
        let last = position == self.input.scalars.len();
        let word = |value: Option<char>| {
            value.is_some_and(|value| value.is_ascii_alphanumeric() || value == '_')
        };
        match node {
            Node::Start => position == 0 || (self.options.multi_line && previous == Some('\n')),
            Node::End => {
                last || (self.options.multi_line
                    && ((current == Some('\n') && (!self.options.crlf || previous != Some('\r')))
                        || (self.options.crlf && current == Some('\r') && following == Some('\n'))))
            }
            Node::AbsoluteStart => position == 0,
            Node::AbsoluteEnd => last,
            Node::WordBoundary => word(previous) != word(current),
            _ => unreachable!("anchor branch"),
        }
    }

    fn repeat(
        &self,
        body: &Node,
        state: State,
        repetition: Repeat,
        budget: &mut Budget,
    ) -> Result<Vec<State>, ReferenceError> {
        budget.charge()?;
        let mut states = Vec::new();
        let exit = repetition.count >= repetition.min;
        if !repetition.greedy && exit {
            states.push(state.clone());
        }
        if !repetition.blocked && repetition.max.is_none_or(|max| repetition.count < max) {
            for next in self.node(body, state.clone(), budget)? {
                let blocked = repetition.max.is_none()
                    && next.position == state.position
                    && repetition.count + 1 >= repetition.min;
                states.extend(self.repeat(
                    body,
                    next,
                    Repeat {
                        count: repetition.count + 1,
                        blocked,
                        ..repetition
                    },
                    budget,
                )?);
            }
        }
        if repetition.greedy && exit {
            states.push(state);
        }
        Ok(states)
    }
}

#[derive(Clone, Copy)]
struct Repeat {
    count: usize,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCase {
    pub pattern: String,
    pub input: String,
    pub replacement: String,
    pub options: ReferenceOptions,
}

pub fn case_from_seed(mut seed: u64) -> GeneratedCase {
    const PATTERNS: &[&str] = &[
        "",
        "a",
        "ab",
        ".",
        "[ab]",
        "[^a]",
        "[a-c]",
        "a|ab",
        "ab|a",
        "a*?a*",
        "(a|ab)+",
        "(a|(b))+",
        "(a)?(b*)",
        "(?<n>a*)",
        "((a?)*)",
        "(a*)*",
        "(?:a?)*",
        "a{0,2}",
        "a{1,3}?",
        "(?:ab|a){1,2}",
        "(a|b)*",
        "(a(b)?)",
        "^a*$",
        r"\Aa*\z",
        r"\b\w+\b",
        r"\d+",
        r"\s*",
        "[a-zA-Z_0-9]+",
        "a*b|a",
        "(a|)(b?)",
        "(?<n>a|b)?",
        "(a*?)(a*)",
        r"\b{2}",
        r"\x{61}+",
        r"[\-\]]",
        "é|π",
        "(é*)",
        "[é-π]+",
        "🙂?",
        "(?:中|a)+",
    ];
    let mut next = || {
        seed = seed.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = seed;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^ (value >> 31)
    };
    let pattern = PATTERNS[next() as usize % PATTERNS.len()].to_owned();
    let flags = next();
    let ascii = pattern.contains(r"\b")
        || pattern.contains(r"\w")
        || pattern.contains(r"\d")
        || pattern.contains(r"\s");
    let options = ReferenceOptions {
        case_insensitive: flags & 1 != 0 && pattern.is_ascii(),
        multi_line: flags & 2 != 0,
        dot_matches_newline: flags & 4 != 0,
        crlf: flags & 8 != 0,
        ungreedy: flags & 16 != 0,
    };
    let alphabet: &[char] = if ascii || options.case_insensitive {
        &['a', 'b', 'A', '0', '_', ' ', '\n', '\r']
    } else {
        &['a', 'b', 'é', 'π', '中', '🙂', ' ', '\n', '\r']
    };
    let input = (0..next() % 6)
        .map(|_| alphabet[next() as usize % alphabet.len()])
        .collect();
    GeneratedCase {
        pattern,
        input,
        replacement: "[$0]$$".into(),
        options,
    }
}

pub fn seed_at_step(input: &[u8], step: usize) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in input.iter().take(MAX_REGEX_FUZZ_INPUT_BYTES) {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    hash ^ (step as u64).wrapping_mul(0x9e3779b97f4a7c15)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySummary {
    pub steps: usize,
    pub matches: usize,
    pub output_bytes: usize,
    pub digest: u64,
}

pub fn run_regex_fuzz_case(input: &[u8]) -> Result<ReplaySummary, ReferenceError> {
    let steps = input
        .len()
        .min(MAX_REGEX_FUZZ_INPUT_BYTES)
        .clamp(1, MAX_REGEX_FUZZ_STEPS);
    let mut summary = ReplaySummary {
        steps,
        matches: 0,
        output_bytes: 0,
        digest: 0xcbf29ce484222325,
    };
    for step in 0..steps {
        let case = case_from_seed(seed_at_step(input, step));
        let regex = ReferenceRegex::compile(&case.pattern, case.options)?;
        let matches = regex.find_all(&case.input)?;
        let output = regex.replace(&case.input, &case.replacement, true)?;
        summary.matches += matches.len();
        summary.output_bytes += output.len();
        for byte in output.bytes() {
            summary.digest = (summary.digest ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
        for found in matches {
            summary.digest ^= (found.span.start as u64).wrapping_mul(31) ^ found.span.end as u64;
        }
    }
    Ok(summary)
}
