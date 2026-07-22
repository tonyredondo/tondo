use super::{HirConstantValueKind, HirExpressionId, HirExpressionKind, HirLiteral, HirProgram};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StaticSlice {
    pub(crate) start: Option<u64>,
    pub(crate) end: Option<u64>,
    pub(crate) step: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StaticCollectionRegion {
    Index(u64),
    Slice(StaticSlice),
    PatternIndex(u32),
    PatternRest { start: u32, suffix: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaticRegionRelation {
    Disjoint,
    Overlap,
    Runtime,
}

pub(crate) fn static_nonnegative_integer(
    program: &HirProgram,
    expression: HirExpressionId,
) -> Option<u64> {
    let expression = program.expression(expression)?;
    match expression.kind() {
        HirExpressionKind::Literal(HirLiteral::Integer(spelling)) => {
            parse_nonnegative_integer(spelling)
        }
        HirExpressionKind::Constant(symbol) => {
            let HirConstantValueKind::Integer(value) =
                program.constant(*symbol)?.evaluated()?.kind()
            else {
                return None;
            };
            u64::try_from(*value).ok()
        }
        HirExpressionKind::Coerce { value, .. } => static_nonnegative_integer(program, *value),
        _ => None,
    }
}

pub(crate) fn static_slice(
    program: &HirProgram,
    start: Option<HirExpressionId>,
    end: Option<HirExpressionId>,
    step: Option<HirExpressionId>,
) -> Option<StaticSlice> {
    Some(StaticSlice {
        start: optional_static_integer(program, start)?,
        end: optional_static_integer(program, end)?,
        step: optional_static_integer(program, step)?,
    })
}

fn optional_static_integer(
    program: &HirProgram,
    expression: Option<HirExpressionId>,
) -> Option<Option<u64>> {
    match expression {
        Some(expression) => Some(Some(static_nonnegative_integer(program, expression)?)),
        None => Some(None),
    }
}

pub(crate) fn static_collection_relation(
    left: StaticCollectionRegion,
    right: StaticCollectionRegion,
) -> StaticRegionRelation {
    use StaticCollectionRegion::{Index, PatternIndex, PatternRest, Slice};

    match (left, right) {
        (Index(left), Index(right)) => index_relation(left, right),
        (PatternIndex(left), PatternIndex(right)) => {
            index_relation(u64::from(left), u64::from(right))
        }
        (Index(left), PatternIndex(right)) | (PatternIndex(right), Index(left)) => {
            index_relation(left, u64::from(right))
        }
        (Index(index), Slice(slice)) | (Slice(slice), Index(index)) => {
            index_slice_relation(index, slice)
        }
        (PatternIndex(index), Slice(slice)) | (Slice(slice), PatternIndex(index)) => {
            index_slice_relation(u64::from(index), slice)
        }
        (Slice(left), Slice(right)) => slice_relation(left, right),
        (PatternIndex(index), PatternRest { start, suffix })
        | (PatternRest { start, suffix }, PatternIndex(index)) => {
            if index < start {
                StaticRegionRelation::Disjoint
            } else if suffix == 0 {
                StaticRegionRelation::Overlap
            } else {
                StaticRegionRelation::Runtime
            }
        }
        (Index(index), PatternRest { start, suffix })
        | (PatternRest { start, suffix }, Index(index)) => {
            if index < u64::from(start) {
                StaticRegionRelation::Disjoint
            } else if suffix == 0 {
                StaticRegionRelation::Overlap
            } else {
                StaticRegionRelation::Runtime
            }
        }
        (PatternRest { .. }, PatternRest { .. })
        | (Slice(_), PatternRest { .. })
        | (PatternRest { .. }, Slice(_)) => StaticRegionRelation::Runtime,
    }
}

fn index_relation(left: u64, right: u64) -> StaticRegionRelation {
    if left == right {
        StaticRegionRelation::Overlap
    } else {
        StaticRegionRelation::Disjoint
    }
}

fn index_slice_relation(index: u64, slice: StaticSlice) -> StaticRegionRelation {
    if slice_contains(slice, index) {
        StaticRegionRelation::Overlap
    } else {
        StaticRegionRelation::Disjoint
    }
}

fn slice_relation(left: StaticSlice, right: StaticSlice) -> StaticRegionRelation {
    let Some(left) = positive_progression(left) else {
        return StaticRegionRelation::Disjoint;
    };
    let Some(right) = positive_progression(right) else {
        return StaticRegionRelation::Disjoint;
    };
    if left.end.is_some_and(|end| end <= right.start)
        || right.end.is_some_and(|end| end <= left.start)
    {
        return StaticRegionRelation::Disjoint;
    }
    let divisor = greatest_common_divisor(left.step, right.step);
    if left.start % divisor != right.start % divisor {
        return StaticRegionRelation::Disjoint;
    }
    StaticRegionRelation::Runtime
}

fn slice_contains(slice: StaticSlice, index: u64) -> bool {
    let Some(slice) = positive_progression(slice) else {
        return false;
    };
    index >= slice.start
        && slice.end.is_none_or(|end| index < end)
        && (index - slice.start).is_multiple_of(slice.step)
}

#[derive(Clone, Copy)]
struct PositiveProgression {
    start: u64,
    end: Option<u64>,
    step: u64,
}

fn positive_progression(slice: StaticSlice) -> Option<PositiveProgression> {
    let start = slice.start.unwrap_or(0);
    let step = slice.step.unwrap_or(1);
    if step == 0 || slice.end.is_some_and(|end| end <= start) {
        return None;
    }
    Some(PositiveProgression {
        start,
        end: slice.end,
        step,
    })
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

pub(crate) fn parse_nonnegative_integer(spelling: &str) -> Option<u64> {
    let suffix_length = ["i16", "i32", "i64", "u16", "u32", "u64"]
        .into_iter()
        .find(|suffix| spelling.ends_with(suffix))
        .map_or_else(
            || {
                ["i8", "u8"]
                    .into_iter()
                    .find(|suffix| spelling.ends_with(suffix))
                    .map_or(0, |suffix| suffix.len())
            },
            str::len,
        );
    let body = &spelling[..spelling.len().checked_sub(suffix_length)?];
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, body)
    };
    u128::from_str_radix(&digits.replace('_', ""), radix)
        .ok()
        .and_then(|value| u64::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(start: Option<u64>, end: Option<u64>, step: Option<u64>) -> StaticCollectionRegion {
        StaticCollectionRegion::Slice(StaticSlice { start, end, step })
    }

    #[test]
    fn positive_static_regions_prove_only_length_independent_facts() {
        assert_eq!(
            static_collection_relation(slice(None, Some(2), None), slice(Some(2), None, None)),
            StaticRegionRelation::Disjoint
        );
        assert_eq!(
            static_collection_relation(slice(None, None, Some(2)), slice(Some(1), None, Some(2))),
            StaticRegionRelation::Disjoint
        );
        assert_eq!(
            static_collection_relation(
                StaticCollectionRegion::Index(1),
                slice(None, Some(2), None)
            ),
            StaticRegionRelation::Overlap
        );
        assert_eq!(
            static_collection_relation(slice(None, Some(2), None), slice(Some(1), Some(3), None)),
            StaticRegionRelation::Runtime
        );
    }

    #[test]
    fn pattern_prefix_and_rest_are_statically_disjoint() {
        assert_eq!(
            static_collection_relation(
                StaticCollectionRegion::PatternIndex(1),
                StaticCollectionRegion::PatternRest {
                    start: 2,
                    suffix: 0,
                }
            ),
            StaticRegionRelation::Disjoint
        );
    }
}
