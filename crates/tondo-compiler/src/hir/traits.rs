use std::error::Error;
use std::fmt;

use crate::types::{TypeError, TypeId, TypeInterner};

use super::{HirImplementation, HirImplementationId, HirTraitConstructor, HirTraitReference};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TraitQuery {
    constructor: HirTraitConstructor,
    arguments: Vec<TypeId>,
    target: TypeId,
}

impl TraitQuery {
    pub(crate) fn new(reference: &HirTraitReference, target: TypeId) -> Self {
        Self {
            constructor: reference.constructor.clone(),
            arguments: reference.arguments.clone(),
            target,
        }
    }

    pub(crate) fn from_parts(
        constructor: HirTraitConstructor,
        arguments: Vec<TypeId>,
        target: TypeId,
    ) -> Self {
        Self {
            constructor,
            arguments,
            target,
        }
    }

    pub(crate) fn constructor(&self) -> &HirTraitConstructor {
        &self.constructor
    }

    pub(crate) fn arguments(&self) -> &[TypeId] {
        &self.arguments
    }

    pub(crate) fn target(&self) -> TypeId {
        self.target
    }

    fn components(&self) -> Vec<TypeId> {
        self.arguments
            .iter()
            .copied()
            .chain([self.target])
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TraitSelection {
    implementation: HirImplementationId,
    arguments: Vec<TypeId>,
}

impl TraitSelection {
    pub(crate) fn implementation(&self) -> HirImplementationId {
        self.implementation
    }

    pub(crate) fn arguments(&self) -> &[TypeId] {
        &self.arguments
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraitSelectionError {
    Ambiguous,
    Type(TypeError),
}

impl fmt::Display for TraitSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ambiguous => formatter.write_str("more than one coherent implementation matched"),
            Self::Type(error) => error.fmt(formatter),
        }
    }
}

impl Error for TraitSelectionError {}

impl From<TypeError> for TraitSelectionError {
    fn from(error: TypeError) -> Self {
        Self::Type(error)
    }
}

pub(crate) fn select_implementation(
    interner: &TypeInterner,
    implementations: &[HirImplementation],
    query: &TraitQuery,
) -> Result<Option<TraitSelection>, TraitSelectionError> {
    let actuals = query.components();
    let mut selected = None;
    for implementation in implementations.iter().filter(|implementation| {
        implementation.contract_complete
            && implementation.trait_reference.constructor == query.constructor
    }) {
        let patterns = implementation
            .trait_reference
            .arguments
            .iter()
            .copied()
            .chain([implementation.target])
            .collect::<Vec<_>>();
        let Some(arguments) = interner.first_order_pattern_substitution(
            &patterns,
            &actuals,
            u32::try_from(implementation.parameters.len())
                .map_err(|_| TypeError::ResourceLimit { limit: u32::MAX })?,
        )?
        else {
            continue;
        };
        if selected.is_some() {
            return Err(TraitSelectionError::Ambiguous);
        }
        selected = Some(TraitSelection {
            implementation: implementation.id,
            arguments,
        });
    }
    Ok(selected)
}
