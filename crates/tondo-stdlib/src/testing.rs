//! Deterministic, allocation-bounded helpers used by `std.testing`.
//!
//! The runner remains the owner of lifecycle and failure control.  This module
//! contains only pure value helpers so production code cannot acquire a test
//! envelope by importing it.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffLimits {
    pub max_input_bytes: usize,
    pub max_lines: usize,
    pub max_hunks: usize,
    pub max_output_bytes: usize,
}

impl Default for DiffLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1 << 20,
            max_lines: 16_384,
            max_hunks: 4_096,
            max_output_bytes: 1 << 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextDiffHunk {
    Equal(String),
    Delete(String),
    Insert(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    pub equal: bool,
    pub hunks: Vec<TextDiffHunk>,
    pub expected_bytes: usize,
    pub actual_bytes: usize,
    pub truncated: bool,
}

const DIFF_HEADER: &str = "--- expected\n+++ actual\n";
const DIFF_TRUNCATION: &str = "... truncated ...\n";

/// Portable logical units for diff descriptors, line views, frontier positions,
/// pending partitions and hunk payloads. They are independent of Rust layout.
pub const TEXT_DIFF_MEMORY_MODEL: &str = "linear-myers-32-96-16-32-48-32/1";

impl TextDiffHunk {
    pub fn view(&self) -> TextDiffHunkView<'_> {
        match self {
            Self::Equal(text) => TextDiffHunkView::Equal(text),
            Self::Delete(text) => TextDiffHunkView::Delete(text),
            Self::Insert(text) => TextDiffHunkView::Insert(text),
        }
    }

    fn text(&self) -> &str {
        match self {
            Self::Equal(text) | Self::Delete(text) | Self::Insert(text) => text,
        }
    }

    fn rendered_len(&self) -> usize {
        rendered_hunk_len(self.text())
    }
}

impl TextDiff {
    pub fn retained_bytes(&self) -> u64 {
        32 + self
            .hunks
            .iter()
            .map(|hunk| 32 + hunk.text().len() as u64)
            .sum::<u64>()
    }

    /// Exact output size, without constructing a temporary rendered copy.
    pub fn rendered_len(&self) -> usize {
        self.render_plan().rendered_len()
    }

    pub fn render(&self) -> String {
        let plan = self.render_plan();
        let mut output = String::with_capacity(plan.rendered_len());
        plan.write_rendered(&mut output);
        output
    }

    /// Append to storage whose complete size the caller can admit in advance.
    pub fn write_rendered(&self, output: &mut String) {
        self.render_plan().write_rendered(output);
    }

    fn render_plan(
        &self,
    ) -> TextDiffRenderPlan<'_, impl Clone + Iterator<Item = TextDiffHunkView<'_>>> {
        TextDiffRenderPlan::new(
            self.hunks.iter().map(TextDiffHunk::view),
            self.truncated,
            DiffLimits::default(),
        )
    }
}

/// A borrowed hunk lets nominal host values share the kernel's renderer
/// without copying their strings or constructing an intermediate owned diff.
#[derive(Debug, Clone, Copy)]
pub enum TextDiffHunkView<'a> {
    Equal(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

impl<'a> TextDiffHunkView<'a> {
    fn parts(self) -> (char, &'a str) {
        match self {
            Self::Equal(text) => (' ', text),
            Self::Delete(text) => ('-', text),
            Self::Insert(text) => ('+', text),
        }
    }
}

fn rendered_hunk_len(text: &str) -> usize {
    text.len()
        .saturating_add(text.split_inclusive('\n').count())
        .saturating_add(usize::from(!text.is_empty() && !text.ends_with('\n')))
}

/// Allocation-free rendering admission for both computed and caller-created
/// diffs. The immutable iterator keeps the planned input alive until writing.
pub struct TextDiffRenderPlan<'a, I: Clone + Iterator<Item = TextDiffHunkView<'a>>> {
    hunks: I,
    count: usize,
    length: usize,
    truncated: bool,
}

impl<'a, I: Clone + Iterator<Item = TextDiffHunkView<'a>>> TextDiffRenderPlan<'a, I> {
    pub fn new(hunks: I, source_truncated: bool, limits: DiffLimits) -> Self {
        let maximum = diff_output_limit(limits);
        let content_limit = maximum
            - if source_truncated {
                DIFF_TRUNCATION.len()
            } else {
                0
            };
        let mut length = DIFF_HEADER.len();
        let mut count = 0;
        let mut truncated = source_truncated;
        for hunk in hunks.clone() {
            let text = hunk.parts().1;
            let remaining = content_limit - length;
            // An oversized string is rejected by length before scanning its
            // lines. Neither that hunk nor the remaining iterator is copied.
            if count == limits.max_hunks || text.len() > remaining {
                truncated = true;
                break;
            }
            let needed = rendered_hunk_len(text);
            if needed > remaining {
                truncated = true;
                break;
            }
            length += needed;
            count += 1;
        }
        if truncated && !source_truncated {
            // Keep complete hunks while making room for the marker. A second
            // bounded pass avoids storing a vector of per-hunk lengths and
            // does not truncate an exact-fit result followed by empty hunks.
            let content_limit = maximum - DIFF_TRUNCATION.len();
            let mut kept = 0;
            length = DIFF_HEADER.len();
            for hunk in hunks.clone().take(count) {
                let needed = rendered_hunk_len(hunk.parts().1);
                if needed > content_limit - length {
                    break;
                }
                length += needed;
                kept += 1;
            }
            count = kept;
        }
        if truncated {
            length += DIFF_TRUNCATION.len();
        }
        Self {
            hunks,
            count,
            length,
            truncated,
        }
    }

    pub const fn rendered_len(&self) -> usize {
        self.length
    }

    pub fn write_rendered(&self, output: &mut String) {
        output.push_str(DIFF_HEADER);
        for hunk in self.hunks.clone().take(self.count) {
            let (prefix, text) = hunk.parts();
            for line in text.split_inclusive('\n') {
                output.push(prefix);
                output.push_str(line);
            }
            if !text.ends_with('\n') && !text.is_empty() {
                output.push('\n');
            }
        }
        if self.truncated {
            output.push_str(DIFF_TRUNCATION);
        }
    }
}

pub fn diff_text(expected: &str, actual: &str) -> TextDiff {
    diff_text_with_limits(expected, actual, DiffLimits::default())
}

pub fn diff_text_with_limits(expected: &str, actual: &str, limits: DiffLimits) -> TextDiff {
    TextDiffPlan::new(expected, actual, limits).compute()
}

/// Allocation-free admission plan. The host reserves `memory_bytes` before
/// computing, then retains only `TextDiff::retained_bytes` until reclamation.
pub struct TextDiffPlan<'a> {
    expected: &'a str,
    actual: &'a str,
    expected_lines: usize,
    actual_lines: usize,
    expected_bytes: usize,
    actual_bytes: usize,
    equal: bool,
    truncated: bool,
    limits: DiffLimits,
}

impl<'a> TextDiffPlan<'a> {
    pub fn new(expected: &'a str, actual: &'a str, limits: DiffLimits) -> Self {
        let equal = expected == actual;
        let expected_bytes = expected.len();
        let actual_bytes = actual.len();
        let (expected, expected_lines) = diff_prefix(expected, limits);
        let (actual, actual_lines) = diff_prefix(actual, limits);
        Self {
            expected,
            actual,
            expected_lines,
            actual_lines,
            expected_bytes,
            actual_bytes,
            equal,
            truncated: expected.len() != expected_bytes || actual.len() != actual_bytes,
            limits,
        }
    }

    pub fn memory_bytes(&self) -> u64 {
        if self.equal {
            return 32;
        }
        let lines = (self.expected_lines + self.actual_lines) as u64;
        let frontier = 2 * lines.div_ceil(2) + 3;
        let hunks = lines.min(self.limits.max_hunks as u64);
        let payload = (self.expected.len() as u64 + self.actual.len() as u64)
            .min(diff_output_limit(self.limits) as u64);
        // Two line-view vectors, two reusable Myers frontiers, and a bounded
        // explicit partition stack. No edit-history matrix is retained.
        32 + 96 + 16 * lines + 64 * frontier + 48 * (2 * lines + 1) + 32 * hunks + payload
    }

    pub fn compute(self) -> TextDiff {
        let mut result = TextDiff {
            equal: self.equal,
            hunks: Vec::new(),
            expected_bytes: self.expected_bytes,
            actual_bytes: self.actual_bytes,
            truncated: !self.equal && self.truncated,
        };
        if self.equal {
            return result;
        }
        let expected = self.expected.split_inclusive('\n').collect::<Vec<_>>();
        let actual = self.actual.split_inclusive('\n').collect::<Vec<_>>();
        let total = expected.len() + actual.len();
        let frontier_len = 2 * total.div_ceil(2) + 3;
        let mut forward = vec![DiffFrontier::UNREACHED; frontier_len];
        let mut backward = vec![DiffFrontier::UNREACHED; frontier_len];
        let mut pending = vec![DiffTask::Compare(0..expected.len(), 0..actual.len())];
        let mut output = DiffOutput {
            expected: &expected,
            actual: &actual,
            result: &mut result,
            limits: self.limits,
            rendered: DIFF_HEADER.len(),
            equal: None,
            deleted: None,
            inserted: None,
            stopped: false,
        };
        while let Some(task) = pending.pop() {
            if output.stopped {
                break;
            }
            let (mut left, mut right) = match task {
                DiffTask::Equal(lines) => {
                    output.push(DiffKind::Equal, lines);
                    continue;
                }
                DiffTask::Compare(left, right) => (left, right),
            };
            let prefix = expected[left.clone()]
                .iter()
                .zip(&actual[right.clone()])
                .take_while(|(left, right)| left == right)
                .count();
            output.push(DiffKind::Equal, left.start..left.start + prefix);
            if output.stopped {
                break;
            }
            left.start += prefix;
            right.start += prefix;
            if left.is_empty() {
                output.push(DiffKind::Insert, right);
            } else if right.is_empty() {
                output.push(DiffKind::Delete, left);
            } else {
                let middle = middle_snake(
                    &expected[left.clone()],
                    &actual[right.clone()],
                    &mut forward,
                    &mut backward,
                );
                if middle.distance == left.len() + right.len() {
                    output.push(DiffKind::Delete, left);
                    output.push(DiffKind::Insert, right);
                    continue;
                }
                let (x, y) = (left.start + middle.start.0, right.start + middle.start.1);
                let (u, v) = (left.start + middle.end.0, right.start + middle.end.1);
                // LIFO order emits the earlier expected/actual offsets first.
                pending.push(DiffTask::Compare(u..left.end, v..right.end));
                pending.push(DiffTask::Equal(x..u));
                pending.push(DiffTask::Compare(left.start..x, right.start..y));
            }
        }
        output.finish();
        result
    }
}

fn diff_prefix(text: &str, limits: DiffLimits) -> (&str, usize) {
    let prefix = &text[..text.floor_char_boundary(text.len().min(limits.max_input_bytes))];
    let (bytes, lines) = prefix
        .split_inclusive('\n')
        .take(limits.max_lines)
        .fold((0, 0), |(bytes, lines), line| {
            (bytes + line.len(), lines + 1)
        });
    (&prefix[..bytes], lines)
}

fn diff_output_limit(limits: DiffLimits) -> usize {
    // Even an empty truncated result must preserve the diagnostic envelope.
    limits
        .max_output_bytes
        .max(DIFF_HEADER.len() + DIFF_TRUNCATION.len())
}

enum DiffTask {
    Compare(std::ops::Range<usize>, std::ops::Range<usize>),
    Equal(std::ops::Range<usize>),
}

struct MiddleSnake {
    start: (usize, usize),
    end: (usize, usize),
    distance: usize,
    first_match: Option<(usize, usize)>,
}

impl MiddleSnake {
    fn consider(self, selected: &mut Option<Self>) {
        if selected.as_ref().is_none_or(|previous| {
            (
                self.first_match.unwrap_or((usize::MAX, usize::MAX)),
                self.start,
                self.end,
            ) < (
                previous.first_match.unwrap_or((usize::MAX, usize::MAX)),
                previous.start,
                previous.end,
            )
        }) {
            *selected = Some(self);
        }
    }
}

#[derive(Clone, Copy)]
struct DiffFrontier {
    end: isize,
    first_match: Option<(usize, usize)>,
}

impl DiffFrontier {
    const UNREACHED: Self = Self {
        end: -1,
        first_match: None,
    };
}

// Myers' bidirectional middle-snake search, with one reusable vector per
// direction. At the first overlapping distance, choose the lowest expected
// offset and then actual offset; line order and byte-offset order coincide.
fn middle_snake(
    expected: &[&str],
    actual: &[&str],
    forward: &mut [DiffFrontier],
    backward: &mut [DiffFrontier],
) -> MiddleSnake {
    let n = expected.len() as isize;
    let m = actual.len() as isize;
    let maximum = (expected.len() + actual.len()).div_ceil(2) as isize;
    let offset = maximum + 1;
    let width = (2 * maximum + 3) as usize;
    let forward = &mut forward[..width];
    let backward = &mut backward[..width];
    forward.fill(DiffFrontier::UNREACHED);
    backward.fill(DiffFrontier::UNREACHED);
    let delta = n - m;
    for distance in 0..=maximum {
        let mut selected = None;
        for diagonal in (-distance..=distance).step_by(2) {
            let index = (offset + diagonal) as usize;
            let Some(mut path) = frontier_start(forward, index, diagonal, distance, n, m) else {
                forward[index] = DiffFrontier::UNREACHED;
                continue;
            };
            let mut x = path.end;
            let mut y = x - diagonal;
            let start = (x as usize, y as usize);
            while x < n && y < m && expected[x as usize] == actual[y as usize] {
                x += 1;
                y += 1;
            }
            if x as usize != start.0 && path.first_match.is_none() {
                path.first_match = Some(start);
            }
            path.end = x;
            forward[index] = path;
            let reverse_diagonal = delta - diagonal;
            if delta % 2 != 0 && reverse_diagonal.abs() < distance {
                let reverse = backward[(offset + reverse_diagonal) as usize];
                if reverse.end >= 0 && x + reverse.end >= n {
                    MiddleSnake {
                        start,
                        end: (x as usize, y as usize),
                        distance: (2 * distance - 1) as usize,
                        first_match: path.first_match.or(reverse.first_match),
                    }
                    .consider(&mut selected);
                }
            }
        }
        if let Some(middle) = selected {
            return middle;
        }
        for diagonal in (-distance..=distance).step_by(2) {
            let index = (offset + diagonal) as usize;
            let Some(mut path) = frontier_start(backward, index, diagonal, distance, n, m) else {
                backward[index] = DiffFrontier::UNREACHED;
                continue;
            };
            let mut x = path.end;
            let mut y = x - diagonal;
            let end = ((n - x) as usize, (m - y) as usize);
            while x < n && y < m && expected[(n - x - 1) as usize] == actual[(m - y - 1) as usize] {
                x += 1;
                y += 1;
            }
            if (n - x) as usize != end.0 {
                path.first_match = Some(((n - x) as usize, (m - y) as usize));
            }
            path.end = x;
            backward[index] = path;
            let forward_diagonal = delta - diagonal;
            if delta % 2 == 0 && forward_diagonal.abs() <= distance {
                let opposite = forward[(offset + forward_diagonal) as usize];
                if opposite.end >= 0 && opposite.end + x >= n {
                    MiddleSnake {
                        start: ((n - x) as usize, (m - y) as usize),
                        end,
                        distance: (2 * distance) as usize,
                        first_match: opposite.first_match.or(path.first_match),
                    }
                    .consider(&mut selected);
                }
            }
        }
        if let Some(middle) = selected {
            return middle;
        }
    }
    unreachable!("deleting and inserting every line is an edit path")
}

fn frontier_start(
    frontier: &[DiffFrontier],
    index: usize,
    diagonal: isize,
    distance: isize,
    n: isize,
    m: isize,
) -> Option<DiffFrontier> {
    if distance == 0 {
        return Some(DiffFrontier {
            end: 0,
            first_match: None,
        });
    }
    let mut deleted = frontier[index - 1];
    if deleted.end >= 0 {
        deleted.end += 1;
    }
    [frontier[index + 1], deleted]
        .into_iter()
        .filter(|path| {
            path.end >= 0 && path.end <= n && path.end - diagonal >= 0 && path.end - diagonal <= m
        })
        .max_by_key(|path| {
            (
                path.end,
                std::cmp::Reverse(path.first_match.unwrap_or((usize::MAX, usize::MAX))),
            )
        })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    Equal,
    Delete,
    Insert,
}

struct DiffOutput<'a, 'b> {
    expected: &'a [&'b str],
    actual: &'a [&'b str],
    result: &'a mut TextDiff,
    limits: DiffLimits,
    rendered: usize,
    equal: Option<std::ops::Range<usize>>,
    deleted: Option<std::ops::Range<usize>>,
    inserted: Option<std::ops::Range<usize>>,
    stopped: bool,
}

impl DiffOutput<'_, '_> {
    fn push(&mut self, kind: DiffKind, lines: std::ops::Range<usize>) {
        if self.stopped || lines.is_empty() {
            return;
        }
        if kind == DiffKind::Equal {
            self.flush_edits();
        } else if let Some(equal) = self.equal.take() {
            self.emit(DiffKind::Equal, equal);
        }
        if self.stopped {
            return;
        }
        let pending = match kind {
            DiffKind::Equal => &mut self.equal,
            DiffKind::Delete => &mut self.deleted,
            DiffKind::Insert => &mut self.inserted,
        };
        if let Some(range) = pending {
            debug_assert_eq!(range.end, lines.start);
            range.end = lines.end;
        } else {
            *pending = Some(lines);
        }
    }

    fn flush_edits(&mut self) {
        // Edits between two matches commute. Emit their complete deletions
        // before insertions so a replacement has one canonical hunk order,
        // regardless of which direction found its middle crossing.
        if let Some(deleted) = self.deleted.take() {
            self.emit(DiffKind::Delete, deleted);
        }
        if let Some(inserted) = self.inserted.take() {
            self.emit(DiffKind::Insert, inserted);
        }
    }

    fn emit(&mut self, kind: DiffKind, range: std::ops::Range<usize>) {
        if self.stopped {
            return;
        }
        let lines = if kind == DiffKind::Insert {
            self.actual
        } else {
            self.expected
        };
        let lines = &lines[range];
        let bytes = lines.iter().map(|line| line.len()).sum::<usize>();
        let rendered = bytes + lines.len() + usize::from(!lines.last().unwrap().ends_with('\n'));
        let marker = if self.result.truncated {
            DIFF_TRUNCATION.len()
        } else {
            0
        };
        if self.result.hunks.len() == self.limits.max_hunks
            || self.rendered + rendered + marker > diff_output_limit(self.limits)
        {
            self.result.truncated = true;
            self.stopped = true;
            return;
        }
        let mut text = String::with_capacity(bytes);
        for line in lines {
            text.push_str(line);
        }
        self.result.hunks.push(match kind {
            DiffKind::Equal => TextDiffHunk::Equal(text),
            DiffKind::Delete => TextDiffHunk::Delete(text),
            DiffKind::Insert => TextDiffHunk::Insert(text),
        });
        self.rendered += rendered;
    }

    fn finish(mut self) {
        if let Some(equal) = self.equal.take() {
            self.emit(DiffKind::Equal, equal);
        }
        self.flush_edits();
        if self.result.truncated {
            while self.rendered + DIFF_TRUNCATION.len() > diff_output_limit(self.limits) {
                let hunk = self.result.hunks.pop().expect("the envelope always fits");
                self.rendered -= hunk.rendered_len();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatTolerance {
    pub absolute: f64,
    pub relative: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatToleranceError {
    Negative,
    NonFinite,
    Overflow,
}

impl FloatTolerance {
    pub fn new(absolute: f64, relative: f64) -> Result<Self, FloatToleranceError> {
        if !absolute.is_finite() || !relative.is_finite() {
            return Err(FloatToleranceError::NonFinite);
        }
        if absolute < 0.0 || relative < 0.0 {
            return Err(FloatToleranceError::Negative);
        }
        if absolute + relative == f64::INFINITY {
            return Err(FloatToleranceError::Overflow);
        }
        Ok(Self { absolute, relative })
    }

    pub fn is_near(self, expected: f64, actual: f64) -> bool {
        if expected.is_nan() || actual.is_nan() {
            return false;
        }
        if expected == actual {
            return true;
        }
        if !expected.is_finite() || !actual.is_finite() {
            return false;
        }
        let delta = (actual - expected).abs();
        if delta <= self.absolute {
            return true;
        }
        let scale = expected.abs().max(actual.abs());
        scale > 0.0 && delta / scale <= self.relative
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationId {
    pub seed: u64,
    pub case_index: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationError {
    InvalidBounds,
    LimitExceeded,
    Exhausted,
}

/// Defensive per-generator draw budget used by every hosted and native route.
pub const MAX_GENERATOR_DRAWS: u64 = 1_048_576;
/// Maximum materialized payload accepted by one generated bytes/text value.
pub const MAX_GENERATED_BYTES: usize = 1_048_576;
/// Maximum number of candidates one public shrink operation may materialize.
pub const MAX_SHRINK_CANDIDATES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generator {
    seed: u64,
    case_index: u64,
    state: u64,
    draws: u64,
}

impl Generator {
    pub fn new(seed: u64) -> Self {
        Self::for_case(seed, 0)
    }

    pub fn for_case(seed: u64, case_index: u64) -> Self {
        let mut state = (seed ^ 0x9e37_79b9_7f4a_7c15)
            .wrapping_add(case_index.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        if state == 0 {
            state = 0x6a09_e667_f3bc_c909;
        }
        Self {
            seed,
            case_index,
            state,
            draws: 0,
        }
    }

    pub const fn id(self) -> GenerationId {
        GenerationId {
            seed: self.seed,
            case_index: self.case_index,
        }
    }

    pub const fn draw_count(self) -> u64 {
        self.draws
    }

    pub fn next_u64(&mut self) -> Result<u64, GenerationError> {
        if self.draws >= MAX_GENERATOR_DRAWS {
            return Err(GenerationError::Exhausted);
        }
        self.draws = self
            .draws
            .checked_add(1)
            .ok_or(GenerationError::Exhausted)?;
        self.state ^= self.state << 7;
        self.state ^= self.state >> 9;
        self.state ^= self.state << 8;
        Ok(self.state)
    }

    pub fn next_bool(&mut self) -> Result<bool, GenerationError> {
        Ok(self.next_u64()? & 1 == 1)
    }

    pub fn next_int(&mut self, minimum: i128, maximum: i128) -> Result<i128, GenerationError> {
        let mut next = *self;
        let value = next.sample_int(minimum, maximum)?;
        *self = next;
        Ok(value)
    }

    fn sample_int(&mut self, minimum: i128, maximum: i128) -> Result<i128, GenerationError> {
        if minimum > maximum {
            return Err(GenerationError::InvalidBounds);
        }
        let span = (maximum as u128)
            .wrapping_sub(minimum as u128)
            .wrapping_add(1);
        if span == 0 {
            let high = u128::from(self.next_u64()?);
            let low = u128::from(self.next_u64()?);
            return Ok(i128::from_ne_bytes(((high << 64) | low).to_ne_bytes()));
        }
        let threshold = span.wrapping_neg() % span;
        loop {
            let high = u128::from(self.next_u64()?);
            let low = u128::from(self.next_u64()?);
            let sample = (high << 64) | low;
            if sample >= threshold {
                return Ok(minimum.wrapping_add((sample % span) as i128));
            }
        }
    }

    pub fn next_bytes(&mut self, maximum_length: usize) -> Result<Vec<u8>, GenerationError> {
        let mut next = *self;
        let length = next.next_buffer_length(maximum_length)?;
        if length as u64 > MAX_GENERATOR_DRAWS - next.draws {
            return Err(GenerationError::Exhausted);
        }
        let mut output = Vec::with_capacity(length);
        for _ in 0..length {
            output.push(next.next_u64()? as u8);
        }
        *self = next;
        Ok(output)
    }

    pub fn next_text(&mut self, maximum_bytes: usize) -> Result<String, GenerationError> {
        let mut next = *self;
        let length = next.next_buffer_length(maximum_bytes)?;
        let mut output = String::with_capacity(length);
        while output.len() < length {
            let mut scalar = (next.next_u64()? % 0x11_0000) as u32;
            if (0xd800..=0xdfff).contains(&scalar) {
                scalar += 0x800;
            }
            let character = char::from_u32(scalar).ok_or(GenerationError::LimitExceeded)?;
            if output.len() + character.len_utf8() > length {
                break;
            }
            output.push(character);
        }
        *self = next;
        Ok(output)
    }

    /// Exact capacity selected by the next bytes/text call, without consuming
    /// a draw or creating a buffer. The host must admit it before that call and
    /// must not advance this generator between planning and generation.
    pub fn planned_buffer_capacity(self, maximum: usize) -> Result<usize, GenerationError> {
        let mut next = self;
        next.next_buffer_length(maximum)
    }

    fn next_buffer_length(&mut self, maximum: usize) -> Result<usize, GenerationError> {
        if maximum > MAX_GENERATED_BYTES {
            return Err(GenerationError::LimitExceeded);
        }
        usize::try_from(self.next_int(0, maximum as i128)?)
            .map_err(|_| GenerationError::LimitExceeded)
    }
}

pub trait Shrink: Clone + PartialEq {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError>;
}

impl Shrink for i128 {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        let mut value = *self;
        let mut result = Vec::new();
        while value != 0 && result.len() < limit {
            value /= 2;
            if !result.contains(&value) {
                result.push(value);
            }
        }
        Ok(result)
    }
}

impl Shrink for String {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for length in 0..=self.len() {
            if result.len() == limit {
                break;
            }
            let candidate = self[..self.floor_char_boundary(length)].to_owned();
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        Ok(result)
    }
}

impl Shrink for f64 {
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 || self.is_nan() {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for candidate in [0.0, *self / 2.0, -*self / 2.0] {
            if result.len() == limit {
                break;
            }
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        Ok(result)
    }
}

impl<T> Shrink for Vec<T>
where
    T: Shrink + Clone + PartialEq,
{
    fn candidates(&self, limit: usize) -> Result<Vec<Self>, GenerationError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for length in 0..=self.len() {
            if result.len() == limit {
                break;
            }
            let candidate = self[..length].to_vec();
            if !result.contains(&candidate) {
                result.push(candidate);
            }
        }
        for index in 0..self.len() {
            for value in self[index].candidates(limit.saturating_sub(result.len()))? {
                if result.len() == limit {
                    break;
                }
                let mut candidate = self.clone();
                candidate[index] = value;
                if !result.contains(&candidate) {
                    result.push(candidate);
                }
            }
        }
        Ok(result)
    }
}

pub fn shrink<T: Shrink>(value: &T, limit: usize) -> Result<Vec<T>, GenerationError> {
    if limit > MAX_SHRINK_CANDIDATES {
        return Err(GenerationError::LimitExceeded);
    }
    let candidates = value.candidates(limit)?;
    if candidates.len() > limit {
        return Err(GenerationError::LimitExceeded);
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_render_plans_preserve_exact_hunk_prefixes_at_output_and_count_limits() {
        let hunks = [
            TextDiffHunkView::Equal("same\n"),
            TextDiffHunkView::Delete("é\n"),
            TextDiffHunkView::Insert("🦀"),
        ];
        let pieces = [" same\n", "-é\n", "+🦀\n"];
        for max_hunks in 0..=4 {
            for max_output_bytes in [0, 43, 48, 53, 100] {
                for source_truncated in [false, true] {
                    let limits = DiffLimits {
                        max_hunks,
                        max_output_bytes,
                        ..DiffLimits::default()
                    };
                    let maximum = max_output_bytes.max(DIFF_HEADER.len() + DIFF_TRUNCATION.len());
                    let full = format!(
                        "{DIFF_HEADER}{}{}",
                        pieces.concat(),
                        if source_truncated {
                            DIFF_TRUNCATION
                        } else {
                            ""
                        }
                    );
                    let expected = if max_hunks >= pieces.len() && full.len() <= maximum {
                        full
                    } else {
                        (0..=max_hunks.min(pieces.len()))
                            .map(|count| {
                                format!(
                                    "{DIFF_HEADER}{}{DIFF_TRUNCATION}",
                                    pieces[..count].concat()
                                )
                            })
                            .rfind(|candidate| candidate.len() <= maximum)
                            .unwrap()
                    };
                    let plan = TextDiffRenderPlan::new(hunks.into_iter(), source_truncated, limits);
                    let mut actual = String::with_capacity(plan.rendered_len());
                    plan.write_rendered(&mut actual);
                    assert_eq!(
                        actual, expected,
                        "hunks={max_hunks}, bytes={max_output_bytes}, truncated={source_truncated}"
                    );
                    assert_eq!(actual.len(), plan.rendered_len());
                }
            }
        }
        let text = "one two three four five\n";
        let expected = format!("{DIFF_HEADER} {text}");
        let plan = TextDiffRenderPlan::new(
            [TextDiffHunkView::Equal(text), TextDiffHunkView::Insert("")].into_iter(),
            false,
            DiffLimits {
                max_output_bytes: expected.len(),
                ..DiffLimits::default()
            },
        );
        let mut actual = String::new();
        plan.write_rendered(&mut actual);
        assert_eq!(actual, expected);
    }

    #[test]
    fn manually_constructed_diffs_cannot_bypass_rendering_bounds() {
        for hunks in [
            vec![TextDiffHunk::Insert("é".repeat(1_048_576))],
            vec![TextDiffHunk::Equal(String::new()); DiffLimits::default().max_hunks + 1],
        ] {
            let diff = TextDiff {
                equal: false,
                hunks,
                expected_bytes: 0,
                actual_bytes: 0,
                truncated: false,
            };
            assert_eq!(diff.render(), format!("{DIFF_HEADER}{DIFF_TRUNCATION}"));
            assert_eq!(
                diff.rendered_len(),
                DIFF_HEADER.len() + DIFF_TRUNCATION.len()
            );
            assert!(
                !diff.truncated,
                "rendering must not mutate the public value"
            );
        }
    }

    #[test]
    fn diff_is_stable_and_merges_adjacent_hunks() {
        let diff = diff_text("same\nold\n", "same\nnew\n");
        assert!(!diff.equal);
        assert_eq!(
            diff.hunks,
            vec![
                TextDiffHunk::Equal("same\n".into()),
                TextDiffHunk::Delete("old\n".into()),
                TextDiffHunk::Insert("new\n".into()),
            ]
        );
        assert_eq!(
            diff.render(),
            "--- expected\n+++ actual\n same\n-old\n+new\n"
        );
        assert_eq!(diff_text("x", "x").hunks, Vec::new());
    }

    #[test]
    fn diff_limits_never_hide_that_output_was_truncated() {
        let limits = DiffLimits {
            max_input_bytes: 3,
            max_lines: 1,
            max_hunks: 1,
            max_output_bytes: 8,
        };
        let diff = diff_text_with_limits("abcdef", "uvwxyz", limits);
        assert!(diff.truncated);
        assert!(diff.render().contains("truncated"));
    }

    #[test]
    fn myers_diff_matches_an_independent_bounded_edit_distance() {
        fn texts() -> Vec<String> {
            (0..=6)
                .flat_map(|length| {
                    (0..1_u32 << length).map(move |bits| {
                        (0..length)
                            .map(|bit| if bits & (1 << bit) == 0 { "a\n" } else { "b\n" })
                            .collect()
                    })
                })
                .collect()
        }
        let texts = texts();
        for expected in &texts {
            for actual in &texts {
                let left = expected.split_inclusive('\n').collect::<Vec<_>>();
                let right = actual.split_inclusive('\n').collect::<Vec<_>>();
                // A small dynamic-programming oracle is independent of the
                // production frontiers and is never used on large documents.
                let mut distance = vec![vec![0; right.len() + 1]; left.len() + 1];
                for (row, values) in distance.iter_mut().enumerate() {
                    values[0] = row;
                }
                for (column, value) in distance[0].iter_mut().enumerate() {
                    *value = column;
                }
                for row in 1..=left.len() {
                    for column in 1..=right.len() {
                        distance[row][column] = if left[row - 1] == right[column - 1] {
                            distance[row - 1][column - 1]
                        } else {
                            1 + distance[row - 1][column].min(distance[row][column - 1])
                        };
                    }
                }
                let diff = diff_text(expected, actual);
                assert!(!diff.truncated, "{expected:?} / {actual:?}");
                assert_eq!(diff.rendered_len(), diff.render().len());
                let mut restored_expected = String::new();
                let mut restored_actual = String::new();
                let mut edits = 0;
                for hunk in &diff.hunks {
                    match hunk {
                        TextDiffHunk::Equal(text) => {
                            restored_expected.push_str(text);
                            restored_actual.push_str(text);
                        }
                        TextDiffHunk::Delete(text) => {
                            restored_expected.push_str(text);
                            edits += text.split_inclusive('\n').count();
                        }
                        TextDiffHunk::Insert(text) => {
                            restored_actual.push_str(text);
                            edits += text.split_inclusive('\n').count();
                        }
                    }
                }
                if !diff.equal {
                    assert_eq!(&restored_expected, expected);
                    assert_eq!(&restored_actual, actual);
                }
                assert_eq!(
                    edits,
                    distance[left.len()][right.len()],
                    "{expected:?} / {actual:?}"
                );
                assert_eq!(diff, diff_text(expected, actual));
            }
        }
    }

    #[test]
    fn myers_diff_resolves_repeated_line_ties_and_preserves_exact_bytes() {
        assert_eq!(
            diff_text("a\nb\n", "b\na\n").hunks,
            vec![
                TextDiffHunk::Insert("b\n".into()),
                TextDiffHunk::Equal("a\n".into()),
                TextDiffHunk::Delete("b\n".into()),
            ]
        );
        assert_eq!(
            diff_text("x\na\n", "a\na\n").hunks,
            vec![
                TextDiffHunk::Delete("x\n".into()),
                TextDiffHunk::Equal("a\n".into()),
                TextDiffHunk::Insert("a\n".into()),
            ]
        );
        assert_eq!(
            diff_text("é\r\nΩ", "é\nΩ").render(),
            "--- expected\n+++ actual\n-é\r\n+é\n Ω\n"
        );
        assert_eq!(
            diff_text("", "Ω").hunks,
            vec![TextDiffHunk::Insert("Ω".into())]
        );
        assert_eq!(
            diff_text("Ω", "").hunks,
            vec![TextDiffHunk::Delete("Ω".into())]
        );
    }

    #[test]
    fn myers_diff_reserves_linear_workspace_and_keeps_whole_bounded_hunks() {
        let expected = "same\n".repeat(16_383) + "old\n";
        let actual = "same\n".repeat(16_383) + "new\n";
        let plan = TextDiffPlan::new(&expected, &actual, DiffLimits::default());
        let reservation = plan.memory_bytes();
        assert!(reservation < 8 * 1024 * 1024);
        let result = plan.compute();
        assert!(!result.truncated);
        assert_eq!(result.hunks.len(), 3);
        assert!(result.retained_bytes() <= reservation);
        for lines in [0, 1, 2, 16] {
            for hunks in [0, 1, 2, 8] {
                for output in [0, 8, 40, 48, 128] {
                    let limits = DiffLimits {
                        max_lines: lines,
                        max_hunks: hunks,
                        max_output_bytes: output,
                        max_input_bytes: 7,
                    };
                    let plan = TextDiffPlan::new("é\r\nx\ny\n", "é\r\nw\nz\n", limits);
                    let reservation = plan.memory_bytes();
                    let result = plan.compute();
                    assert!(result.retained_bytes() <= reservation);
                    assert!(result.hunks.len() <= hunks);
                    assert!(result.rendered_len() <= diff_output_limit(limits));
                    assert_eq!(result.rendered_len(), result.render().len());
                    assert!(result.truncated);
                }
            }
        }
        let limits = DiffLimits {
            max_output_bytes: 50,
            ..DiffLimits::default()
        };
        let result = diff_text_with_limits("old\nold\nold\nold\nold\nold\n", "new\n", limits);
        assert!(result.truncated);
        assert!(result.hunks.is_empty());
    }

    #[test]
    fn tolerance_follows_nan_infinity_and_relative_rules() {
        let tolerance = FloatTolerance::new(0.01, 0.1).unwrap();
        assert!(tolerance.is_near(10.0, 10.5));
        assert!(!tolerance.is_near(f64::NAN, f64::NAN));
        assert!(tolerance.is_near(f64::INFINITY, f64::INFINITY));
        assert_eq!(
            FloatTolerance::new(-1.0, 0.0),
            Err(FloatToleranceError::Negative)
        );
        assert_eq!(
            FloatTolerance::new(f64::INFINITY, 0.0),
            Err(FloatToleranceError::NonFinite)
        );
    }

    #[test]
    fn generator_replays_and_respects_bounds() {
        let mut left = Generator::for_case(7, 3);
        let mut right = Generator::for_case(7, 3);
        assert_eq!(left.id(), right.id());
        for _ in 0..10 {
            assert_eq!(left.next_u64(), right.next_u64());
        }
        assert!(left.next_int(3, 2).is_err());
        assert!(left.next_int(-2, 2).unwrap().abs() <= 2);
        let mut text_left = Generator::for_case(7, 3);
        let mut text_right = Generator::for_case(7, 3);
        assert_eq!(
            text_left.next_text(4).unwrap(),
            text_right.next_text(4).unwrap()
        );
        for (seed, case_index) in [(0, 0), (u64::MAX, u64::MAX), (7, 3)] {
            let text = Generator::for_case(seed, case_index).next_text(64).unwrap();
            assert!(text.len() <= 64);
            assert!(text.is_char_boundary(text.len()));
            assert!(
                text.chars()
                    .all(|character| !(0xd800..=0xdfff).contains(&(character as u32)))
            );
        }
        assert_eq!(Generator::for_case(7, 3).next_int(4, 4), Ok(4));
        let mut full_range = Generator::for_case(7, 3);
        assert!(full_range.next_int(i128::MIN, i128::MAX).is_ok());
        assert_eq!(full_range.draw_count(), 2);
        assert_eq!(
            Generator::for_case(7, 3).next_bytes(MAX_GENERATED_BYTES + 1),
            Err(GenerationError::LimitExceeded)
        );
        assert_eq!(
            Generator::for_case(7, 3).next_text(MAX_GENERATED_BYTES + 1),
            Err(GenerationError::LimitExceeded)
        );
    }

    #[test]
    fn generator_stops_before_advancing_past_draw_budget() {
        let mut generator = Generator::new(1);
        for _ in 0..MAX_GENERATOR_DRAWS {
            generator.next_u64().unwrap();
        }
        assert_eq!(generator.draw_count(), MAX_GENERATOR_DRAWS);
        assert_eq!(generator.next_u64(), Err(GenerationError::Exhausted));
        assert_eq!(generator.draw_count(), MAX_GENERATOR_DRAWS);
    }

    #[test]
    fn generator_exhaustion_preserves_the_complete_state() {
        for operation in 0..3 {
            let mut generator = Generator::new(7);
            generator.draws = MAX_GENERATOR_DRAWS - 1;
            let before = generator;
            let result = match operation {
                0 => generator.next_int(i128::MIN, i128::MAX).map(|_| ()),
                1 => generator.next_bytes(4096).map(|_| ()),
                _ => generator.next_text(4096).map(|_| ()),
            };
            assert_eq!(result, Err(GenerationError::Exhausted));
            assert_eq!(generator, before);
        }
    }

    #[test]
    fn generator_capacity_planning_preserves_replay_and_bounds() {
        for seed in 0..32 {
            for maximum in [0, 1, 4, 64, 4096] {
                let initial = Generator::for_case(seed, 3);
                let capacity = initial.planned_buffer_capacity(maximum).unwrap();
                assert!(capacity <= maximum);
                let mut bytes = initial;
                assert_eq!(bytes.next_bytes(maximum).unwrap().len(), capacity);
                let mut text = initial;
                assert!(text.next_text(maximum).unwrap().len() <= capacity);
                assert_eq!(initial, Generator::for_case(seed, 3));
            }
        }
        let generator = Generator::new(7);
        assert!(generator.planned_buffer_capacity(1_048_576).unwrap() > 32_768);
        assert_eq!(
            generator.planned_buffer_capacity(MAX_GENERATED_BYTES + 1),
            Err(GenerationError::LimitExceeded)
        );
        assert_eq!(generator.draw_count(), 0);
    }

    #[test]
    fn shrink_candidates_are_bounded_and_deterministic() {
        assert_eq!(shrink(&10_i128, 3).unwrap(), vec![5, 2, 1]);
        assert_eq!(
            shrink(&"tondo".to_owned(), 3).unwrap(),
            vec!["".to_owned(), "t".to_owned(), "to".to_owned()]
        );
        assert_eq!(shrink(&vec![10_i128, 4], 3).unwrap()[0], Vec::<i128>::new());
        assert!(shrink(&10_i128, 0).unwrap().is_empty());
        assert_eq!(
            shrink(&10_i128, MAX_SHRINK_CANDIDATES + 1),
            Err(GenerationError::LimitExceeded)
        );
    }

    #[test]
    fn shrink_rejects_a_custom_implementation_that_ignores_the_limit() {
        #[derive(Debug, Clone, PartialEq)]
        struct Broken;

        impl Shrink for Broken {
            fn candidates(&self, _limit: usize) -> Result<Vec<Self>, GenerationError> {
                Ok(vec![Broken, Broken])
            }
        }

        assert_eq!(shrink(&Broken, 1), Err(GenerationError::LimitExceeded));
    }
}
