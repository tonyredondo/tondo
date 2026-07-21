use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::source::{FileId, ModulePath, SourceDatabase, SourceError, SourceId};
use crate::syntax::TokenKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Edition {
    V0_1,
}

impl Edition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V0_1 => "0.1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Discard,
    Keyword(String),
    InvalidIdentifier(String),
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a name cannot be empty"),
            Self::Discard => formatter.write_str("`_` is a discard, not a name"),
            Self::Keyword(name) => write!(formatter, "`{name}` is a keyword, not a name"),
            Self::InvalidIdentifier(name) => {
                write!(formatter, "`{name}` is not a valid Tondo identifier")
            }
        }
    }
}

impl Error for NameError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    pub fn new(value: impl AsRef<str>) -> Result<Self, NameError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(NameError::Empty);
        }
        let normalized = value.nfc().collect::<String>();
        if normalized == "_" {
            return Err(NameError::Discard);
        }
        if TokenKind::from_keyword(&normalized).is_some() {
            return Err(NameError::Keyword(normalized));
        }
        let mut characters = normalized.chars();
        let Some(first) = characters.next() else {
            return Err(NameError::Empty);
        };
        if !(first == '_' || unicode_ident::is_xid_start(first))
            || !characters.all(unicode_ident::is_xid_continue)
        {
            return Err(NameError::InvalidIdentifier(normalized));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(String);

impl PackageId {
    pub fn new(value: impl Into<String>) -> Result<Self, PackageGraphError> {
        let value = value.into();
        if value.is_empty() {
            return Err(PackageGraphError::EmptyPackageId);
        }
        if value.contains(['\n', '\r']) {
            return Err(PackageGraphError::PackageIdContainsLineBreak(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageAlias(Name);

impl PackageAlias {
    pub fn new(value: impl AsRef<str>) -> Result<Self, PackageGraphError> {
        let name = Name::new(value).map_err(PackageGraphError::InvalidPackageAlias)?;
        if name.as_str() == "std" {
            return Err(PackageGraphError::ReservedStandardAlias);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for PackageAlias {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageGraphError {
    EmptyPackageId,
    PackageIdContainsLineBreak(String),
    InvalidPackageAlias(NameError),
    ReservedStandardAlias,
    DuplicatePackage(PackageId),
    DuplicateSourceId(SourceId),
    DuplicateDependencyAlias(PackageAlias),
    AliasMatchesCurrentPackage(PackageAlias),
    UnknownRootPackage(PackageId),
    UnknownStandardPackage(PackageId),
    RootIsStandardPackage,
    UnknownDependency {
        package: PackageId,
        dependency: PackageId,
    },
    DependencyCycle,
    UnknownSourceId(SourceId),
    UndeclaredModule {
        package: PackageId,
        module: ModulePath,
    },
    RootSourceOwnedByAnotherPackage {
        root: FileId,
        expected: PackageId,
        actual: PackageId,
    },
    Source(SourceError),
}

impl fmt::Display for PackageGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPackageId => formatter.write_str("a package ID cannot be empty"),
            Self::PackageIdContainsLineBreak(id) => {
                write!(formatter, "package ID `{id}` contains a line break")
            }
            Self::InvalidPackageAlias(error) => write!(formatter, "invalid package alias: {error}"),
            Self::ReservedStandardAlias => {
                formatter.write_str("`std` is reserved for the selected standard package")
            }
            Self::DuplicatePackage(package) => write!(formatter, "duplicate package `{package}`"),
            Self::DuplicateSourceId(source) => {
                write!(
                    formatter,
                    "source ID `{source}` belongs to more than one package"
                )
            }
            Self::DuplicateDependencyAlias(alias) => {
                write!(formatter, "duplicate dependency alias `{alias}`")
            }
            Self::AliasMatchesCurrentPackage(alias) => write!(
                formatter,
                "dependency alias `{alias}` matches the current package name"
            ),
            Self::UnknownRootPackage(package) => {
                write!(formatter, "unknown root package `{package}`")
            }
            Self::UnknownStandardPackage(package) => {
                write!(formatter, "unknown standard package `{package}`")
            }
            Self::RootIsStandardPackage => {
                formatter.write_str("the root package and standard package must be distinct")
            }
            Self::UnknownDependency {
                package,
                dependency,
            } => write!(
                formatter,
                "package `{package}` references unknown dependency `{dependency}`"
            ),
            Self::DependencyCycle => formatter.write_str("the package graph contains a cycle"),
            Self::UnknownSourceId(source) => {
                write!(formatter, "source ID `{source}` has no package owner")
            }
            Self::UndeclaredModule { package, module } => write!(
                formatter,
                "module `{module}` is not declared by package `{package}`"
            ),
            Self::RootSourceOwnedByAnotherPackage {
                root,
                expected,
                actual,
            } => write!(
                formatter,
                "root source {root} belongs to `{actual}`, not root package `{expected}`"
            ),
            Self::Source(error) => error.fmt(formatter),
        }
    }
}

impl Error for PackageGraphError {}

impl From<SourceError> for PackageGraphError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

#[derive(Debug, Clone)]
pub struct PackageNode {
    id: PackageId,
    source_id: SourceId,
    local_name: PackageAlias,
    edition: Edition,
    modules: BTreeSet<ModulePath>,
    dependencies: BTreeMap<PackageAlias, PackageId>,
}

impl PackageNode {
    pub fn new(
        id: PackageId,
        source_id: SourceId,
        local_name: PackageAlias,
        edition: Edition,
        modules: impl IntoIterator<Item = ModulePath>,
        dependencies: impl IntoIterator<Item = (PackageAlias, PackageId)>,
    ) -> Result<Self, PackageGraphError> {
        let modules = modules.into_iter().collect();
        let mut dependency_map = BTreeMap::new();
        for (alias, dependency) in dependencies {
            if alias == local_name {
                return Err(PackageGraphError::AliasMatchesCurrentPackage(alias));
            }
            if dependency_map.insert(alias.clone(), dependency).is_some() {
                return Err(PackageGraphError::DuplicateDependencyAlias(alias));
            }
        }
        Ok(Self {
            id,
            source_id,
            local_name,
            edition,
            modules,
            dependencies: dependency_map,
        })
    }

    pub fn id(&self) -> &PackageId {
        &self.id
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn local_name(&self) -> &PackageAlias {
        &self.local_name
    }

    pub fn edition(&self) -> Edition {
        self.edition
    }

    pub fn modules(&self) -> &BTreeSet<ModulePath> {
        &self.modules
    }

    pub fn dependencies(&self) -> &BTreeMap<PackageAlias, PackageId> {
        &self.dependencies
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleId {
    package: PackageId,
    path: ModulePath,
}

impl ModuleId {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn path(&self) -> &ModulePath {
        &self.path
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.package, self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportResolutionError {
    EmptyPath,
    UnknownPackageAlias(Name),
    MissingModulePath(Name),
    UnknownModule(ModuleId),
    MissingTargetCapability {
        module: ModuleId,
        capability: &'static str,
    },
    UnknownFromPackage(PackageId),
}

impl fmt::Display for ImportResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("an import path cannot be empty"),
            Self::UnknownPackageAlias(alias) => {
                write!(formatter, "unknown package alias `{alias}`")
            }
            Self::MissingModulePath(alias) => {
                write!(formatter, "package alias `{alias}` is not a module path")
            }
            Self::UnknownModule(module) => write!(formatter, "unknown module `{module}`"),
            Self::MissingTargetCapability { module, capability } => write!(
                formatter,
                "module `{module}` requires missing target capability `{capability}`"
            ),
            Self::UnknownFromPackage(package) => {
                write!(formatter, "unknown importing package `{package}`")
            }
        }
    }
}

impl Error for ImportResolutionError {}

#[derive(Debug, Clone)]
pub struct PackageGraph {
    root: PackageId,
    standard: PackageId,
    packages: BTreeMap<PackageId, PackageNode>,
    by_source: BTreeMap<SourceId, PackageId>,
}

impl PackageGraph {
    pub fn new(
        root: PackageId,
        standard: PackageId,
        nodes: impl IntoIterator<Item = PackageNode>,
    ) -> Result<Self, PackageGraphError> {
        if root == standard {
            return Err(PackageGraphError::RootIsStandardPackage);
        }
        let mut packages = BTreeMap::new();
        let mut by_source = BTreeMap::new();
        for node in nodes {
            let id = node.id.clone();
            if by_source
                .insert(node.source_id.clone(), node.id.clone())
                .is_some()
            {
                return Err(PackageGraphError::DuplicateSourceId(node.source_id.clone()));
            }
            if packages.insert(id.clone(), node).is_some() {
                return Err(PackageGraphError::DuplicatePackage(id));
            }
        }
        if !packages.contains_key(&root) {
            return Err(PackageGraphError::UnknownRootPackage(root));
        }
        if !packages.contains_key(&standard) {
            return Err(PackageGraphError::UnknownStandardPackage(standard));
        }
        for node in packages.values() {
            for dependency in node.dependencies.values() {
                if !packages.contains_key(dependency) {
                    return Err(PackageGraphError::UnknownDependency {
                        package: node.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
        if has_dependency_cycle(&packages) {
            return Err(PackageGraphError::DependencyCycle);
        }
        Ok(Self {
            root,
            standard,
            packages,
            by_source,
        })
    }

    pub fn loose(sources: &SourceDatabase, root: FileId) -> Result<Self, PackageGraphError> {
        let root_source = sources.get(root)?.source_id().clone();
        let mut modules_by_source = BTreeMap::<SourceId, BTreeSet<ModulePath>>::new();
        for (_, source) in sources.iter() {
            modules_by_source
                .entry(source.source_id().clone())
                .or_default()
                .insert(source.module().clone());
        }

        let root_id = loose_package_id("root", &root_source)?;
        let standard_id = PackageId::new("toolchain:std:0.1-bootstrap")?;
        let mut nodes = Vec::with_capacity(modules_by_source.len().saturating_add(1));
        for (index, (source_id, modules)) in modules_by_source.into_iter().enumerate() {
            let is_root = source_id == root_source;
            let id = if is_root {
                root_id.clone()
            } else {
                loose_package_id("source", &source_id)?
            };
            let local_name = if is_root {
                PackageAlias::new("main")?
            } else {
                PackageAlias::new(format!("dependency{index}"))?
            };
            nodes.push(PackageNode::new(
                id,
                source_id,
                local_name,
                Edition::V0_1,
                modules,
                [],
            )?);
        }
        nodes.push(PackageNode::new(
            standard_id.clone(),
            SourceId::new("toolchain:std:0.1-bootstrap")?,
            PackageAlias::new("tondoStd")?,
            Edition::V0_1,
            [ModulePath::new("console")?],
            [],
        )?);
        Self::new(root_id, standard_id, nodes)
    }

    pub fn root(&self) -> &PackageId {
        &self.root
    }

    pub fn standard(&self) -> &PackageId {
        &self.standard
    }

    pub(crate) fn select_bootstrap_standard_modules(
        &mut self,
        has_capability: impl Fn(&str) -> bool,
    ) {
        if self.standard.as_str() != "toolchain:std:0.1-bootstrap" {
            return;
        }
        let standard = self
            .packages
            .get_mut(&self.standard)
            .expect("the standard package was validated during graph construction");
        standard.modules.retain(|module| match module.as_str() {
            "console" => has_capability("console"),
            _ => false,
        });
    }

    pub fn packages(&self) -> impl ExactSizeIterator<Item = &PackageNode> {
        self.packages.values()
    }

    pub fn package(&self, id: &PackageId) -> Option<&PackageNode> {
        self.packages.get(id)
    }

    pub fn package_for_source(&self, source: &SourceId) -> Option<&PackageNode> {
        self.by_source
            .get(source)
            .and_then(|package| self.packages.get(package))
    }

    pub fn module(&self, package: &PackageId, path: &ModulePath) -> Option<ModuleId> {
        self.packages
            .get(package)
            .is_some_and(|node| node.modules.contains(path))
            .then(|| ModuleId {
                package: package.clone(),
                path: path.clone(),
            })
    }

    pub fn module_for_file(
        &self,
        sources: &SourceDatabase,
        file: FileId,
    ) -> Result<ModuleId, PackageGraphError> {
        let source = sources.get(file)?;
        let package = self
            .by_source
            .get(source.source_id())
            .ok_or_else(|| PackageGraphError::UnknownSourceId(source.source_id().clone()))?;
        self.module(package, source.module())
            .ok_or_else(|| PackageGraphError::UndeclaredModule {
                package: package.clone(),
                module: source.module().clone(),
            })
    }

    pub fn validate_sources(
        &self,
        sources: &SourceDatabase,
        root: FileId,
    ) -> Result<(), PackageGraphError> {
        for (file, _) in sources.iter() {
            self.module_for_file(sources, file)?;
        }
        let actual = self.module_for_file(sources, root)?.package;
        if actual != self.root {
            return Err(PackageGraphError::RootSourceOwnedByAnotherPackage {
                root,
                expected: self.root.clone(),
                actual,
            });
        }
        Ok(())
    }

    pub fn resolve_import(
        &self,
        from: &PackageId,
        segments: &[Name],
    ) -> Result<ModuleId, ImportResolutionError> {
        let Some(first) = segments.first() else {
            return Err(ImportResolutionError::EmptyPath);
        };
        if segments.len() == 1 {
            return Err(ImportResolutionError::MissingModulePath(first.clone()));
        }
        let from_node = self
            .packages
            .get(from)
            .ok_or_else(|| ImportResolutionError::UnknownFromPackage(from.clone()))?;
        let package = if first.as_str() == "std" {
            &self.standard
        } else if first.as_str() == from_node.local_name.as_str() {
            from
        } else {
            from_node
                .dependencies
                .iter()
                .find_map(|(alias, package)| (alias.as_str() == first.as_str()).then_some(package))
                .ok_or_else(|| ImportResolutionError::UnknownPackageAlias(first.clone()))?
        };
        let module_path = ModulePath::new(
            segments[1..]
                .iter()
                .map(Name::as_str)
                .collect::<Vec<_>>()
                .join("."),
        )
        .expect("validated names form a valid module path");
        self.module(package, &module_path).ok_or_else(|| {
            let module = ModuleId {
                package: package.clone(),
                path: module_path,
            };
            if package == &self.standard
                && self.standard.as_str() == "toolchain:std:0.1-bootstrap"
                && module.path().as_str() == "console"
            {
                ImportResolutionError::MissingTargetCapability {
                    module,
                    capability: "console",
                }
            } else {
                ImportResolutionError::UnknownModule(module)
            }
        })
    }

    pub fn symbol_identity(
        &self,
        module: ModuleId,
        namespace: Namespace,
        declaration: DeclarationPath,
    ) -> Result<SymbolIdentity, PackageGraphError> {
        let package = self
            .packages
            .get(module.package())
            .ok_or_else(|| PackageGraphError::UnknownRootPackage(module.package().clone()))?;
        if !package.modules.contains(module.path()) {
            return Err(PackageGraphError::UndeclaredModule {
                package: module.package().clone(),
                module: module.path().clone(),
            });
        }
        Ok(SymbolIdentity {
            package: module.package,
            source_id: package.source_id.clone(),
            module: module.path,
            namespace,
            declaration,
        })
    }
}

fn loose_package_id(kind: &str, source: &SourceId) -> Result<PackageId, PackageGraphError> {
    PackageId::new(format!(
        "loose:{kind}:{}:{}",
        source.as_str().len(),
        source.as_str()
    ))
}

fn has_dependency_cycle(packages: &BTreeMap<PackageId, PackageNode>) -> bool {
    let mut incoming = packages
        .keys()
        .cloned()
        .map(|package| (package, 0_usize))
        .collect::<BTreeMap<_, _>>();
    for node in packages.values() {
        for dependency in node.dependencies.values() {
            *incoming
                .get_mut(dependency)
                .expect("all dependencies were validated") += 1;
        }
    }
    let mut ready = incoming
        .iter()
        .filter_map(|(package, count)| (*count == 0).then_some(package.clone()))
        .collect::<VecDeque<_>>();
    let mut visited = 0_usize;
    while let Some(package) = ready.pop_front() {
        visited += 1;
        for dependency in packages[&package].dependencies.values() {
            let count = incoming
                .get_mut(dependency)
                .expect("all dependencies were validated");
            *count -= 1;
            if *count == 0 {
                ready.push_back(dependency.clone());
            }
        }
    }
    visited != packages.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Namespace {
    Type,
    Value,
    Module,
}

impl Namespace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Value => "value",
            Self::Module => "module",
        }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeclarationPath(Vec<Name>);

impl DeclarationPath {
    pub fn new(names: impl IntoIterator<Item = Name>) -> Result<Self, NameError> {
        let names = names.into_iter().collect::<Vec<_>>();
        if names.is_empty() {
            return Err(NameError::Empty);
        }
        Ok(Self(names))
    }

    pub fn single(name: Name) -> Self {
        Self(vec![name])
    }

    pub fn names(&self) -> &[Name] {
        &self.0
    }
}

impl fmt::Display for DeclarationPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, name) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(".")?;
            }
            name.fmt(formatter)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolIdentity {
    package: PackageId,
    source_id: SourceId,
    module: ModulePath,
    namespace: Namespace,
    declaration: DeclarationPath,
}

impl SymbolIdentity {
    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub fn declaration(&self) -> &DeclarationPath {
        &self.declaration
    }

    pub fn canonical_name(&self) -> String {
        format!(
            "@{}:{}::{}::{}::{}",
            self.source_id.as_str().len(),
            self.source_id,
            self.module,
            self.namespace,
            self.declaration
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{LogicalPath, SourceInput};

    fn node(
        id: &str,
        source: &str,
        local_name: &str,
        modules: &[&str],
        dependencies: &[(&str, &str)],
    ) -> PackageNode {
        PackageNode::new(
            PackageId::new(id).unwrap(),
            SourceId::new(source).unwrap(),
            PackageAlias::new(local_name).unwrap(),
            Edition::V0_1,
            modules
                .iter()
                .map(|module| ModulePath::new(module).unwrap()),
            dependencies.iter().map(|(alias, package)| {
                (
                    PackageAlias::new(alias).unwrap(),
                    PackageId::new(*package).unwrap(),
                )
            }),
        )
        .unwrap()
    }

    #[test]
    fn graph_resolves_only_declared_exact_package_aliases_and_modules() {
        let graph = PackageGraph::new(
            PackageId::new("pkg:app@1").unwrap(),
            PackageId::new("pkg:std@1").unwrap(),
            [
                node(
                    "pkg:app@1",
                    "source:app@1",
                    "app",
                    &["main", "models"],
                    &[("users", "pkg:users@2")],
                ),
                node(
                    "pkg:users@2",
                    "source:users@2",
                    "usersPackage",
                    &["api"],
                    &[],
                ),
                node("pkg:std@1", "source:std@1", "tondoStd", &["fs"], &[]),
            ],
        )
        .unwrap();

        let users = [Name::new("users").unwrap(), Name::new("api").unwrap()];
        assert_eq!(
            graph
                .resolve_import(&PackageId::new("pkg:app@1").unwrap(), &users)
                .unwrap()
                .to_string(),
            "pkg:users@2::api"
        );
        let standard = [Name::new("std").unwrap(), Name::new("fs").unwrap()];
        assert_eq!(
            graph
                .resolve_import(&PackageId::new("pkg:app@1").unwrap(), &standard)
                .unwrap()
                .to_string(),
            "pkg:std@1::fs"
        );
        let missing = [Name::new("users").unwrap(), Name::new("missing").unwrap()];
        assert!(matches!(
            graph.resolve_import(&PackageId::new("pkg:app@1").unwrap(), &missing),
            Err(ImportResolutionError::UnknownModule(_))
        ));
    }

    #[test]
    fn nominal_identity_includes_package_module_namespace_and_path() {
        let graph = PackageGraph::new(
            PackageId::new("pkg:app@1").unwrap(),
            PackageId::new("pkg:std@1").unwrap(),
            [
                node("pkg:app@1", "pkg:app@1", "app", &["models"], &[]),
                node("pkg:std@1", "pkg:std@1", "tondoStd", &[], &[]),
            ],
        )
        .unwrap();
        let module = graph
            .module(
                &PackageId::new("pkg:app@1").unwrap(),
                &ModulePath::new("models").unwrap(),
            )
            .unwrap();
        let identity = graph
            .symbol_identity(
                module,
                Namespace::Type,
                DeclarationPath::single(Name::new("User").unwrap()),
            )
            .unwrap();

        assert_eq!(identity.package().as_str(), "pkg:app@1");
        assert_eq!(
            identity.canonical_name(),
            "@9:pkg:app@1::models::type::User"
        );
    }

    #[test]
    fn invalid_aliases_and_non_closed_graphs_are_rejected() {
        assert!(matches!(
            PackageAlias::new("std"),
            Err(PackageGraphError::ReservedStandardAlias)
        ));
        assert!(PackageAlias::new("import").is_err());
        let result = PackageGraph::new(
            PackageId::new("pkg:app").unwrap(),
            PackageId::new("pkg:std").unwrap(),
            [
                node(
                    "pkg:app",
                    "source:app",
                    "app",
                    &["main"],
                    &[("missing", "pkg:missing")],
                ),
                node("pkg:std", "source:std", "tondoStd", &[], &[]),
            ],
        );
        assert!(matches!(
            result,
            Err(PackageGraphError::UnknownDependency { .. })
        ));
    }

    #[test]
    fn loose_graph_owns_every_source_and_the_declared_root() {
        let mut sources = SourceDatabase::new();
        let root = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("main").unwrap(),
                LogicalPath::new("main.to").unwrap(),
                Arc::<[u8]>::from(&b"fn main() {}\n"[..]),
            ))
            .unwrap();
        sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:test").unwrap(),
                ModulePath::new("helpers").unwrap(),
                LogicalPath::new("helpers.to").unwrap(),
                Arc::<[u8]>::from(&b"fn helper() {}\n"[..]),
            ))
            .unwrap();

        let graph = PackageGraph::loose(&sources, root).unwrap();
        graph.validate_sources(&sources, root).unwrap();
        assert_eq!(graph.packages().count(), 2);
        assert_eq!(
            graph.module_for_file(&sources, root).unwrap().package(),
            graph.root()
        );
    }
}
