//! Canonical type identity and explicit source-rendering data for meta programs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hir::HirProgram;
use crate::package::{Namespace, PackageGraph, PackageId, SymbolIdentity};
use crate::resolve::{ResolvedProgram, Visibility};
use crate::types::{IntrinsicType, TypeId, TypeKind};

/// A type's identity is independent of the spelling used by its declaration.
/// The authored spelling is retained for the standard derive renderer, and is
/// hashed along with the explicit rendering plan; neither is ambient state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaTypeRef {
    identity: String,
    source: String,
    parts: Vec<MetaTypePart>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MetaTypePart {
    Text(String),
    Name(MetaTypeName),
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaTypeName {
    pub module: String,
    pub local: bool,
    pub public: bool,
    pub name: String,
    pub import: Option<String>,
    pub alias: String,
}

impl MetaTypeRef {
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn parts(&self) -> &[MetaTypePart] {
        &self.parts
    }

    /// Constructed from resolved HIR, never by interpreting an author's alias.
    pub(crate) fn resolved(
        hir: &HirProgram,
        resolved: &ResolvedProgram,
        packages: &PackageGraph,
        owner: &PackageId,
        ty: TypeId,
        source: String,
        parameters: &[String],
    ) -> Result<Self, crate::types::TypeError> {
        Self::resolved_in(
            hir.interner(),
            resolved,
            packages,
            owner,
            ty,
            source,
            parameters,
        )
    }

    pub(crate) fn resolved_in(
        interner: &crate::types::TypeInterner,
        resolved: &ResolvedProgram,
        packages: &PackageGraph,
        owner: &PackageId,
        ty: TypeId,
        source: String,
        parameters: &[String],
    ) -> Result<Self, crate::types::TypeError> {
        let mut names = Vec::new();
        // Delimiters cannot occur in names or in canonical type punctuation.
        // They remain internal and are replaced by structured data below.
        let rendered = interner.render_named(ty, &mut |kind| {
            let part = match kind {
                TypeKind::Nominal { identity, .. } => {
                    MetaTypePart::Name(type_name(packages, resolved, owner, identity))
                }
                TypeKind::GenericParameter(index) => parameters
                    .get(*index as usize)
                    .filter(|name| !name.is_empty())
                    .map(|name| MetaTypePart::Text(name.clone()))
                    .unwrap_or_else(|| {
                        MetaTypePart::Unavailable(format!("unbound type parameter {index}"))
                    }),
                TypeKind::Intrinsic { constructor, .. } => {
                    match intrinsic_identity(resolved, packages, *constructor) {
                        Some(identity) => {
                            let mut name = type_name(packages, resolved, owner, &identity);
                            name.public = true;
                            MetaTypePart::Name(name)
                        }
                        None if is_prelude(*constructor) => {
                            MetaTypePart::Text(constructor.as_str().into())
                        }
                        None => MetaTypePart::Unavailable(format!(
                            "type {} has no admitted source name",
                            constructor.as_str()
                        )),
                    }
                }
                TypeKind::OpaqueResult { .. }
                | TypeKind::Generated { .. }
                | TypeKind::Cursor { .. } => MetaTypePart::Unavailable(
                    "type has no directly nameable source representation".into(),
                ),
                _ => unreachable!("only named atoms reach this callback"),
            };
            let index = names.len();
            names.push(part);
            format!("\u{1f}{index}\u{1f}")
        })?;
        let mut parts = Vec::new();
        for (index, part) in rendered.split('\u{1f}').enumerate() {
            if index % 2 == 0 {
                if !part.is_empty() {
                    parts.push(MetaTypePart::Text(part.into()));
                }
            } else {
                parts.push(
                    names[part.parse::<usize>().expect("renderer marker is an index")].clone(),
                );
            }
        }
        let identity = interner.render_named(ty, &mut |kind| match kind {
            TypeKind::Nominal { identity, .. } => identity.canonical_name(),
            TypeKind::GenericParameter(index) => format!("${index}"),
            TypeKind::Intrinsic { constructor, .. } => {
                intrinsic_identity(resolved, packages, *constructor)
                    .map(|identity| identity.canonical_name())
                    .unwrap_or_else(|| {
                        if is_prelude(*constructor) {
                            constructor.as_str().into()
                        } else {
                            format!("internal:{constructor:?}")
                        }
                    })
            }
            kind => crate::types::canonical_type_name(kind),
        })?;
        Ok(Self {
            identity,
            source,
            parts,
        })
    }
}

// The independent bounded model also constructs leaf types without a HIR.
// These constructors describe literal type syntax; production uses `resolved`.
impl From<String> for MetaTypeRef {
    fn from(source: String) -> Self {
        Self {
            identity: source.clone(),
            parts: vec![MetaTypePart::Text(source.clone())],
            source,
        }
    }
}
impl From<&str> for MetaTypeRef {
    fn from(source: &str) -> Self {
        source.to_owned().into()
    }
}
impl std::fmt::Display for MetaTypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}

fn is_prelude(constructor: IntrinsicType) -> bool {
    matches!(
        constructor,
        IntrinsicType::Array
            | IntrinsicType::Map
            | IntrinsicType::Set
            | IntrinsicType::Range
            | IntrinsicType::Ref
            | IntrinsicType::Pointer
            | IntrinsicType::Join
    )
}

fn intrinsic_identity(
    resolved: &ResolvedProgram,
    packages: &PackageGraph,
    constructor: IntrinsicType,
) -> Option<SymbolIdentity> {
    use crate::package::DeclarationPath;
    use crate::resolve::{ResolvedEntity, ResolvedName};
    let mut candidates = std::collections::BTreeSet::new();
    for reference in resolved.references() {
        let name = match reference.entity() {
            ResolvedEntity::Name(name)
            | ResolvedEntity::ContextualCandidates {
                type_name: name, ..
            } => name,
            _ => continue,
        };
        if let ResolvedName::External {
            module,
            namespace: Namespace::Type,
            name,
        } = name
            && crate::hir::bootstrap_process_intrinsic(module, name) == Some(constructor)
            && let Ok(identity) = packages.symbol_identity(
                module.clone(),
                Namespace::Type,
                DeclarationPath::single(name.clone()),
            )
        {
            candidates.insert(identity);
        }
    }
    for symbol in resolved.symbols() {
        if symbol.identity().package() == packages.standard()
            && symbol.identity().namespace() == Namespace::Type
            && packages
                .module(symbol.identity().package(), symbol.identity().module())
                .is_some_and(|module| {
                    crate::hir::bootstrap_process_intrinsic(&module, symbol.name())
                        == Some(constructor)
                })
        {
            candidates.insert(symbol.identity().clone());
        }
    }
    candidates.into_iter().next()
}

fn type_name(
    packages: &PackageGraph,
    resolved: &ResolvedProgram,
    owner: &PackageId,
    identity: &SymbolIdentity,
) -> MetaTypeName {
    let local = identity.package() == owner;
    let package = packages
        .package(owner)
        .expect("snapshot owner belongs to graph");
    let prefix = if local {
        Some(package.local_name().as_str())
    } else if identity.package() == packages.standard() {
        Some("std")
    } else {
        package
            .dependencies()
            .iter()
            .find(|(_, id)| *id == identity.package())
            .map(|(alias, _)| alias.as_str())
    };
    let module = identity.module().as_str();
    let hash = crate::artifact::sha256(format!("{}::{module}", identity.package()).as_bytes());
    let alias = format!("__tondo_meta_{}", &hash[7..]);
    let import = prefix.map(|prefix| format!("import {prefix}.{module} as {alias}\n"));
    let public = resolved
        .symbols()
        .find(|symbol| symbol.identity() == identity)
        .is_some_and(|symbol| symbol.visibility() == Visibility::Public);
    MetaTypeName {
        module: module.into(),
        local,
        public,
        name: identity.declaration().to_string(),
        import,
        alias,
    }
}

/// Reference rendering oracle, with the same import ordering as the source SDK.
pub fn render_type(ty: &MetaTypeRef, module: &str) -> Result<(String, Vec<String>), String> {
    let mut output = String::new();
    let mut imports = BTreeMap::new();
    for part in ty.parts() {
        match part {
            MetaTypePart::Text(text) => output.push_str(text),
            MetaTypePart::Unavailable(reason) => return Err(reason.clone()),
            MetaTypePart::Name(name) if name.local && name.module == module => {
                output.push_str(&name.name)
            }
            MetaTypePart::Name(name) => {
                if !name.public {
                    return Err("type is private to another module".into());
                }
                let import = name
                    .import
                    .as_ref()
                    .ok_or("type package is not a direct dependency")?;
                imports.insert(name.alias.clone(), import.clone());
                output.push_str(&format!("{}.{}", name.alias, name.name));
            }
        }
    }
    Ok((output, imports.into_values().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::*;
    use crate::meta_vm::{MetaEntryKind, MetaVmArtifact, MetaVmError, MetaVmLimits};
    use crate::std_meta::source_api;

    #[test]
    fn meta_type_source_builder_scopes_imports_and_failed_rendering_is_atomic() {
        let source = r#"
import std.meta
pub fn generate(request: meta.GenerateRequest): meta.GenerateResponse ! meta.Error {
    let fields = match request.snapshot().declarations[0].kind {
        meta.DeclarationKind.Record(value) => value
        _ => panic("record expected")
    }
    var builder = request.sourceBuilder()
    let remote = request.outputs()[1].path
    assert(match builder.renderType(remote, fields[1].typeRef) {
        err(meta.Error.TypeRendering(_)) => true
        _ => false
    })
    for output in request.outputs() {
        let ty = builder.renderType(output.path, fields[0].typeRef)?
        builder.add(output.path, "pub type ResultType = {ty}\n")?
        assert(match builder.renderType(output.path, fields[0].typeRef) {
            err(meta.Error.DuplicateOutput(_)) => true
            _ => false
        })
    }
    builder.finish()
}
"#;
        let user = MetaTypeName {
            module: "model".into(),
            local: true,
            public: true,
            name: "User".into(),
            import: Some("import app.model as modelTypes\n".into()),
            alias: "modelTypes".into(),
        };
        let good = MetaTypeRef {
            identity: "canonical:User".into(),
            source: "UserAlias".into(),
            parts: vec![MetaTypePart::Name(user.clone())],
        };
        let bad = MetaTypeRef {
            identity: "canonical:private-tuple".into(),
            source: "(Other, Private)".into(),
            parts: vec![
                MetaTypePart::Text("(".into()),
                MetaTypePart::Name(MetaTypeName {
                    module: "other".into(),
                    name: "Other".into(),
                    import: Some("import app.other as otherTypes\n".into()),
                    alias: "otherTypes".into(),
                    ..user.clone()
                }),
                MetaTypePart::Text(", ".into()),
                MetaTypePart::Name(MetaTypeName {
                    public: false,
                    name: "Private".into(),
                    ..user
                }),
                MetaTypePart::Text(")".into()),
            ],
        };
        assert!(
            render_type(&bad, "generated")
                .unwrap_err()
                .contains("private")
        );
        assert!(render_type(&bad, "model").is_ok());
        let span = MetaSpan::new(0, 0, 1).unwrap();
        let snapshot = MetaSnapshot::new(
            MetaEnvironment::meta(),
            [MetaRoot::new("app", "model").unwrap()],
            [MetaModule::new("model", None::<String>).unwrap()],
            [MetaDeclaration::new(
                "Owner",
                "model",
                MetaVisibility::Private,
                [],
                [],
                span,
                None::<String>,
                MetaDeclarationKind::record([good.clone(), bad].into_iter().enumerate().map(
                    |(index, ty)| {
                        MetaField::new(
                            format!("field{index}"),
                            ty,
                            MetaVisibility::Private,
                            index as u32,
                            span,
                            None::<String>,
                        )
                        .unwrap()
                    },
                )),
            )
            .unwrap()],
        )
        .unwrap();
        let artifact = MetaVmArtifact::compile_entry(
            crate::meta_test_support::source_request(source),
            "provider.generate",
            MetaEntryKind::Generate,
        )
        .unwrap();
        for limit in [2048, 32] {
            let limits = MetaLimits::new(100_000, 1_048_576, limit).unwrap();
            let request = MetaRequest::new(
                snapshot.clone(),
                [],
                [
                    MetaOutputSpec::new("generated/a.to", "model").unwrap(),
                    MetaOutputSpec::new("generated/b.to", "generated").unwrap(),
                ],
                limits,
            )
            .unwrap();
            let program = artifact
                .clone()
                .load(MetaVmLimits::for_request(limits))
                .unwrap();
            let execution = program
                .run_with_request(source_api::generate_request(&request), |outcome| {
                    source_api::measure_generate_output(outcome, &request)
                });
            if limit == 32 {
                assert!(
                    matches!(
                        execution,
                        Err(MetaVmError::ReportedOutputLimit { limit: 32 })
                    ),
                    "{execution:?}"
                );
                continue;
            }
            let execution = execution.unwrap();
            let response = source_api::generate_response(&execution.outcome, &request).unwrap();
            for output in response.outputs() {
                let (ty, imports) = render_type(&good, output.module()).unwrap();
                let expected = format!("{}pub type ResultType = {ty}\n", imports.concat());
                // The response validator also applies canonical formatting.
                assert_eq!(std::str::from_utf8(output.bytes()).unwrap(), expected);
                assert!(!expected.contains("otherTypes"));
            }
        }
    }
}
