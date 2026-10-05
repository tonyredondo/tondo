//! Ordered Thompson threads; priority is structural, never a longest-match sort.

use super::{
    RegexError, RegexErrorKind as Kind, RegexLimits, RegexMatch, RegexPhase as Phase, RegexSpan,
    reserve,
};
use regex_syntax::hir::{Class, ClassUnicode, Hir, HirKind, Look};
use std::{mem::size_of, sync::Arc};

#[derive(Debug)]
enum Matcher {
    Literal(char),
    Ranges(Vec<(char, char)>),
}
impl Matcher {
    fn matches(&self, value: char) -> bool {
        match self {
            Self::Literal(literal) => *literal == value,
            Self::Ranges(ranges) => contains(ranges, value),
        }
    }
}
fn contains(ranges: &[(char, char)], value: char) -> bool {
    ranges
        .binary_search_by(|&(start, end)| {
            if value < start {
                std::cmp::Ordering::Greater
            } else if value > end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
#[derive(Debug)]
enum Inst {
    Accept,
    Consume(Matcher, usize),
    Split(usize, usize),
    Save(usize, usize),
    Assert(Look, usize),
    LoopStart(usize, usize),
    LoopEnd(usize, usize, usize),
}
enum Work<'a> {
    Node(&'a Hir, usize),
    Concat(&'a [Hir], usize),
    ConcatTail(&'a [Hir]),
    Alternation(usize),
    Capture(usize),
    Loop(usize, bool, bool),
    Repeat(&'a Hir, u32, u32, bool, usize),
    Mandatory(&'a Hir, u32, bool),
    Optional(&'a Hir, u32, u32, bool, usize),
}

#[derive(Debug)]
pub(super) struct Program {
    code: Vec<Inst>,
    depths: Vec<usize>,
    bases: Vec<usize>,
    semantic_states: usize,
    start: usize,
    slots: usize,
    word: Vec<(char, char)>,
    ranges: usize,
}

pub(super) enum SearchMode {
    At(usize),
    Full,
}

fn compile_limit(kind: Kind, id: &'static str) -> RegexError {
    RegexError::limited(kind, Phase::Compile, 0, id)
}

impl Program {
    #[cfg(test)]
    pub(super) fn performance_shape(&self) -> [usize; 4] {
        let range_vectors = self.code.iter().filter(|instruction| {
            matches!(instruction, Inst::Consume(Matcher::Ranges(ranges), _) if !ranges.is_empty())
        }).count() + usize::from(!self.word.is_empty());
        [
            self.code.len(),
            self.semantic_states,
            self.ranges,
            range_vectors,
        ]
    }
    #[cfg(test)]
    pub(super) fn performance_search_storage(&self) -> usize {
        let thread = size_of::<Thread>() + self.slots * size_of::<Option<usize>>();
        self.semantic_states * (thread * 4 + size_of::<usize>()) + self.logical_storage()
    }
    pub(super) fn state_count(&self) -> usize {
        self.code.len()
    }
    pub(super) fn logical_storage(&self) -> usize {
        self.code
            .len()
            .saturating_mul(size_of::<Inst>() + 2 * size_of::<usize>())
            .saturating_add(self.ranges.saturating_mul(size_of::<(char, char)>()))
    }
    fn emit(
        &mut self,
        depth: usize,
        instruction: Inst,
        limits: RegexLimits,
    ) -> Result<usize, RegexError> {
        if self.code.len() == limits.max_program_states {
            return Err(compile_limit(
                Kind::ProgramLimitExceeded,
                "max_program_states",
            ));
        }
        let storage = (self.code.len() + 1)
            .saturating_mul(size_of::<Inst>() + 2 * size_of::<usize>())
            .saturating_add(self.ranges.saturating_mul(size_of::<(char, char)>()));
        if storage > limits.vm_heap {
            return Err(compile_limit(Kind::OutOfMemory, "vm_heap"));
        }
        reserve(&mut self.code, 1, Phase::Compile, 0)?;
        reserve(&mut self.depths, 1, Phase::Compile, 0)?;
        let index = self.code.len();
        self.code.push(instruction);
        self.depths.push(depth);
        Ok(index)
    }
    fn class(
        &mut self,
        class: &ClassUnicode,
        limits: RegexLimits,
    ) -> Result<Vec<(char, char)>, RegexError> {
        self.ranges = self
            .ranges
            .checked_add(class.ranges().len())
            .ok_or_else(|| compile_limit(Kind::ProgramLimitExceeded, "max_class_ranges"))?;
        if self.ranges > limits.max_class_ranges {
            return Err(compile_limit(
                Kind::ProgramLimitExceeded,
                "max_class_ranges",
            ));
        }
        if self.ranges.saturating_mul(size_of::<(char, char)>()) > limits.vm_heap {
            return Err(compile_limit(Kind::OutOfMemory, "vm_heap"));
        }
        let mut result = Vec::new();
        reserve(&mut result, class.ranges().len(), Phase::Compile, 0)?;
        result.extend(
            class
                .ranges()
                .iter()
                .map(|range| (range.start(), range.end())),
        );
        Ok(result)
    }
    pub(super) fn compile(
        hir: &Hir,
        word: ClassUnicode,
        limits: RegexLimits,
    ) -> Result<Self, RegexError> {
        let mut program = Self {
            code: Vec::new(),
            depths: Vec::new(),
            bases: Vec::new(),
            semantic_states: 0,
            start: 0,
            slots: 2,
            word: Vec::new(),
            ranges: 0,
        };
        program.emit(0, Inst::Accept, limits)?;
        let mut work = vec![(Work::Node(hir, 0), 0usize)];
        let mut starts = Vec::new();
        while let Some((operation, depth)) = work.pop() {
            // Both stacks have explicit admission and fallible capacity growth.
            if work
                .len()
                .saturating_add(starts.len())
                .saturating_mul(size_of::<Work<'_>>() + size_of::<usize>())
                > limits.vm_heap
            {
                return Err(compile_limit(Kind::OutOfMemory, "vm_heap"));
            }
            reserve(&mut work, 3, Phase::Compile, 0)?;
            reserve(&mut starts, 1, Phase::Compile, 0)?;
            match operation {
                Work::Node(node, tail) => match node.kind() {
                    HirKind::Empty => starts.push(tail),
                    HirKind::Literal(literal) => {
                        let mut start = tail;
                        let text =
                            std::str::from_utf8(&literal.0).expect("Unicode-only HIR literal");
                        for value in text.chars().rev() {
                            start = program.emit(
                                depth,
                                Inst::Consume(Matcher::Literal(value), start),
                                limits,
                            )?;
                        }
                        starts.push(start);
                    }
                    HirKind::Class(Class::Unicode(class)) => {
                        let ranges = program.class(class, limits)?;
                        starts.push(program.emit(
                            depth,
                            Inst::Consume(Matcher::Ranges(ranges), tail),
                            limits,
                        )?);
                    }
                    HirKind::Class(Class::Bytes(_)) => {
                        return Err(compile_limit(Kind::UnsupportedFeature, "unicode-only"));
                    }
                    HirKind::Look(look) => {
                        if *look == Look::WordUnicode && program.word.is_empty() {
                            program.word = program.class(&word, limits)?;
                        }
                        starts.push(program.emit(depth, Inst::Assert(*look, tail), limits)?);
                    }
                    HirKind::Concat(children) => work.push((Work::Concat(children, tail), depth)),
                    HirKind::Alternation(children) => {
                        reserve(&mut work, children.len(), Phase::Compile, 0)?;
                        work.push((Work::Alternation(children.len()), depth));
                        for child in children.iter().rev() {
                            work.push((Work::Node(child, tail), depth));
                        }
                    }
                    HirKind::Capture(capture) => {
                        let slot = capture.index as usize * 2;
                        program.slots = program.slots.max(slot + 2);
                        let end = program.emit(depth, Inst::Save(slot + 1, tail), limits)?;
                        work.push((Work::Capture(slot), depth));
                        work.push((Work::Node(&capture.sub, end), depth));
                    }
                    HirKind::Repetition(repetition) => {
                        if repetition.min as usize > limits.max_repeat
                            || repetition
                                .max
                                .is_some_and(|max| max as usize > limits.max_repeat)
                        {
                            return Err(compile_limit(Kind::ProgramLimitExceeded, "max_repeat"));
                        }
                        if let Some(max) = repetition.max {
                            work.push((
                                Work::Repeat(
                                    &repetition.sub,
                                    repetition.min,
                                    max - repetition.min,
                                    repetition.greedy,
                                    tail,
                                ),
                                depth,
                            ));
                        } else {
                            let split = program.emit(depth, Inst::Split(tail, tail), limits)?;
                            let end = program.emit(
                                depth + 1,
                                Inst::LoopEnd(depth + 1, split, tail),
                                limits,
                            )?;
                            work.push((
                                Work::Repeat(
                                    &repetition.sub,
                                    repetition.min,
                                    0,
                                    repetition.greedy,
                                    split,
                                ),
                                depth,
                            ));
                            work.push((Work::Loop(split, repetition.greedy, true), depth));
                            work.push((Work::Node(&repetition.sub, end), depth + 1));
                        }
                    }
                },
                Work::Concat(children, tail) => {
                    if let Some((last, previous)) = children.split_last() {
                        work.push((Work::ConcatTail(previous), depth));
                        work.push((Work::Node(last, tail), depth));
                    } else {
                        starts.push(tail);
                    }
                }
                Work::ConcatTail(previous) => {
                    let tail = starts.pop().expect("compiled concat child");
                    work.push((Work::Concat(previous, tail), depth));
                }
                Work::Alternation(count) => {
                    let mut start = starts.pop().expect("nonempty HIR alternation");
                    for _ in 1..count {
                        start = program.emit(
                            depth,
                            Inst::Split(starts.pop().expect("compiled alternative"), start),
                            limits,
                        )?;
                    }
                    starts.push(start);
                }
                Work::Capture(slot) => {
                    let tail = starts.pop().expect("compiled capture body");
                    starts.push(program.emit(depth, Inst::Save(slot, tail), limits)?);
                }
                Work::Loop(split, greedy, guarded) => {
                    let mut child = starts.pop().expect("compiled repetition body");
                    let Inst::Split(tail, _) = program.code[split] else {
                        unreachable!("reserved repetition split");
                    };
                    if guarded {
                        child = program.emit(depth, Inst::LoopStart(depth + 1, child), limits)?;
                    }
                    program.code[split] = if greedy {
                        Inst::Split(child, tail)
                    } else {
                        Inst::Split(tail, child)
                    };
                }
                Work::Repeat(child, mandatory, optional, greedy, tail) => {
                    if optional > 0 {
                        let split = program.emit(depth, Inst::Split(tail, tail), limits)?;
                        work.push((
                            Work::Optional(child, mandatory, optional - 1, greedy, split),
                            depth,
                        ));
                        work.push((Work::Loop(split, greedy, false), depth));
                        work.push((Work::Node(child, tail), depth));
                    } else if mandatory > 0 {
                        work.push((Work::Mandatory(child, mandatory - 1, greedy), depth));
                        work.push((Work::Node(child, tail), depth));
                    } else {
                        starts.push(tail);
                    }
                }
                Work::Mandatory(child, count, greedy) => {
                    let tail = starts.pop().expect("compiled mandatory repetition");
                    work.push((Work::Repeat(child, count, 0, greedy, tail), depth));
                }
                Work::Optional(child, mandatory, optional, greedy, tail) => {
                    work.push((
                        Work::Repeat(child, mandatory, optional, greedy, tail),
                        depth,
                    ));
                }
            }
        }
        program.start = starts.pop().expect("one compiled root");
        reserve(&mut program.bases, program.code.len(), Phase::Compile, 0)?;
        for depth in &program.depths {
            program.bases.push(program.semantic_states);
            program.semantic_states = program
                .semantic_states
                .checked_add(depth + 1)
                .ok_or_else(|| compile_limit(Kind::OutOfMemory, "vm_heap"))?;
        }
        Ok(program)
    }

    fn assertion(&self, look: Look, input: &str, position: usize) -> bool {
        let previous = input[..position].chars().next_back();
        let next = input[position..].chars().next();
        match look {
            Look::Start => position == 0,
            Look::End => position == input.len(),
            Look::StartLF | Look::StartCRLF => position == 0 || previous == Some('\n'),
            Look::EndLF => position == input.len() || next == Some('\n'),
            Look::EndCRLF => {
                position == input.len()
                    || (next == Some('\n') && previous != Some('\r'))
                    || input[position..].starts_with("\r\n")
            }
            Look::WordUnicode => {
                previous.is_some_and(|value| contains(&self.word, value))
                    != next.is_some_and(|value| contains(&self.word, value))
            }
            _ => unreachable!("closed syntax admits only absolute/line/Unicode word anchors"),
        }
    }
}

#[derive(Debug)]
struct Thread {
    pc: usize,
    advanced_depth: usize,
    captures: Vec<Option<usize>>,
}
impl Thread {
    fn duplicate(&self, phase: Phase, offset: usize) -> Result<Self, RegexError> {
        let mut captures = Vec::new();
        reserve(&mut captures, self.captures.len(), phase, offset)?;
        captures.extend_from_slice(&self.captures);
        Ok(Self {
            pc: self.pc,
            advanced_depth: self.advanced_depth,
            captures,
        })
    }
}
fn step(
    steps: &mut usize,
    limits: RegexLimits,
    phase: Phase,
    offset: usize,
) -> Result<(), RegexError> {
    if *steps == limits.max_steps {
        return Err(RegexError::limited(
            Kind::StepLimitExceeded,
            phase,
            offset,
            "max_steps",
        ));
    }
    *steps += 1;
    Ok(())
}

struct Search<'a> {
    program: &'a Program,
    input: &'a str,
    seen: Vec<usize>,
    generation: usize,
    pending: Vec<Thread>,
    limits: RegexLimits,
    phase: Phase,
}
impl Search<'_> {
    fn closure(
        &mut self,
        seed: Thread,
        position: usize,
        frontier: &mut Vec<Thread>,
        steps: &mut usize,
    ) -> Result<(), RegexError> {
        self.pending.push(seed);
        while let Some(mut thread) = self.pending.pop() {
            step(steps, self.limits, self.phase, position)?;
            thread.advanced_depth = thread.advanced_depth.min(self.program.depths[thread.pc]);
            let key = self.program.bases[thread.pc] + thread.advanced_depth;
            if self.seen[key] == self.generation {
                continue;
            }
            self.seen[key] = self.generation;
            match self.program.code[thread.pc] {
                Inst::Accept | Inst::Consume(_, _) => frontier.push(thread),
                Inst::LoopStart(depth, next) => {
                    thread.advanced_depth = thread.advanced_depth.min(depth - 1);
                    thread.pc = next;
                    self.pending.push(thread);
                }
                Inst::LoopEnd(depth, next, exit) => {
                    thread.pc = if thread.advanced_depth >= depth {
                        next
                    } else {
                        exit
                    };
                    self.pending.push(thread);
                }
                Inst::Save(slot, next) => {
                    thread.captures[slot] = Some(position);
                    thread.pc = next;
                    self.pending.push(thread);
                }
                Inst::Split(first, second) => {
                    let mut alternate = thread.duplicate(self.phase, position)?;
                    alternate.pc = second;
                    self.pending.push(alternate);
                    thread.pc = first;
                    self.pending.push(thread);
                }
                Inst::Assert(look, next) => {
                    if self.program.assertion(look, self.input, position) {
                        thread.pc = next;
                        self.pending.push(thread);
                    }
                }
            }
        }
        Ok(())
    }
}

impl Program {
    pub(super) fn search(
        &self,
        input: &str,
        mode: SearchMode,
        names: Arc<Vec<Option<String>>>,
        limits: RegexLimits,
        steps: &mut usize,
        phase: Phase,
    ) -> Result<Option<RegexMatch>, RegexError> {
        let (start, full) = match mode {
            SearchMode::At(start) => (start, false),
            SearchMode::Full => (0, true),
        };
        let bytes_per_thread = size_of::<Thread>()
            .saturating_add(self.slots.saturating_mul(size_of::<Option<usize>>()));
        let logical = self
            .semantic_states
            .saturating_mul(
                bytes_per_thread
                    .saturating_mul(4)
                    .saturating_add(size_of::<usize>()),
            )
            .saturating_add(self.logical_storage());
        if logical > limits.vm_heap {
            return Err(RegexError::limited(
                Kind::OutOfMemory,
                phase,
                start,
                "vm_heap",
            ));
        }
        let mut seen = Vec::new();
        reserve(&mut seen, self.semantic_states, phase, start)?;
        seen.resize(self.semantic_states, 0);
        let mut pending = Vec::new();
        reserve(
            &mut pending,
            self.semantic_states.saturating_mul(2),
            phase,
            start,
        )?;
        let mut frontier = Vec::new();
        let mut next = Vec::new();
        reserve(&mut frontier, self.semantic_states, phase, start)?;
        reserve(&mut next, self.semantic_states, phase, start)?;
        let mut search = Search {
            program: self,
            input,
            seen,
            generation: 1,
            pending,
            limits,
            phase,
        };
        let mut best = None;
        for position in input[start..]
            .char_indices()
            .map(|(offset, _)| start + offset)
            .chain([input.len()])
        {
            if best.is_none() && (!full || position == 0) {
                let mut captures = Vec::new();
                reserve(&mut captures, self.slots, phase, position)?;
                captures.resize(self.slots, None);
                captures[0] = Some(position);
                search.closure(
                    Thread {
                        pc: self.start,
                        advanced_depth: 0,
                        captures,
                    },
                    position,
                    &mut frontier,
                    steps,
                )?;
            }
            if let Some(index) = frontier
                .iter()
                .position(|thread| matches!(self.code[thread.pc], Inst::Accept))
                && (!full || position == input.len())
            {
                let mut captured = frontier[index].duplicate(phase, position)?.captures;
                captured[1] = Some(position);
                best = Some(captured);
                frontier.truncate(index);
                if frontier.is_empty() {
                    break;
                }
            }
            let Some(value) = input[position..].chars().next() else {
                break;
            };
            let next_position = position + value.len_utf8();
            next.clear();
            search.generation = search.generation.wrapping_add(1);
            if search.generation == 0 {
                search.seen.fill(0);
                search.generation = 1;
            }
            for thread in frontier.drain(..) {
                step(steps, limits, phase, position)?;
                if let Inst::Consume(matcher, successor) = &self.code[thread.pc]
                    && matcher.matches(value)
                {
                    search.closure(
                        Thread {
                            pc: *successor,
                            advanced_depth: self.depths[thread.pc],
                            captures: thread.captures,
                        },
                        next_position,
                        &mut next,
                        steps,
                    )?;
                }
            }
            std::mem::swap(&mut frontier, &mut next);
            if frontier.is_empty() && best.is_some() {
                break;
            }
        }
        let Some(offsets) = best else {
            return Ok(None);
        };
        let mut captures = Vec::new();
        reserve(&mut captures, names.len(), phase, start)?;
        captures.resize(names.len(), None);
        for (index, pair) in offsets.chunks_exact(2).enumerate() {
            if let (Some(start), Some(end)) = (pair[0], pair[1]) {
                captures[index] = Some(RegexSpan { start, end });
            }
        }
        Ok(Some(RegexMatch {
            span: captures[0].expect("capture zero always participates"),
            captures,
            names,
        }))
    }
}
