//! Isolated integration-test consumers.
//!
//! An integration source is not another module of the package under test.  It
//! is a small, deterministic consumer package whose only imported surfaces are
//! the public interfaces named by the test plan.  This module closes that
//! boundary before body checking or lowering starts; it never reads source
//! bytes, opens a package, or grants a friend/private capability.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::artifact::sha256;
use crate::package::{Name, Namespace, PackageAlias, PackageId};
use crate::resolve::{SymbolKind, Visibility};
use crate::source::ModulePath;
use crate::test_dependencies::TestDependencyGraph;
use crate::test_plan::TestSourceClass;

pub const INTEGRATION_ROOT_FORMAT: &str = "tondo-integration-root-draft/1";

/// A declaration in a supplied interface or in the private helper namespace
/// of an integration source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntegrationDeclaration {
    namespace: Namespace,
    name: Name,
    kind: SymbolKind,
    visibility: Visibility,
}

impl IntegrationDeclaration {
    pub fn new(namespace: Namespace, name: Name, kind: SymbolKind, visibility: Visibility) -> Self {
        Self {
            namespace,
            name,
            kind,
            visibility,
        }
    }

    pub fn public(namespace: Namespace, name: Name, kind: SymbolKind) -> Self {
        Self::new(namespace, name, kind, Visibility::Public)
    }

    pub fn private(namespace: Namespace, name: Name, kind: SymbolKind) -> Self {
        Self::new(namespace, name, kind, Visibility::Private)
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    fn key(&self) -> DeclarationKey {
        DeclarationKey {
            namespace: self.namespace,
            name: self.name.clone(),
        }
    }
}

/// A source-level import together with the interface declarations supplied by
/// the already verified package artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationImport {
    alias: PackageAlias,
    package: PackageId,
    module: ModulePath,
    declarations: Vec<IntegrationDeclaration>,
}

impl IntegrationImport {
    pub fn new(
        alias: PackageAlias,
        package: PackageId,
        module: ModulePath,
        declarations: impl IntoIterator<Item = IntegrationDeclaration>,
    ) -> Self {
        let mut declarations = declarations.into_iter().collect::<Vec<_>>();
        declarations.sort();
        Self {
            alias,
            package,
            module,
            declarations,
        }
    }

    pub fn alias(&self) -> &PackageAlias {
        &self.alias
    }

    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn declarations(&self) -> &[IntegrationDeclaration] {
        &self.declarations
    }
}

/// A name use from an integration body.  The target is an import alias, never
/// an implicit path into the package under test.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IntegrationReference {
    alias: PackageAlias,
    namespace: Namespace,
    name: Name,
}

impl IntegrationReference {
    pub fn new(alias: PackageAlias, namespace: Namespace, name: Name) -> Self {
        Self {
            alias,
            namespace,
            name,
        }
    }

    pub fn alias(&self) -> &PackageAlias {
        &self.alias
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub fn name(&self) -> &Name {
        &self.name
    }
}

/// Unchecked metadata for one source under `tests/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRootInput {
    tested_package: PackageId,
    logical_path: String,
    module: ModulePath,
    source_class: TestSourceClass,
    imports: Vec<IntegrationImport>,
    declarations: Vec<IntegrationDeclaration>,
    references: Vec<IntegrationReference>,
}

impl IntegrationRootInput {
    pub fn new(
        tested_package: PackageId,
        logical_path: impl Into<String>,
        module: ModulePath,
    ) -> Self {
        Self {
            tested_package,
            logical_path: logical_path.into(),
            module,
            source_class: TestSourceClass::IntegrationTest,
            imports: Vec::new(),
            declarations: Vec::new(),
            references: Vec::new(),
        }
    }

    pub fn with_source_class(mut self, source_class: TestSourceClass) -> Self {
        self.source_class = source_class;
        self
    }

    pub fn with_imports(mut self, imports: impl IntoIterator<Item = IntegrationImport>) -> Self {
        self.imports = imports.into_iter().collect();
        self
    }

    pub fn with_declarations(
        mut self,
        declarations: impl IntoIterator<Item = IntegrationDeclaration>,
    ) -> Self {
        self.declarations = declarations.into_iter().collect();
        self
    }

    pub fn with_references(
        mut self,
        references: impl IntoIterator<Item = IntegrationReference>,
    ) -> Self {
        self.references = references.into_iter().collect();
        self
    }

    pub fn tested_package(&self) -> &PackageId {
        &self.tested_package
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn source_class(&self) -> TestSourceClass {
        self.source_class
    }

    pub fn imports(&self) -> &[IntegrationImport] {
        &self.imports
    }

    pub fn declarations(&self) -> &[IntegrationDeclaration] {
        &self.declarations
    }

    pub fn references(&self) -> &[IntegrationReference] {
        &self.references
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct DeclarationKey {
    namespace: Namespace,
    name: Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedIntegrationReference {
    reference: IntegrationReference,
    package: PackageId,
    module: ModulePath,
}

impl ResolvedIntegrationReference {
    pub fn reference(&self) -> &IntegrationReference {
        &self.reference
    }

    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }
}

/// A sealed consumer root.  The synthetic package is intentionally retained
/// in the semantic identity while the visible IDs continue to use the tested
/// package name, as required by the test runner contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRoot {
    consumer_package: PackageId,
    tested_package: PackageId,
    logical_path: String,
    module: ModulePath,
    imports: Vec<IntegrationImport>,
    private_declarations: Vec<IntegrationDeclaration>,
    references: Vec<ResolvedIntegrationReference>,
}

impl IntegrationRoot {
    pub fn consumer_package(&self) -> &PackageId {
        &self.consumer_package
    }

    pub fn tested_package(&self) -> &PackageId {
        &self.tested_package
    }

    pub fn logical_path(&self) -> &str {
        &self.logical_path
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }

    pub fn imports(&self) -> &[IntegrationImport] {
        &self.imports
    }

    pub fn private_declarations(&self) -> &[IntegrationDeclaration] {
        &self.private_declarations
    }

    pub fn references(&self) -> &[ResolvedIntegrationReference] {
        &self.references
    }

    /// Visible IDs intentionally retain the package name while semantic
    /// resolution uses [`consumer_package`](Self::consumer_package).
    pub fn visible_prefix(&self) -> String {
        let relative = self
            .logical_path
            .strip_prefix("tests/")
            .expect("validated integration path has a tests/ prefix")
            .strip_suffix(".to")
            .expect("validated integration path has a .to suffix")
            .replace('/', ".");
        format!("{}::integration::{relative}", self.tested_package)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrationError {
    InvalidSourceClass(TestSourceClass),
    InvalidPath(String),
    EmptyPackage,
    PackageMismatch {
        expected: PackageId,
        actual: PackageId,
    },
    DuplicateImport(String),
    UnknownImport(String),
    UnknownPackage(PackageId),
    SelfImport,
    PrivateImport {
        alias: String,
        namespace: Namespace,
        name: Name,
    },
    DuplicateDeclaration {
        namespace: Namespace,
        name: Name,
    },
    PublicDeclaration(Name),
    MissingReference {
        alias: String,
        namespace: Namespace,
        name: Name,
    },
    DuplicateRoot(String),
}

impl fmt::Display for IntegrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSourceClass(class) => write!(
                formatter,
                "integration root requires integration-test source class, got `{}`",
                class.as_str()
            ),
            Self::InvalidPath(path) => write!(
                formatter,
                "integration root path `{path}` is not a canonical tests/*.to path"
            ),
            Self::EmptyPackage => formatter.write_str("integration root package cannot be empty"),
            Self::PackageMismatch { expected, actual } => write!(
                formatter,
                "integration root targets `{actual}`, expected `{expected}`"
            ),
            Self::DuplicateImport(alias) => {
                write!(
                    formatter,
                    "integration import alias `{alias}` is declared more than once"
                )
            }
            Self::UnknownImport(alias) => {
                write!(
                    formatter,
                    "integration reference uses unknown import `{alias}`"
                )
            }
            Self::UnknownPackage(package) => write!(
                formatter,
                "integration import package `{package}` is not the tested package or a declared test dependency"
            ),
            Self::SelfImport => formatter
                .write_str("an integration root cannot import its synthetic consumer package"),
            Self::PrivateImport {
                alias,
                namespace,
                name,
            } => write!(
                formatter,
                "integration root cannot access private `{namespace}::{name}` from `{alias}`"
            ),
            Self::DuplicateDeclaration { namespace, name } => write!(
                formatter,
                "integration helper `{namespace}::{name}` is declared more than once"
            ),
            Self::PublicDeclaration(name) => write!(
                formatter,
                "integration root declaration `{name}` cannot publish an interface"
            ),
            Self::MissingReference {
                alias,
                namespace,
                name,
            } => write!(
                formatter,
                "integration import `{alias}` does not export `{namespace}::{name}`"
            ),
            Self::DuplicateRoot(path) => {
                write!(
                    formatter,
                    "integration root `{path}` is registered more than once"
                )
            }
        }
    }
}

impl Error for IntegrationError {}

/// Build one isolated integration consumer.
pub fn build(
    expected_tested_package: &PackageId,
    input: IntegrationRootInput,
    allowed_test_dependencies: impl IntoIterator<Item = PackageId>,
) -> Result<IntegrationRoot, IntegrationError> {
    if input.source_class != TestSourceClass::IntegrationTest {
        return Err(IntegrationError::InvalidSourceClass(input.source_class));
    }
    if input.tested_package.as_str().is_empty() {
        return Err(IntegrationError::EmptyPackage);
    }
    if &input.tested_package != expected_tested_package {
        return Err(IntegrationError::PackageMismatch {
            expected: expected_tested_package.clone(),
            actual: input.tested_package,
        });
    }
    validate_integration_path(&input.logical_path)?;

    let consumer_package = synthetic_package(&input.tested_package, &input.logical_path);
    let mut allowed = allowed_test_dependencies
        .into_iter()
        .collect::<BTreeSet<_>>();
    allowed.remove(expected_tested_package);
    if allowed.contains(&consumer_package) {
        return Err(IntegrationError::UnknownPackage(consumer_package));
    }

    let mut imports = input.imports;
    imports.sort_by(|left, right| left.alias().cmp(right.alias()));
    let mut aliases = BTreeSet::new();
    let mut import_map = BTreeMap::<PackageAlias, IntegrationImport>::new();
    for import in imports {
        if !aliases.insert(import.alias().clone()) {
            return Err(IntegrationError::DuplicateImport(
                import.alias().to_string(),
            ));
        }
        if import.package() == &consumer_package {
            return Err(IntegrationError::SelfImport);
        }
        if import.package() != expected_tested_package && !allowed.contains(import.package()) {
            return Err(IntegrationError::UnknownPackage(import.package().clone()));
        }
        let mut declarations = BTreeMap::<DeclarationKey, IntegrationDeclaration>::new();
        for declaration in import.declarations() {
            let key = declaration.key();
            if declarations.insert(key, declaration.clone()).is_some() {
                return Err(IntegrationError::MissingReference {
                    alias: import.alias().to_string(),
                    namespace: declaration.namespace(),
                    name: declaration.name().clone(),
                });
            }
            if declaration.visibility() != Visibility::Public {
                return Err(IntegrationError::PrivateImport {
                    alias: import.alias().to_string(),
                    namespace: declaration.namespace(),
                    name: declaration.name().clone(),
                });
            }
        }
        import_map.insert(import.alias().clone(), import);
    }

    let mut private_declarations = input.declarations;
    private_declarations.sort();
    let mut helper_names = BTreeSet::new();
    for declaration in &private_declarations {
        if declaration.visibility() != Visibility::Private {
            return Err(IntegrationError::PublicDeclaration(
                declaration.name().clone(),
            ));
        }
        if !helper_names.insert(declaration.key()) {
            return Err(IntegrationError::DuplicateDeclaration {
                namespace: declaration.namespace(),
                name: declaration.name().clone(),
            });
        }
    }

    let mut references = input.references;
    references.sort();
    let mut resolved = Vec::with_capacity(references.len());
    for reference in references {
        let Some(import) = import_map.get(reference.alias()) else {
            return Err(IntegrationError::UnknownImport(
                reference.alias().to_string(),
            ));
        };
        let Some(declaration) = import.declarations().iter().find(|declaration| {
            declaration.namespace() == reference.namespace()
                && declaration.name() == reference.name()
        }) else {
            return Err(IntegrationError::MissingReference {
                alias: reference.alias().to_string(),
                namespace: reference.namespace(),
                name: reference.name().clone(),
            });
        };
        if declaration.visibility() != Visibility::Public {
            return Err(IntegrationError::PrivateImport {
                alias: reference.alias().to_string(),
                namespace: reference.namespace(),
                name: reference.name().clone(),
            });
        }
        resolved.push(ResolvedIntegrationReference {
            reference,
            package: import.package().clone(),
            module: import.module().clone(),
        });
    }

    Ok(IntegrationRoot {
        consumer_package,
        tested_package: input.tested_package,
        logical_path: input.logical_path,
        module: input.module,
        imports: import_map.into_values().collect(),
        private_declarations,
        references: resolved,
    })
}

/// Build every integration root in canonical path order and reject duplicate
/// roots before any caller can accidentally share a module scope.
pub fn build_many(
    expected_tested_package: &PackageId,
    inputs: impl IntoIterator<Item = IntegrationRootInput>,
    allowed_test_dependencies: impl IntoIterator<Item = PackageId> + Clone,
) -> Result<Vec<IntegrationRoot>, IntegrationError> {
    let mut inputs = inputs.into_iter().collect::<Vec<_>>();
    inputs.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    let mut paths = BTreeSet::new();
    let mut roots = Vec::with_capacity(inputs.len());
    for input in inputs {
        if !paths.insert(input.logical_path.clone()) {
            return Err(IntegrationError::DuplicateRoot(input.logical_path));
        }
        roots.push(build(
            expected_tested_package,
            input,
            allowed_test_dependencies.clone(),
        )?);
    }
    Ok(roots)
}

/// Convenience adapter from the closed test dependency graph.  The graph is
/// read-only and only its package identities are admitted; interface members
/// still have to be supplied explicitly by each import.
pub fn build_with_graph(
    expected_tested_package: &PackageId,
    input: IntegrationRootInput,
    graph: &TestDependencyGraph,
) -> Result<IntegrationRoot, IntegrationError> {
    build(
        expected_tested_package,
        input,
        graph.packages().map(|node| node.package().clone()),
    )
}

fn synthetic_package(tested_package: &PackageId, logical_path: &str) -> PackageId {
    let mut identity = Vec::new();
    append_field(&mut identity, tested_package.as_str());
    append_field(&mut identity, logical_path);
    let digest = sha256(&identity);
    PackageId::new(format!("test:integration:{digest}")).expect("synthetic package is line-free")
}

fn append_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.len().to_string().as_bytes());
    output.push(b':');
    output.extend_from_slice(value.as_bytes());
    output.push(b'|');
}

fn validate_integration_path(path: &str) -> Result<(), IntegrationError> {
    if !path.starts_with("tests/")
        || !path.ends_with(".to")
        || path.len() <= "tests/.to".len()
        || path.starts_with('/')
        || path.contains(['\\', '\n', '\r'])
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(IntegrationError::InvalidPath(path.to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package() -> PackageId {
        PackageId::new("workspace:application@1").unwrap()
    }

    fn dep() -> PackageId {
        PackageId::new("registry:assertions@1#abc").unwrap()
    }

    fn alias(value: &str) -> PackageAlias {
        PackageAlias::new(value).unwrap()
    }

    fn name(value: &str) -> Name {
        Name::new(value).unwrap()
    }

    fn module(value: &str) -> ModulePath {
        ModulePath::new(value).unwrap()
    }

    fn public_import(alias_value: &str, package: PackageId) -> IntegrationImport {
        IntegrationImport::new(
            alias(alias_value),
            package,
            module("api"),
            [IntegrationDeclaration::public(
                Namespace::Value,
                name("add"),
                SymbolKind::Function,
            )],
        )
    }

    fn input(path: &str) -> IntegrationRootInput {
        IntegrationRootInput::new(package(), path, module("tests.root"))
            .with_imports([
                public_import("application", package()),
                public_import("assertions", dep()),
            ])
            .with_declarations([IntegrationDeclaration::private(
                Namespace::Value,
                name("helper"),
                SymbolKind::Function,
            )])
            .with_references([IntegrationReference::new(
                alias("application"),
                Namespace::Value,
                name("add"),
            )])
    }

    #[test]
    fn builds_stable_consumer_and_visible_identity() {
        let root = build(&package(), input("tests/http/client.to"), [dep()]).unwrap();
        assert_ne!(root.consumer_package(), root.tested_package());
        assert!(
            root.consumer_package()
                .as_str()
                .starts_with("test:integration:sha256:")
        );
        assert_eq!(
            root.visible_prefix(),
            "workspace:application@1::integration::http.client"
        );
        assert_eq!(root.imports().len(), 2);
        assert_eq!(root.private_declarations()[0].name().as_str(), "helper");
        assert_eq!(root.references()[0].package(), &package());
    }

    #[test]
    fn synthetic_identity_depends_on_package_and_path_only() {
        let a = build(&package(), input("tests/a.to"), [dep()]).unwrap();
        let b = build(&package(), input("tests/b.to"), [dep()]).unwrap();
        let a_again = build(&package(), input("tests/a.to"), [dep()]).unwrap();
        assert_ne!(a.consumer_package(), b.consumer_package());
        assert_eq!(a.consumer_package(), a_again.consumer_package());
    }

    #[test]
    fn rejects_non_integration_source_and_bad_paths() {
        let error = build(
            &package(),
            input("tests/a.to").with_source_class(TestSourceClass::UnitTest),
            [dep()],
        )
        .unwrap_err();
        assert!(matches!(error, IntegrationError::InvalidSourceClass(_)));
        for path in [
            "src/a.to",
            "tests/a",
            "tests//a.to",
            "tests/../a.to",
            "tests\\a.to",
            "tests/.to",
            "tests/a.to\n",
        ] {
            assert!(matches!(
                build(&package(), input(path), [dep()]),
                Err(IntegrationError::InvalidPath(_))
            ));
        }
    }

    #[test]
    fn rejects_package_mismatch_and_unknown_or_self_imports() {
        let other = PackageId::new("workspace:other@1").unwrap();
        assert!(matches!(
            build(&other, input("tests/a.to"), [dep()]),
            Err(IntegrationError::PackageMismatch { .. })
        ));
        let unknown = PackageId::new("registry:unknown@1#x").unwrap();
        let unknown_input = input("tests/a.to").with_imports([public_import("unknown", unknown)]);
        assert!(matches!(
            build(&package(), unknown_input, [dep()]),
            Err(IntegrationError::UnknownPackage(_))
        ));
        let consumer = synthetic_package(&package(), "tests/a.to");
        let self_input = input("tests/a.to").with_imports([public_import("consumer", consumer)]);
        assert!(matches!(
            build(&package(), self_input, [dep()]),
            Err(IntegrationError::SelfImport)
        ));
    }

    #[test]
    fn rejects_duplicate_imports_and_public_or_duplicate_helpers() {
        let duplicate_import = input("tests/a.to").with_imports([
            public_import("application", package()),
            public_import("application", package()),
        ]);
        assert!(matches!(
            build(&package(), duplicate_import, [dep()]),
            Err(IntegrationError::DuplicateImport(_))
        ));
        let public_helper =
            input("tests/a.to").with_declarations([IntegrationDeclaration::public(
                Namespace::Value,
                name("exported"),
                SymbolKind::Function,
            )]);
        assert!(matches!(
            build(&package(), public_helper, [dep()]),
            Err(IntegrationError::PublicDeclaration(_))
        ));
        let duplicate_helper = input("tests/a.to").with_declarations([
            IntegrationDeclaration::private(Namespace::Value, name("helper"), SymbolKind::Function),
            IntegrationDeclaration::private(Namespace::Value, name("helper"), SymbolKind::Function),
        ]);
        assert!(matches!(
            build(&package(), duplicate_helper, [dep()]),
            Err(IntegrationError::DuplicateDeclaration { .. })
        ));
    }

    #[test]
    fn rejects_private_exports_and_missing_references() {
        let private_import = input("tests/a.to").with_imports([IntegrationImport::new(
            alias("application"),
            package(),
            module("api"),
            [IntegrationDeclaration::private(
                Namespace::Value,
                name("secret"),
                SymbolKind::Function,
            )],
        )]);
        assert!(matches!(
            build(&package(), private_import, [dep()]),
            Err(IntegrationError::PrivateImport { .. })
        ));
        let missing = input("tests/a.to").with_references([IntegrationReference::new(
            alias("application"),
            Namespace::Value,
            name("missing"),
        )]);
        assert!(matches!(
            build(&package(), missing, [dep()]),
            Err(IntegrationError::MissingReference { .. })
        ));
        let unknown_alias = input("tests/a.to").with_references([IntegrationReference::new(
            alias("missing"),
            Namespace::Value,
            name("add"),
        )]);
        assert!(matches!(
            build(&package(), unknown_alias, [dep()]),
            Err(IntegrationError::UnknownImport(_))
        ));
    }

    #[test]
    fn rejects_duplicate_interface_members_and_preserves_public_only_surface() {
        let duplicate = input("tests/a.to").with_imports([IntegrationImport::new(
            alias("application"),
            package(),
            module("api"),
            [
                IntegrationDeclaration::public(Namespace::Value, name("add"), SymbolKind::Function),
                IntegrationDeclaration::public(Namespace::Value, name("add"), SymbolKind::Function),
            ],
        )]);
        assert!(matches!(
            build(&package(), duplicate, [dep()]),
            Err(IntegrationError::MissingReference { .. })
        ));
        let only_dependency = input("tests/a.to")
            .with_imports([public_import("assertions", dep())])
            .with_references([]);
        let root = build(&package(), only_dependency, [dep()]).unwrap();
        assert_eq!(root.imports()[0].package(), &dep());
    }

    #[test]
    fn build_many_is_sorted_and_rejects_duplicate_paths() {
        let roots = build_many(
            &package(),
            [input("tests/z.to"), input("tests/a.to")],
            [dep()],
        )
        .unwrap();
        assert_eq!(roots[0].logical_path(), "tests/a.to");
        assert_eq!(roots[1].logical_path(), "tests/z.to");
        let duplicate = build_many(
            &package(),
            [input("tests/a.to"), input("tests/a.to")],
            [dep()],
        );
        assert!(matches!(duplicate, Err(IntegrationError::DuplicateRoot(_))));
    }
}
