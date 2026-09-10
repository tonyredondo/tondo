//! Resolved binders shared by every structural meta declaration.

use crate::hir::{HirGenericParameter, HirProgram, HirTraitConstructor, HirTraitReference};
use crate::meta::{MetaBound, MetaGenericParameter};
use crate::resolve::ResolvedProgram;
use crate::types::{TypeId, TypeKind};

pub(crate) fn names(
    hir: &HirProgram,
    resolved: &ResolvedProgram,
    parameters: &[HirGenericParameter],
    self_type: Option<TypeId>,
) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for parameter in parameters {
        let position = parameter.position() as usize;
        names.resize(names.len().max(position + 1), String::new());
        names[position] = resolved
            .local(parameter.local())
            .ok_or("generic binder is absent")?
            .name()
            .as_str()
            .into();
    }
    if let Some(self_type) = self_type {
        let TypeKind::GenericParameter(position) = hir
            .interner()
            .kind(self_type)
            .map_err(|error| error.to_string())?
        else {
            return Err("trait Self is not a generic binder".into());
        };
        let position = *position as usize;
        names.resize(names.len().max(position + 1), String::new());
        names[position] = "Self".into();
    }
    Ok(names)
}

pub(crate) fn constructor(
    resolved: &ResolvedProgram,
    constructor: &HirTraitConstructor,
) -> Result<String, String> {
    Ok(match constructor {
        HirTraitConstructor::Symbol(id) => resolved
            .symbol(*id)
            .ok_or("bound trait is absent")?
            .identity()
            .canonical_name(),
        HirTraitConstructor::Prelude(name) => name.as_str().into(),
        HirTraitConstructor::External(identity) => identity.canonical_name(),
    })
}

pub(crate) fn reference(
    hir: &HirProgram,
    resolved: &ResolvedProgram,
    reference: &HirTraitReference,
) -> Result<String, String> {
    let name = constructor(resolved, reference.constructor())?;
    if reference.arguments().is_empty() {
        return Ok(name);
    }
    let arguments = reference
        .arguments()
        .iter()
        .map(|ty| {
            hir.interner()
                .canonical_interface(*ty)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("{name}[{}]", arguments.join(", ")))
}

pub(crate) fn parameters(
    hir: &HirProgram,
    resolved: &ResolvedProgram,
    parameters: &[HirGenericParameter],
) -> Result<Vec<MetaGenericParameter>, String> {
    parameters
        .iter()
        .map(|parameter| {
            let name = resolved
                .local(parameter.local())
                .ok_or("generic binder is absent")?
                .name()
                .as_str();
            let bounds = parameter
                .bounds()
                .iter()
                .map(|bound| reference(hir, resolved, bound))
                .collect::<Result<Vec<_>, _>>()?;
            MetaGenericParameter::new(name, bounds).map_err(|error| error.to_string())
        })
        .collect()
}

pub(crate) fn bounds(parameters: &[MetaGenericParameter]) -> Result<Vec<MetaBound>, String> {
    parameters
        .iter()
        .flat_map(|parameter| {
            parameter.bounds().iter().map(|bound| {
                MetaBound::new(parameter.name(), bound).map_err(|error| error.to_string())
            })
        })
        .collect()
}
