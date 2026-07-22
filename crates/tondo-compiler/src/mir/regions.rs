use std::collections::BTreeMap;

use crate::hir::{
    HirConstantValueKind, HirIndexAccess, HirProgram, StaticCollectionRegion, StaticRegionRelation,
    StaticSlice, parse_nonnegative_integer, static_collection_relation,
};

use super::{
    MirConstant, MirFunction, MirLocalId, MirLocalKind, MirOperandKind, MirPlace, MirProjection,
    MirProjectionKind, MirRvalue, MirRvalueKind, MirStatementKind, MirTerminatorKind,
};

pub(super) fn loan_place_relation(
    left: &MirPlace,
    right: &MirPlace,
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> StaticRegionRelation {
    if left.local != right.local {
        return StaticRegionRelation::Disjoint;
    }
    let mut relation = StaticRegionRelation::Overlap;
    for (left, right) in left.projections.iter().zip(&right.projections) {
        match (
            collection_region(left, static_integers),
            collection_region(right, static_integers),
        ) {
            (CollectionComponent::Static(left), CollectionComponent::Static(right)) => {
                let current = static_collection_relation(left, right);
                if current == StaticRegionRelation::Disjoint {
                    return current;
                }
                if static_regions_are_identical(left, right) {
                    relation = current;
                    continue;
                }
                return current;
            }
            (CollectionComponent::Dynamic, _)
            | (_, CollectionComponent::Dynamic)
            | (CollectionComponent::Static(_), CollectionComponent::None)
            | (CollectionComponent::None, CollectionComponent::Static(_)) => {
                return StaticRegionRelation::Runtime;
            }
            (CollectionComponent::None, CollectionComponent::None) => {
                if left == right {
                    continue;
                }
                if fixed_projections_are_disjoint(left.kind(), right.kind()) {
                    return StaticRegionRelation::Disjoint;
                }
                return StaticRegionRelation::Overlap;
            }
        }
    }
    relation
}

#[derive(Clone, Copy)]
enum CollectionComponent {
    None,
    Static(StaticCollectionRegion),
    Dynamic,
}

fn collection_region(
    projection: &MirProjection,
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> CollectionComponent {
    match projection.kind() {
        MirProjectionKind::ArrayPatternIndex(index) => {
            CollectionComponent::Static(StaticCollectionRegion::PatternIndex(*index))
        }
        MirProjectionKind::ArrayPatternRest { start, suffix } => {
            CollectionComponent::Static(StaticCollectionRegion::PatternRest {
                start: *start,
                suffix: *suffix,
            })
        }
        MirProjectionKind::Index {
            index,
            access: HirIndexAccess::Array,
        } => static_integers
            .get(index)
            .map_or(CollectionComponent::Dynamic, |index| {
                CollectionComponent::Static(StaticCollectionRegion::Index(*index))
            }),
        MirProjectionKind::Index {
            access: HirIndexAccess::MapLookup | HirIndexAccess::MapEntry,
            ..
        } => CollectionComponent::Dynamic,
        MirProjectionKind::Slice { start, end, step } => {
            let Some(start) = static_optional_bound(*start, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(end) = static_optional_bound(*end, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            let Some(step) = static_optional_bound(*step, static_integers) else {
                return CollectionComponent::Dynamic;
            };
            CollectionComponent::Static(StaticCollectionRegion::Slice(StaticSlice {
                start,
                end,
                step,
            }))
        }
        MirProjectionKind::ClosureCapture { .. }
        | MirProjectionKind::Field(_)
        | MirProjectionKind::TupleField(_)
        | MirProjectionKind::NewtypeValue
        | MirProjectionKind::VariantTuple { .. }
        | MirProjectionKind::VariantField { .. }
        | MirProjectionKind::OptionValue
        | MirProjectionKind::ResultOkValue
        | MirProjectionKind::ResultErrValue
        | MirProjectionKind::UnionValue(_) => CollectionComponent::None,
    }
}

fn static_optional_bound(
    local: Option<MirLocalId>,
    static_integers: &BTreeMap<MirLocalId, u64>,
) -> Option<Option<u64>> {
    match local {
        Some(local) => Some(Some(*static_integers.get(&local)?)),
        None => Some(None),
    }
}

fn static_regions_are_identical(
    left: StaticCollectionRegion,
    right: StaticCollectionRegion,
) -> bool {
    if left == right {
        return true;
    }
    matches!(
        (left, right),
        (
            StaticCollectionRegion::Index(left),
            StaticCollectionRegion::PatternIndex(right)
        ) | (
            StaticCollectionRegion::PatternIndex(right),
            StaticCollectionRegion::Index(left)
        ) if left == u64::from(right)
    )
}

fn fixed_projections_are_disjoint(left: &MirProjectionKind, right: &MirProjectionKind) -> bool {
    match (left, right) {
        (
            MirProjectionKind::ClosureCapture {
                closure: left_closure,
                index: left,
            },
            MirProjectionKind::ClosureCapture {
                closure: right_closure,
                index: right,
            },
        ) => left_closure != right_closure || left != right,
        (MirProjectionKind::Field(left), MirProjectionKind::Field(right)) => left != right,
        (MirProjectionKind::TupleField(left), MirProjectionKind::TupleField(right)) => {
            left != right
        }
        (
            MirProjectionKind::VariantTuple {
                variant: left_variant,
                index: left,
            },
            MirProjectionKind::VariantTuple {
                variant: right_variant,
                index: right,
            },
        ) => left_variant != right_variant || left != right,
        (
            MirProjectionKind::VariantField {
                variant: left_variant,
                field: left,
            },
            MirProjectionKind::VariantField {
                variant: right_variant,
                field: right,
            },
        ) => left_variant != right_variant || left != right,
        (
            MirProjectionKind::VariantTuple { variant: left, .. }
            | MirProjectionKind::VariantField { variant: left, .. },
            MirProjectionKind::VariantTuple { variant: right, .. }
            | MirProjectionKind::VariantField { variant: right, .. },
        ) => left != right,
        (MirProjectionKind::OptionValue, MirProjectionKind::ResultOkValue)
        | (MirProjectionKind::OptionValue, MirProjectionKind::ResultErrValue)
        | (MirProjectionKind::ResultOkValue, MirProjectionKind::OptionValue)
        | (MirProjectionKind::ResultErrValue, MirProjectionKind::OptionValue)
        | (MirProjectionKind::ResultOkValue, MirProjectionKind::ResultErrValue)
        | (MirProjectionKind::ResultErrValue, MirProjectionKind::ResultOkValue) => true,
        (MirProjectionKind::UnionValue(left), MirProjectionKind::UnionValue(right)) => {
            left != right
        }
        _ => false,
    }
}

pub(super) fn static_integer_locals(
    hir: &HirProgram,
    function: &MirFunction,
) -> BTreeMap<MirLocalId, u64> {
    let mut candidates = BTreeMap::<MirLocalId, Option<u64>>::new();
    let mut record = |place: &MirPlace, value: Option<u64>| {
        if !place.projections.is_empty()
            || function.locals[place.local.index() as usize].kind != MirLocalKind::Temporary
        {
            return;
        }
        candidates
            .entry(place.local)
            .and_modify(|candidate| *candidate = None)
            .or_insert(value);
    };
    for block in &function.blocks {
        for statement in &block.statements {
            if let MirStatementKind::Assign { destination, value } = &statement.kind {
                record(destination, static_integer_rvalue(hir, value));
            }
        }
        match &block.terminator.kind {
            MirTerminatorKind::Invoke {
                destination: Some(destination),
                ..
            }
            | MirTerminatorKind::IteratorNext { destination, .. } => record(destination, None),
            MirTerminatorKind::Goto { .. }
            | MirTerminatorKind::SwitchBool { .. }
            | MirTerminatorKind::SwitchTag { .. }
            | MirTerminatorKind::Invoke {
                destination: None, ..
            }
            | MirTerminatorKind::ValidatePlaces { .. }
            | MirTerminatorKind::ValidateLoan { .. }
            | MirTerminatorKind::Return
            | MirTerminatorKind::ResumePanic
            | MirTerminatorKind::Unreachable => {}
        }
    }
    candidates
        .into_iter()
        .filter_map(|(local, value)| value.map(|value| (local, value)))
        .collect()
}

fn static_integer_rvalue(hir: &HirProgram, value: &MirRvalue) -> Option<u64> {
    let MirRvalueKind::Use(operand) = &value.kind else {
        return None;
    };
    match &operand.kind {
        MirOperandKind::Constant(MirConstant::Integer(spelling)) => {
            parse_nonnegative_integer(spelling)
        }
        MirOperandKind::Constant(MirConstant::Named(symbol)) => {
            let HirConstantValueKind::Integer(value) = hir.constant(*symbol)?.evaluated()?.kind()
            else {
                return None;
            };
            u64::try_from(*value).ok()
        }
        MirOperandKind::Constant(
            MirConstant::Unit
            | MirConstant::Bool(_)
            | MirConstant::Float(_)
            | MirConstant::Char(_)
            | MirConstant::String(_),
        )
        | MirOperandKind::Copy(_)
        | MirOperandKind::Move(_)
        | MirOperandKind::Borrow(_)
        | MirOperandKind::Loan(_)
        | MirOperandKind::Function { .. }
        | MirOperandKind::PreludeTraitFunction { .. } => None,
    }
}
