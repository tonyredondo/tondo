use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use crate::types::{FunctionParameter, TypeError, TypeId, TypeInterner, TypeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SizeRelation {
    Unknown,
    Equal,
    Decrease,
}

impl SizeRelation {
    fn compose(after: Self, before: Self) -> Self {
        match (after, before) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Equal, Self::Equal) => Self::Equal,
            (Self::Equal | Self::Decrease, Self::Equal | Self::Decrease) => Self::Decrease,
        }
    }

    fn symbol(self) -> char {
        match self {
            Self::Unknown => '?',
            Self::Equal => '=',
            Self::Decrease => '<',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SizeChangeMatrix {
    rows: usize,
    columns: usize,
    cells: Vec<SizeRelation>,
}

impl SizeChangeMatrix {
    fn from_queries(
        interner: &TypeInterner,
        source: &[TypeId],
        destination: &[TypeId],
        budget: &mut AnalysisBudget,
    ) -> Result<Self, TraitTerminationError> {
        if source.is_empty() || destination.is_empty() {
            return Err(TraitTerminationError::EmptyQuery);
        }
        for ty in source.iter().chain(destination) {
            interner.kind(*ty)?;
        }
        let cell_count = destination.len().checked_mul(source.len()).ok_or(
            TraitTerminationError::ResourceLimit {
                limit: budget.limit,
            },
        )?;
        budget.consume(cell_count as u64)?;
        let mut cells = Vec::with_capacity(cell_count);
        for destination in destination {
            for source in source {
                let relation = if destination == source {
                    SizeRelation::Equal
                } else if strict_subterm(interner, *destination, *source, budget)? {
                    SizeRelation::Decrease
                } else {
                    SizeRelation::Unknown
                };
                cells.push(relation);
            }
        }
        Ok(Self {
            rows: destination.len(),
            columns: source.len(),
            cells,
        })
    }

    fn compose_after(
        &self,
        before: &Self,
        budget: &mut AnalysisBudget,
    ) -> Result<Self, TraitTerminationError> {
        if before.rows != self.columns {
            return Err(TraitTerminationError::InconsistentArity {
                left: before.rows,
                right: self.columns,
            });
        }
        let work = self
            .rows
            .checked_mul(before.columns)
            .and_then(|cells| cells.checked_mul(self.columns))
            .and_then(|work| u64::try_from(work).ok())
            .ok_or(TraitTerminationError::ResourceLimit {
                limit: budget.limit,
            })?;
        budget.consume(work)?;
        let cell_count =
            self.rows
                .checked_mul(before.columns)
                .ok_or(TraitTerminationError::ResourceLimit {
                    limit: budget.limit,
                })?;
        let mut cells = Vec::with_capacity(cell_count);
        for row in 0..self.rows {
            for column in 0..before.columns {
                let mut strongest = SizeRelation::Unknown;
                for middle in 0..self.columns {
                    strongest = strongest.max(SizeRelation::compose(
                        self[(row, middle)],
                        before[(middle, column)],
                    ));
                }
                cells.push(strongest);
            }
        }
        Ok(Self {
            rows: self.rows,
            columns: before.columns,
            cells,
        })
    }

    fn is_idempotent(&self, budget: &mut AnalysisBudget) -> Result<bool, TraitTerminationError> {
        if self.rows != self.columns {
            return Ok(false);
        }
        Ok(self.compose_after(self, budget)? == *self)
    }

    fn has_decreasing_diagonal(&self) -> bool {
        (0..self.rows.min(self.columns))
            .any(|position| self[(position, position)] == SizeRelation::Decrease)
    }

    pub(crate) fn render(&self) -> String {
        let mut output = String::from("[");
        for row in 0..self.rows {
            if row != 0 {
                output.push_str(", ");
            }
            output.push('[');
            for column in 0..self.columns {
                if column != 0 {
                    output.push_str(", ");
                }
                output.push(self[(row, column)].symbol());
            }
            output.push(']');
        }
        output.push(']');
        output
    }
}

impl std::ops::Index<(usize, usize)> for SizeChangeMatrix {
    type Output = SizeRelation;

    fn index(&self, (row, column): (usize, usize)) -> &Self::Output {
        &self.cells[row * self.columns + column]
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TraitTerminationEdge<N> {
    pub(crate) source: N,
    pub(crate) source_query: Vec<TypeId>,
    pub(crate) destination: N,
    pub(crate) destination_query: Vec<TypeId>,
    pub(crate) origin: usize,
}

#[derive(Debug, Clone)]
struct PreparedEdge<N> {
    source: N,
    destination: N,
    matrix: SizeChangeMatrix,
    origin: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SummaryKey<N> {
    source: N,
    destination: N,
    matrix: SizeChangeMatrix,
}

#[derive(Debug, Clone, Copy)]
enum SummaryWitness {
    Direct(usize),
    Compose(usize, usize),
}

#[derive(Debug, Clone)]
struct Summary<N> {
    key: SummaryKey<N>,
    witness: SummaryWitness,
}

#[derive(Debug, Clone)]
pub(crate) struct TraitTerminationFailure<N> {
    traits: Vec<N>,
    origins: Vec<usize>,
    matrix: SizeChangeMatrix,
}

impl<N> TraitTerminationFailure<N> {
    pub(crate) fn traits(&self) -> &[N] {
        &self.traits
    }

    pub(crate) fn origins(&self) -> &[usize] {
        &self.origins
    }

    pub(crate) fn matrix(&self) -> &SizeChangeMatrix {
        &self.matrix
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraitTerminationError {
    ResourceLimit { limit: u64 },
    EmptyQuery,
    InconsistentArity { left: usize, right: usize },
    Type(TypeError),
}

impl fmt::Display for TraitTerminationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResourceLimit { limit } => {
                write!(formatter, "trait termination work limit exceeded ({limit})")
            }
            Self::EmptyQuery => formatter.write_str("a trait query has no target component"),
            Self::InconsistentArity { left, right } => write!(
                formatter,
                "trait query arity is inconsistent across an edge ({left} versus {right})"
            ),
            Self::Type(error) => error.fmt(formatter),
        }
    }
}

impl Error for TraitTerminationError {}

impl From<TypeError> for TraitTerminationError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct AnalysisBudget {
    limit: u64,
    remaining: u64,
}

impl AnalysisBudget {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            remaining: limit,
        }
    }

    fn consume(&mut self, work: u64) -> Result<(), TraitTerminationError> {
        self.remaining = self
            .remaining
            .checked_sub(work)
            .ok_or(TraitTerminationError::ResourceLimit { limit: self.limit })?;
        Ok(())
    }
}

pub(crate) fn analyze_trait_termination<N>(
    interner: &TypeInterner,
    edges: &[TraitTerminationEdge<N>],
    max_steps: u64,
) -> Result<Vec<TraitTerminationFailure<N>>, TraitTerminationError>
where
    N: Clone + Ord,
{
    let mut budget = AnalysisBudget::new(max_steps);
    let mut prepared = Vec::with_capacity(edges.len());
    for edge in edges {
        prepared.push(PreparedEdge {
            source: edge.source.clone(),
            destination: edge.destination.clone(),
            matrix: SizeChangeMatrix::from_queries(
                interner,
                &edge.source_query,
                &edge.destination_query,
                &mut budget,
            )?,
            origin: edge.origin,
        });
    }
    prepared.sort_by(|left, right| {
        (&left.source, &left.destination, &left.matrix, left.origin).cmp(&(
            &right.source,
            &right.destination,
            &right.matrix,
            right.origin,
        ))
    });

    let nodes = prepared
        .iter()
        .flat_map(|edge| [edge.source.clone(), edge.destination.clone()])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut adjacency = nodes
        .iter()
        .cloned()
        .map(|node| (node, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for edge in &prepared {
        adjacency
            .get_mut(&edge.source)
            .expect("every prepared source is a graph node")
            .push(edge.destination.clone());
    }
    for destinations in adjacency.values_mut() {
        destinations.sort();
        destinations.dedup();
    }

    let mut failures = Vec::new();
    for component in strongly_connected_components(&nodes, &adjacency) {
        let members = component.iter().cloned().collect::<BTreeSet<_>>();
        let has_cycle = component.len() > 1
            || prepared
                .iter()
                .any(|edge| edge.source == component[0] && edge.destination == component[0]);
        if !has_cycle {
            continue;
        }
        let Some((summary_id, summaries)) = saturate_component(&prepared, &members, &mut budget)?
        else {
            continue;
        };
        failures.push(reconstruct_failure(
            summary_id,
            &summaries,
            &prepared,
            &mut budget,
        )?);
    }
    Ok(failures)
}

fn saturate_component<N>(
    edges: &[PreparedEdge<N>],
    members: &BTreeSet<N>,
    budget: &mut AnalysisBudget,
) -> Result<Option<(usize, Vec<Summary<N>>)>, TraitTerminationError>
where
    N: Clone + Ord,
{
    let mut summaries = Vec::<Summary<N>>::new();
    let mut by_key = BTreeMap::<SummaryKey<N>, usize>::new();
    let mut worklist = VecDeque::new();
    for (edge_id, edge) in edges.iter().enumerate() {
        if members.contains(&edge.source) && members.contains(&edge.destination) {
            insert_summary(
                SummaryKey {
                    source: edge.source.clone(),
                    destination: edge.destination.clone(),
                    matrix: edge.matrix.clone(),
                },
                SummaryWitness::Direct(edge_id),
                &mut summaries,
                &mut by_key,
                &mut worklist,
            );
        }
    }

    while let Some(current_id) = worklist.pop_front() {
        for other_id in 0..=current_id {
            let current = summaries[current_id].key.clone();
            let other = summaries[other_id].key.clone();
            if current.destination == other.source {
                insert_composition(
                    current_id,
                    &current,
                    other_id,
                    &other,
                    budget,
                    &mut summaries,
                    &mut by_key,
                    &mut worklist,
                )?;
            }
            if current_id != other_id && other.destination == current.source {
                insert_composition(
                    other_id,
                    &other,
                    current_id,
                    &current,
                    budget,
                    &mut summaries,
                    &mut by_key,
                    &mut worklist,
                )?;
            }
        }
    }

    for (key, summary_id) in &by_key {
        if key.source == key.destination
            && key.matrix.is_idempotent(budget)?
            && !key.matrix.has_decreasing_diagonal()
        {
            return Ok(Some((*summary_id, summaries)));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn insert_composition<N>(
    before_id: usize,
    before: &SummaryKey<N>,
    after_id: usize,
    after: &SummaryKey<N>,
    budget: &mut AnalysisBudget,
    summaries: &mut Vec<Summary<N>>,
    by_key: &mut BTreeMap<SummaryKey<N>, usize>,
    worklist: &mut VecDeque<usize>,
) -> Result<(), TraitTerminationError>
where
    N: Clone + Ord,
{
    let matrix = after.matrix.compose_after(&before.matrix, budget)?;
    insert_summary(
        SummaryKey {
            source: before.source.clone(),
            destination: after.destination.clone(),
            matrix,
        },
        SummaryWitness::Compose(before_id, after_id),
        summaries,
        by_key,
        worklist,
    );
    Ok(())
}

fn insert_summary<N>(
    key: SummaryKey<N>,
    witness: SummaryWitness,
    summaries: &mut Vec<Summary<N>>,
    by_key: &mut BTreeMap<SummaryKey<N>, usize>,
    worklist: &mut VecDeque<usize>,
) where
    N: Clone + Ord,
{
    if by_key.contains_key(&key) {
        return;
    }
    let id = summaries.len();
    by_key.insert(key.clone(), id);
    summaries.push(Summary { key, witness });
    worklist.push_back(id);
}

fn reconstruct_failure<N>(
    summary_id: usize,
    summaries: &[Summary<N>],
    edges: &[PreparedEdge<N>],
    budget: &mut AnalysisBudget,
) -> Result<TraitTerminationFailure<N>, TraitTerminationError>
where
    N: Clone + Ord,
{
    let mut pending = vec![summary_id];
    let mut path = Vec::new();
    while let Some(summary_id) = pending.pop() {
        budget.consume(1)?;
        match summaries[summary_id].witness {
            SummaryWitness::Direct(edge_id) => path.push(edge_id),
            SummaryWitness::Compose(before, after) => {
                pending.push(after);
                pending.push(before);
            }
        }
    }
    let first = *path
        .first()
        .expect("a saturated cycle summary has at least one direct edge");
    let mut traits = vec![edges[first].source.clone()];
    let mut origins = Vec::with_capacity(path.len());
    for edge_id in path {
        let edge = &edges[edge_id];
        traits.push(edge.destination.clone());
        origins.push(edge.origin);
    }
    Ok(TraitTerminationFailure {
        traits,
        origins,
        matrix: summaries[summary_id].key.matrix.clone(),
    })
}

fn strict_subterm(
    interner: &TypeInterner,
    needle: TypeId,
    root: TypeId,
    budget: &mut AnalysisBudget,
) -> Result<bool, TraitTerminationError> {
    let mut pending = Vec::new();
    push_type_children(interner.kind(root)?, &mut pending);
    let mut visited = BTreeSet::new();
    while let Some(ty) = pending.pop() {
        budget.consume(1)?;
        if ty == needle {
            return Ok(true);
        }
        if visited.insert(ty) {
            push_type_children(interner.kind(ty)?, &mut pending);
        }
    }
    Ok(false)
}

fn push_type_children(kind: &TypeKind, pending: &mut Vec<TypeId>) {
    match kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Union(arguments)
        | TypeKind::Intrinsic { arguments, .. }
        | TypeKind::Generated { arguments, .. } => {
            pending.extend(arguments.iter().copied());
        }
        TypeKind::Function(function) => {
            pending.extend(function.parameters().iter().map(FunctionParameter::ty));
            pending.extend(function.variadic());
            pending.push(function.outcome());
        }
        TypeKind::Option(item) => pending.push(*item),
        TypeKind::Result { success, error } => {
            pending.push(*success);
            pending.push(*error);
        }
        TypeKind::Cursor { collection, .. } => pending.push(*collection),
        TypeKind::Error
        | TypeKind::Scalar(_)
        | TypeKind::GenericParameter(_)
        | TypeKind::Inference(_)
        | TypeKind::OpaqueResult(_) => {}
    }
}

fn strongly_connected_components<N>(nodes: &[N], adjacency: &BTreeMap<N, Vec<N>>) -> Vec<Vec<N>>
where
    N: Clone + Ord,
{
    let node_set = nodes.iter().cloned().collect::<BTreeSet<_>>();
    let mut visited = BTreeSet::new();
    let mut finished = Vec::with_capacity(nodes.len());
    for root in nodes {
        if !visited.insert(root.clone()) {
            continue;
        }
        let mut stack = vec![(root.clone(), 0_usize)];
        while let Some((node, index)) = stack.last_mut() {
            let neighbors = adjacency.get(node).map(Vec::as_slice).unwrap_or_default();
            if let Some(next) = neighbors.get(*index).cloned() {
                *index += 1;
                if node_set.contains(&next) && visited.insert(next.clone()) {
                    stack.push((next, 0));
                }
            } else {
                finished.push(node.clone());
                stack.pop();
            }
        }
    }

    let mut reverse = nodes
        .iter()
        .cloned()
        .map(|node| (node, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for (source, destinations) in adjacency {
        for destination in destinations {
            if node_set.contains(source) && node_set.contains(destination) {
                reverse
                    .get_mut(destination)
                    .expect("all SCC nodes have a reverse entry")
                    .push(source.clone());
            }
        }
    }
    for neighbors in reverse.values_mut() {
        neighbors.sort();
        neighbors.dedup();
    }

    visited.clear();
    let mut components = Vec::new();
    for root in finished.into_iter().rev() {
        if !visited.insert(root.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            component.push(node.clone());
            for next in reverse[&node].iter().rev() {
                if visited.insert(next.clone()) {
                    stack.push(next.clone());
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort_by(|left, right| left[0].cmp(&right[0]));
    components
}

#[cfg(test)]
mod tests {
    use crate::types::{IntrinsicType, ScalarType};

    use super::*;

    fn edge<N>(
        source: N,
        source_query: Vec<TypeId>,
        destination: N,
        destination_query: Vec<TypeId>,
        origin: usize,
    ) -> TraitTerminationEdge<N> {
        TraitTerminationEdge {
            source,
            source_query,
            destination,
            destination_query,
            origin,
        }
    }

    #[test]
    fn size_change_accepts_descent_and_acyclic_unknown_edges() {
        let mut interner = TypeInterner::default();
        let parameter = interner.generic_parameter(0).unwrap();
        let array = interner
            .intrinsic(IntrinsicType::Array, vec![parameter])
            .unwrap();
        let descending = [edge("Walk", vec![array], "Walk", vec![parameter], 0)];
        assert!(
            analyze_trait_termination(&interner, &descending, 10_000)
                .unwrap()
                .is_empty()
        );

        let int = interner.scalar(ScalarType::Int);
        let string = interner.scalar(ScalarType::String);
        let acyclic = [edge("Render", vec![int], "Summary", vec![string], 0)];
        assert!(
            analyze_trait_termination(&interner, &acyclic, 10_000)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn size_change_rejects_equal_self_and_mutual_cycles_with_witnesses() {
        let mut interner = TypeInterner::default();
        let parameter = interner.generic_parameter(0).unwrap();
        let equal = [edge("Loop", vec![parameter], "Loop", vec![parameter], 7)];
        let failures = analyze_trait_termination(&interner, &equal, 10_000).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].traits(), ["Loop", "Loop"]);
        assert_eq!(failures[0].origins(), [7]);
        assert_eq!(failures[0].matrix().render(), "[[=]]");

        let mutual = [
            edge("Left", vec![parameter], "Right", vec![parameter], 3),
            edge("Right", vec![parameter], "Left", vec![parameter], 5),
        ];
        let failures = analyze_trait_termination(&interner, &mutual, 10_000).unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].traits(), ["Left", "Right", "Left"]);
        assert_eq!(failures[0].origins(), [3, 5]);
        assert_eq!(failures[0].matrix().render(), "[[=]]");
    }

    #[test]
    fn size_change_analysis_has_an_explicit_work_budget() {
        let mut interner = TypeInterner::default();
        let parameter = interner.generic_parameter(0).unwrap();
        let equal = [edge("Loop", vec![parameter], "Loop", vec![parameter], 0)];
        assert!(matches!(
            analyze_trait_termination(&interner, &equal, 0),
            Err(TraitTerminationError::ResourceLimit { limit: 0 })
        ));
    }
}
