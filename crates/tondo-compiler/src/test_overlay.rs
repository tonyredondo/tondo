//! Sealed unit-test overlays.
//!
//! A unit companion is checked only after its production module has been
//! resolved and checked.  This module models that boundary explicitly: a
//! [`ProductionSeal`] is an immutable semantic snapshot and [`build`] can
//! only validate the overlay against that snapshot.  It never receives source
//! text, a package graph or a resolver, so there is no path by which an
//! overlay can repair production or cause production bodies to be checked a
//! second time.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use crate::artifact::validate_sha256;
use crate::package::{Name, Namespace, PackageId};
use crate::resolve::{ResolvedProgram, Symbol, SymbolKind, Visibility};
use crate::source::{FileId, ModulePath, Span};
use crate::test_plan::TestSourceClass;
use crate::test_tree::StaticTestTree;

/// The hashes that identify the already-built production unit.
///
/// These are copied into the resulting overlay and are never recalculated
/// from test sources.  Keeping all four identities together makes it
/// impossible for a caller to compare only the public API while overlooking
/// capabilities, coherence, or the production artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionHashes {
    interface: String,
    capabilities: String,
    coherence: String,
    artifact: String,
}

impl ProductionHashes {
    pub fn new(
        interface: impl Into<String>,
        capabilities: impl Into<String>,
        coherence: impl Into<String>,
        artifact: impl Into<String>,
    ) -> Result<Self, OverlayError> {
        let hashes = Self {
            interface: interface.into(),
            capabilities: capabilities.into(),
            coherence: coherence.into(),
            artifact: artifact.into(),
        };
        for hash in hashes.iter() {
            validate_sha256(hash).map_err(|error| OverlayError::InvalidHash(error.to_string()))?;
        }
        Ok(hashes)
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }

    pub fn capabilities(&self) -> &str {
        &self.capabilities
    }

    pub fn coherence(&self) -> &str {
        &self.coherence
    }

    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    fn iter(&self) -> [&str; 4] {
        [
            &self.interface,
            &self.capabilities,
            &self.coherence,
            &self.artifact,
        ]
    }
}

/// Completion proof supplied by the production compiler phase.
///
/// A seal requires all three checks.  The booleans are intentionally kept in
/// the proof rather than inferred from the presence of symbols: an empty but
/// invalid production module must not become a valid unit overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionProof {
    resolution_complete: bool,
    semantic_check_complete: bool,
    coherence_check_complete: bool,
    hashes: ProductionHashes,
}

impl ProductionProof {
    pub fn new(
        resolution_complete: bool,
        semantic_check_complete: bool,
        coherence_check_complete: bool,
        hashes: ProductionHashes,
    ) -> Self {
        Self {
            resolution_complete,
            semantic_check_complete,
            coherence_check_complete,
            hashes,
        }
    }

    pub fn verified(hashes: ProductionHashes) -> Self {
        Self::new(true, true, true, hashes)
    }

    pub fn resolution_complete(&self) -> bool {
        self.resolution_complete
    }

    pub fn semantic_check_complete(&self) -> bool {
        self.semantic_check_complete
    }

    pub fn coherence_check_complete(&self) -> bool {
        self.coherence_check_complete
    }

    pub fn hashes(&self) -> &ProductionHashes {
        &self.hashes
    }

    fn is_complete(&self) -> bool {
        self.resolution_complete && self.semantic_check_complete && self.coherence_check_complete
    }
}

/// Stable package/module identity used by overlay imports and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKey {
    package: PackageId,
    module: ModulePath,
}

impl ModuleKey {
    pub fn new(package: PackageId, module: ModulePath) -> Self {
        Self { package, module }
    }

    pub fn package(&self) -> &PackageId {
        &self.package
    }

    pub fn module(&self) -> &ModulePath {
        &self.module
    }
}

impl fmt::Display for ModuleKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}::{}", self.package, self.module)
    }
}

/// A namespace/name key used by the ordinary resolver rules.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct DeclarationKey {
    namespace: Namespace,
    name: Name,
}

impl DeclarationKey {
    fn new(namespace: Namespace, name: Name) -> Self {
        Self { namespace, name }
    }
}

/// Metadata for one declaration visible in the sealed production module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionDeclaration {
    namespace: Namespace,
    name: Name,
    kind: SymbolKind,
    visibility: Visibility,
    span: Span,
    generic_arity: u32,
    synthetic: bool,
}

impl ProductionDeclaration {
    pub fn new(
        namespace: Namespace,
        name: Name,
        kind: SymbolKind,
        visibility: Visibility,
        span: Span,
        generic_arity: u32,
        synthetic: bool,
    ) -> Self {
        Self {
            namespace,
            name,
            kind,
            visibility,
            span,
            generic_arity,
            synthetic,
        }
    }

    fn from_symbol(symbol: &Symbol) -> Self {
        Self::new(
            symbol.identity().namespace(),
            symbol.name().clone(),
            symbol.kind(),
            symbol.visibility(),
            symbol.span(),
            symbol.generic_arity(),
            symbol.is_synthetic(),
        )
    }

    fn key(&self) -> DeclarationKey {
        DeclarationKey::new(self.namespace, self.name.clone())
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

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn generic_arity(&self) -> u32 {
        self.generic_arity
    }

    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }
}

/// The source identity from which a production seal was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionSeal {
    module: ModuleKey,
    production_files: BTreeSet<FileId>,
    declarations: BTreeMap<DeclarationKey, ProductionDeclaration>,
    proof: ProductionProof,
}

impl ProductionSeal {
    /// Seal declarations that have already passed production checking.
    pub fn new(
        module: ModuleKey,
        production_files: impl IntoIterator<Item = FileId>,
        declarations: impl IntoIterator<Item = ProductionDeclaration>,
        proof: ProductionProof,
    ) -> Result<Self, OverlayError> {
        if !proof.is_complete() {
            return Err(OverlayError::ProductionNotSealed);
        }
        let production_files = production_files.into_iter().collect::<BTreeSet<_>>();
        if production_files.is_empty() {
            return Err(OverlayError::InvalidInput(
                "a production seal requires at least one production source".into(),
            ));
        }
        let mut indexed = BTreeMap::new();
        for declaration in declarations {
            if !production_files.contains(&declaration.span().file()) {
                return Err(OverlayError::InvalidInput(
                    "production declaration does not belong to a sealed production source".into(),
                ));
            }
            let key = declaration.key();
            if indexed.insert(key, declaration).is_some() {
                return Err(OverlayError::ProductionDeclarationCollision);
            }
        }
        Ok(Self {
            module,
            production_files,
            declarations: indexed,
            proof,
        })
    }

    /// Adapt the resolver's already-built program into a seal.
    ///
    /// `production_files` is explicit so a caller cannot accidentally include
    /// a unit companion that happened to share the module path.  The resolver
    /// output is only read; this function does not resolve or check anything.
    pub fn from_resolved(
        module: ModuleKey,
        program: &ResolvedProgram,
        production_files: impl IntoIterator<Item = FileId>,
        proof: ProductionProof,
    ) -> Result<Self, OverlayError> {
        let production_files = production_files.into_iter().collect::<BTreeSet<_>>();
        if production_files.is_empty() {
            return Err(OverlayError::InvalidInput(
                "a production seal requires at least one production source".into(),
            ));
        }
        if production_files
            .iter()
            .any(|file| program.file(*file).is_none())
        {
            return Err(OverlayError::InvalidInput(
                "production seal names a source that is absent from the resolved program".into(),
            ));
        }
        let declarations = program
            .symbols()
            .filter(|symbol| {
                symbol.identity().package() == module.package()
                    && symbol.identity().module() == module.module()
                    && production_files.contains(&symbol.span().file())
            })
            .map(ProductionDeclaration::from_symbol)
            .collect::<Vec<_>>();
        Self::new(module, production_files, declarations, proof)
    }

    pub fn module(&self) -> &ModuleKey {
        &self.module
    }

    pub fn production_files(&self) -> &BTreeSet<FileId> {
        &self.production_files
    }

    pub fn declarations(&self) -> impl ExactSizeIterator<Item = &ProductionDeclaration> {
        self.declarations.values()
    }

    pub fn declaration(&self, namespace: Namespace, name: &Name) -> Option<&ProductionDeclaration> {
        self.declarations
            .get(&DeclarationKey::new(namespace, name.clone()))
    }

    pub fn proof(&self) -> &ProductionProof {
        &self.proof
    }
}

/// Kind of private declaration added by an overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OverlayDeclarationKind {
    PrivateConstant,
    PrivateFunction,
    PrivateType,
    PrivateAlias,
    PrivateEnum,
    PrivateTrait,
    /// A marker used by parser adapters for an `impl`/coherence declaration.
    /// Unit overlays reject it before it can affect production coherence.
    CoherenceImplementation,
}

impl OverlayDeclarationKind {
    fn namespace(self) -> Namespace {
        match self {
            Self::PrivateConstant | Self::PrivateFunction => Namespace::Value,
            Self::PrivateType | Self::PrivateAlias | Self::PrivateEnum | Self::PrivateTrait => {
                Namespace::Type
            }
            Self::CoherenceImplementation => Namespace::Type,
        }
    }
}

/// A declaration owned by the unit companion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayDeclaration {
    namespace: Namespace,
    name: Name,
    kind: OverlayDeclarationKind,
    visibility: Visibility,
    span: Span,
}

impl OverlayDeclaration {
    pub fn new(
        namespace: Namespace,
        name: Name,
        kind: OverlayDeclarationKind,
        visibility: Visibility,
        span: Span,
    ) -> Self {
        Self {
            namespace,
            name,
            kind,
            visibility,
            span,
        }
    }

    pub fn private_helper(name: Name, kind: OverlayDeclarationKind, span: Span) -> Self {
        Self::new(kind.namespace(), name, kind, Visibility::Private, span)
    }

    fn key(&self) -> DeclarationKey {
        DeclarationKey::new(self.namespace, self.name.clone())
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn kind(&self) -> OverlayDeclarationKind {
        self.kind
    }

    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

/// A public surface supplied by a compiled dependency interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayImport {
    alias: Name,
    target: ModuleKey,
    public_declarations: BTreeSet<DeclarationKey>,
    span: Span,
}

impl OverlayImport {
    pub fn new(
        alias: Name,
        target: ModuleKey,
        public_declarations: impl IntoIterator<Item = (Namespace, Name)>,
        span: Span,
    ) -> Self {
        Self {
            alias,
            target,
            public_declarations: public_declarations
                .into_iter()
                .map(|(namespace, name)| DeclarationKey::new(namespace, name))
                .collect(),
            span,
        }
    }

    pub fn alias(&self) -> &Name {
        &self.alias
    }

    pub fn target(&self) -> &ModuleKey {
        &self.target
    }

    pub fn span(&self) -> Span {
        self.span
    }

    pub fn exports(&self, namespace: Namespace, name: &Name) -> bool {
        self.public_declarations
            .contains(&DeclarationKey::new(namespace, name.clone()))
    }
}

/// Where a reference in the overlay expects to find a declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayReferenceTarget {
    Companion,
    Import(Name),
    Helper,
}

/// One semantic name use in an overlay body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayReference {
    target: OverlayReferenceTarget,
    namespace: Namespace,
    name: Name,
    span: Span,
}

impl OverlayReference {
    pub fn companion(namespace: Namespace, name: Name, span: Span) -> Self {
        Self::new(OverlayReferenceTarget::Companion, namespace, name, span)
    }

    pub fn import(alias: Name, namespace: Namespace, name: Name, span: Span) -> Self {
        Self::new(OverlayReferenceTarget::Import(alias), namespace, name, span)
    }

    pub fn helper(namespace: Namespace, name: Name, span: Span) -> Self {
        Self::new(OverlayReferenceTarget::Helper, namespace, name, span)
    }

    pub fn new(
        target: OverlayReferenceTarget,
        namespace: Namespace,
        name: Name,
        span: Span,
    ) -> Self {
        Self {
            target,
            namespace,
            name,
            span,
        }
    }

    pub fn target(&self) -> &OverlayReferenceTarget {
        &self.target
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

/// Unchecked source information handed to the overlay boundary.
#[derive(Debug, Clone)]
pub struct OverlayInput {
    package: Option<PackageId>,
    module: Option<ModulePath>,
    source_class: TestSourceClass,
    imports: Vec<OverlayImport>,
    declarations: Vec<OverlayDeclaration>,
    references: Vec<OverlayReference>,
    test_tree: Option<StaticTestTree>,
}

impl Default for OverlayInput {
    fn default() -> Self {
        Self {
            package: None,
            module: None,
            source_class: TestSourceClass::UnitTest,
            imports: Vec::new(),
            declarations: Vec::new(),
            references: Vec::new(),
            test_tree: None,
        }
    }
}

impl OverlayInput {
    pub fn new(package: PackageId, module: ModulePath) -> Self {
        Self {
            package: Some(package),
            module: Some(module),
            source_class: TestSourceClass::UnitTest,
            ..Self::default()
        }
    }

    pub fn with_source_class(mut self, source_class: TestSourceClass) -> Self {
        self.source_class = source_class;
        self
    }

    pub fn with_imports(mut self, imports: impl IntoIterator<Item = OverlayImport>) -> Self {
        self.imports = imports.into_iter().collect();
        self
    }

    pub fn with_declarations(
        mut self,
        declarations: impl IntoIterator<Item = OverlayDeclaration>,
    ) -> Self {
        self.declarations = declarations.into_iter().collect();
        self
    }

    pub fn with_references(
        mut self,
        references: impl IntoIterator<Item = OverlayReference>,
    ) -> Self {
        self.references = references.into_iter().collect();
        self
    }

    pub fn with_test_tree(mut self, test_tree: StaticTestTree) -> Self {
        self.test_tree = Some(test_tree);
        self
    }

    pub fn package(&self) -> Option<&PackageId> {
        self.package.as_ref()
    }

    pub fn module(&self) -> Option<&ModulePath> {
        self.module.as_ref()
    }

    pub fn source_class(&self) -> TestSourceClass {
        self.source_class
    }

    pub fn imports(&self) -> &[OverlayImport] {
        &self.imports
    }

    pub fn declarations(&self) -> &[OverlayDeclaration] {
        &self.declarations
    }

    pub fn references(&self) -> &[OverlayReference] {
        &self.references
    }

    pub fn test_tree(&self) -> Option<&StaticTestTree> {
        self.test_tree.as_ref()
    }
}

/// Where a validated reference resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedOverlayTarget {
    ProductionPrivate,
    ProductionPublic,
    OverlayPrivate,
    ImportedPublic { alias: Name, module: ModuleKey },
}

/// One resolved reference retained for later body checking/lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedOverlayReference {
    reference: OverlayReference,
    resolved: ResolvedOverlayTarget,
}

impl ResolvedOverlayReference {
    pub fn reference(&self) -> &OverlayReference {
        &self.reference
    }

    pub fn resolved(&self) -> &ResolvedOverlayTarget {
        &self.resolved
    }
}

/// Validated unit companion overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitOverlay {
    production: ProductionSeal,
    imports: Vec<OverlayImport>,
    declarations: Vec<OverlayDeclaration>,
    references: Vec<ResolvedOverlayReference>,
    test_tree: Option<StaticTestTree>,
}

impl UnitOverlay {
    pub fn production(&self) -> &ProductionSeal {
        &self.production
    }

    pub fn imports(&self) -> &[OverlayImport] {
        &self.imports
    }

    pub fn declarations(&self) -> &[OverlayDeclaration] {
        &self.declarations
    }

    pub fn references(&self) -> &[ResolvedOverlayReference] {
        &self.references
    }

    pub fn test_tree(&self) -> Option<&StaticTestTree> {
        self.test_tree.as_ref()
    }
}

/// Errors at the production/overlay boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayError {
    InvalidHash(String),
    ProductionNotSealed,
    ProductionDeclarationCollision,
    InvalidInput(String),
    MissingPackage,
    MissingModule,
    SourceClassNotUnit(TestSourceClass),
    PackageMismatch {
        expected: PackageId,
        actual: PackageId,
    },
    ModuleMismatch {
        expected: ModulePath,
        actual: ModulePath,
    },
    PublicDeclaration {
        name: Name,
        span: Span,
    },
    DeclarationCollision {
        name: Name,
        namespace: Namespace,
        span: Span,
    },
    ImportCollision {
        alias: Name,
        span: Span,
    },
    CompanionImport {
        alias: Name,
        span: Span,
    },
    UnknownImport {
        alias: Name,
        span: Span,
    },
    ReferenceNotFound {
        name: Name,
        namespace: Namespace,
        span: Span,
    },
    PrivateImport {
        name: Name,
        namespace: Namespace,
        span: Span,
    },
    ProductionCoherenceMutation {
        span: Span,
    },
    TestTreeMismatch {
        message: String,
    },
}

impl fmt::Display for OverlayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHash(error) => write!(formatter, "invalid production hash: {error}"),
            Self::ProductionNotSealed => formatter.write_str(
                "unit overlay requires a production module sealed after resolution and semantic checking",
            ),
            Self::ProductionDeclarationCollision => {
                formatter.write_str("production seal contains a duplicate declaration")
            }
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::MissingPackage => formatter.write_str("unit overlay has no package identity"),
            Self::MissingModule => formatter.write_str("unit overlay has no module identity"),
            Self::SourceClassNotUnit(class) => write!(
                formatter,
                "source class `{}` cannot be a unit overlay",
                class.as_str()
            ),
            Self::PackageMismatch { expected, actual } => write!(
                formatter,
                "unit overlay belongs to package `{actual}`, expected `{expected}`"
            ),
            Self::ModuleMismatch { expected, actual } => write!(
                formatter,
                "unit overlay belongs to module `{actual}`, expected `{expected}`"
            ),
            Self::PublicDeclaration { name, .. } => {
                write!(formatter, "unit overlay declaration `{name}` cannot be public")
            }
            Self::DeclarationCollision { namespace, name, .. } => write!(
                formatter,
                "unit overlay declaration `{namespace}::{name}` collides with production or another helper"
            ),
            Self::ImportCollision { alias, .. } => {
                write!(formatter, "unit overlay import alias `{alias}` is declared more than once")
            }
            Self::CompanionImport { alias, .. } => write!(
                formatter,
                "unit overlay cannot import its companion module as `{alias}`"
            ),
            Self::UnknownImport { alias, .. } => {
                write!(formatter, "unit overlay refers to unknown import `{alias}`")
            }
            Self::ReferenceNotFound { namespace, name, .. } => {
                write!(formatter, "overlay reference `{namespace}::{name}` was not found")
            }
            Self::PrivateImport { namespace, name, .. } => write!(
                formatter,
                "overlay cannot access private imported declaration `{namespace}::{name}`"
            ),
            Self::ProductionCoherenceMutation { .. } => formatter.write_str(
                "unit overlay cannot add an implementation or otherwise change production coherence",
            ),
            Self::TestTreeMismatch { message } => write!(formatter, "invalid unit test tree: {message}"),
        }
    }
}

impl Error for OverlayError {}

/// Validate and build a unit overlay over a sealed production unit.
pub fn build(seal: &ProductionSeal, input: OverlayInput) -> Result<UnitOverlay, OverlayError> {
    if input.source_class != TestSourceClass::UnitTest {
        return Err(OverlayError::SourceClassNotUnit(input.source_class));
    }
    let package = input.package.as_ref().ok_or(OverlayError::MissingPackage)?;
    let module = input.module.as_ref().ok_or(OverlayError::MissingModule)?;
    if package != seal.module.package() {
        return Err(OverlayError::PackageMismatch {
            expected: seal.module.package().clone(),
            actual: package.clone(),
        });
    }
    if module != seal.module.module() {
        return Err(OverlayError::ModuleMismatch {
            expected: seal.module.module().clone(),
            actual: module.clone(),
        });
    }

    let mut imports = input.imports;
    imports.sort_by(|left, right| left.alias().cmp(right.alias()));
    let mut aliases = BTreeSet::new();
    for import in &imports {
        if !aliases.insert(import.alias().clone()) {
            return Err(OverlayError::ImportCollision {
                alias: import.alias().clone(),
                span: import.span(),
            });
        }
        if import.target() == &seal.module {
            return Err(OverlayError::CompanionImport {
                alias: import.alias().clone(),
                span: import.span(),
            });
        }
    }

    let mut declarations = input.declarations;
    declarations.sort_by(|left, right| {
        (left.namespace(), left.name().clone(), left.span()).cmp(&(
            right.namespace(),
            right.name().clone(),
            right.span(),
        ))
    });
    let mut declaration_keys = BTreeSet::new();
    for declaration in &declarations {
        if declaration.visibility() != Visibility::Private {
            return Err(OverlayError::PublicDeclaration {
                name: declaration.name().clone(),
                span: declaration.span(),
            });
        }
        if declaration.kind() == OverlayDeclarationKind::CoherenceImplementation
            || declaration.namespace() == Namespace::Module
        {
            return Err(OverlayError::ProductionCoherenceMutation {
                span: declaration.span(),
            });
        }
        let key = declaration.key();
        if seal.declarations.contains_key(&key) || !declaration_keys.insert(key) {
            return Err(OverlayError::DeclarationCollision {
                name: declaration.name().clone(),
                namespace: declaration.namespace(),
                span: declaration.span(),
            });
        }
    }

    let helpers = declaration_keys;
    let mut references = input.references;
    references.sort_by(|left, right| {
        (
            left.span().file().index(),
            left.span().range(),
            left.namespace(),
            left.name().clone(),
        )
            .cmp(&(
                right.span().file().index(),
                right.span().range(),
                right.namespace(),
                right.name().clone(),
            ))
    });
    let mut resolved_references = Vec::with_capacity(references.len());
    for reference in references {
        let key = DeclarationKey::new(reference.namespace(), reference.name().clone());
        let resolved = match reference.target() {
            OverlayReferenceTarget::Companion => {
                let Some(declaration) = seal.declarations.get(&key) else {
                    return Err(OverlayError::ReferenceNotFound {
                        name: reference.name().clone(),
                        namespace: reference.namespace(),
                        span: reference.span(),
                    });
                };
                if declaration.visibility() == Visibility::Private {
                    ResolvedOverlayTarget::ProductionPrivate
                } else {
                    ResolvedOverlayTarget::ProductionPublic
                }
            }
            OverlayReferenceTarget::Helper => {
                if !helpers.contains(&key) {
                    return Err(OverlayError::ReferenceNotFound {
                        name: reference.name().clone(),
                        namespace: reference.namespace(),
                        span: reference.span(),
                    });
                }
                ResolvedOverlayTarget::OverlayPrivate
            }
            OverlayReferenceTarget::Import(alias) => {
                let Some(import) = imports.iter().find(|import| import.alias() == alias) else {
                    return Err(OverlayError::UnknownImport {
                        alias: alias.clone(),
                        span: reference.span(),
                    });
                };
                if !import.exports(reference.namespace(), reference.name()) {
                    return Err(OverlayError::PrivateImport {
                        name: reference.name().clone(),
                        namespace: reference.namespace(),
                        span: reference.span(),
                    });
                }
                ResolvedOverlayTarget::ImportedPublic {
                    alias: alias.clone(),
                    module: import.target().clone(),
                }
            }
        };
        resolved_references.push(ResolvedOverlayReference {
            reference,
            resolved,
        });
    }

    if let Some(test_tree) = &input.test_tree {
        for node in test_tree.nodes() {
            let identity = node.identity();
            if identity.package() != package
                || identity.module() != module
                || identity.source_class() != TestSourceClass::UnitTest
            {
                return Err(OverlayError::TestTreeMismatch {
                    message: format!(
                        "node `{}` does not belong to the companion module",
                        node.visible_id()
                    ),
                });
            }
        }
    }

    Ok(UnitOverlay {
        production: seal.clone(),
        imports,
        declarations,
        references: resolved_references,
        test_tree: input.test_tree,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::artifact::sha256;
    use crate::package::Name;
    use crate::resolve::resolve;
    use crate::source::{
        LogicalPath, SourceDatabase, SourceId, SourceInput, SourceOrigin, TextRange,
    };
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};
    use crate::test_tree::{TestSourceInput, build as build_tree};

    fn package() -> PackageId {
        PackageId::new("workspace:app@1").unwrap()
    }

    fn module() -> ModulePath {
        ModulePath::new("math").unwrap()
    }

    fn file_id() -> FileId {
        FileId::from_index(0).unwrap()
    }

    fn key() -> ModuleKey {
        ModuleKey::new(package(), module())
    }

    fn span(file: FileId, start: u32) -> Span {
        let mut sources = SourceDatabase::new();
        // Spans are value types.  Constructing one through the source DB keeps
        // tests aligned with the same range validation used by the compiler.
        for index in 0..=file.index() {
            sources
                .add(SourceInput::new(
                    SourceId::new(format!("test:{index}")).unwrap(),
                    module(),
                    LogicalPath::new(format!("src/{index}.to")).unwrap(),
                    SourceOrigin::Virtual,
                    std::sync::Arc::<[u8]>::from(vec![0_u8; 32]),
                ))
                .unwrap();
        }
        sources
            .span(file, TextRange::new(start, start + 1).unwrap())
            .unwrap()
    }

    fn hashes() -> ProductionHashes {
        ProductionHashes::new(
            sha256(b"interface"),
            sha256(b"capabilities"),
            sha256(b"coherence"),
            sha256(b"artifact"),
        )
        .unwrap()
    }

    fn seal() -> ProductionSeal {
        ProductionSeal::new(
            key(),
            [file_id()],
            [
                ProductionDeclaration::new(
                    Namespace::Value,
                    Name::new("private_value").unwrap(),
                    SymbolKind::Function,
                    Visibility::Private,
                    span(file_id(), 0),
                    0,
                    false,
                ),
                ProductionDeclaration::new(
                    Namespace::Value,
                    Name::new("public_value").unwrap(),
                    SymbolKind::Function,
                    Visibility::Public,
                    span(file_id(), 2),
                    0,
                    false,
                ),
            ],
            ProductionProof::verified(hashes()),
        )
        .unwrap()
    }

    fn name(value: &str) -> Name {
        Name::new(value).unwrap()
    }

    #[test]
    fn accepts_companion_private_access_and_keeps_production_identity() {
        let seal = seal();
        let before = seal.proof().hashes().clone();
        let overlay = build(
            &seal,
            OverlayInput::new(package(), module()).with_references([
                OverlayReference::companion(
                    Namespace::Value,
                    name("private_value"),
                    span(file_id(), 4),
                ),
                OverlayReference::companion(
                    Namespace::Value,
                    name("public_value"),
                    span(file_id(), 5),
                ),
            ]),
        )
        .unwrap();
        assert!(matches!(
            overlay.references()[0].resolved(),
            ResolvedOverlayTarget::ProductionPrivate
        ));
        assert!(matches!(
            overlay.references()[1].resolved(),
            ResolvedOverlayTarget::ProductionPublic
        ));
        assert_eq!(overlay.production().proof().hashes(), &before);
        assert_eq!(overlay.production().declarations().count(), 2);
    }

    #[test]
    fn rejects_incomplete_production_before_overlay() {
        let result = ProductionSeal::new(
            key(),
            [file_id()],
            [],
            ProductionProof::new(false, true, true, hashes()),
        );
        assert!(matches!(result, Err(OverlayError::ProductionNotSealed)));
    }

    #[test]
    fn rejects_invalid_hashes() {
        let result = ProductionHashes::new("bad", sha256(b"c"), sha256(b"k"), sha256(b"a"));
        assert!(matches!(result, Err(OverlayError::InvalidHash(_))));
    }

    #[test]
    fn rejects_public_helpers_and_production_collisions() {
        let seal = seal();
        let public = OverlayDeclaration::new(
            Namespace::Value,
            name("helper"),
            OverlayDeclarationKind::PrivateFunction,
            Visibility::Public,
            span(file_id(), 6),
        );
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_declarations([public])
            ),
            Err(OverlayError::PublicDeclaration { .. })
        ));

        let collision = OverlayDeclaration::private_helper(
            name("private_value"),
            OverlayDeclarationKind::PrivateFunction,
            span(file_id(), 7),
        );
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_declarations([collision])
            ),
            Err(OverlayError::DeclarationCollision { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_helpers_and_import_aliases() {
        let seal = seal();
        let helper = || {
            OverlayDeclaration::private_helper(
                name("helper"),
                OverlayDeclarationKind::PrivateFunction,
                span(file_id(), 8),
            )
        };
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_declarations([helper(), helper()]),
            ),
            Err(OverlayError::DeclarationCollision { .. })
        ));

        let dependency = ModuleKey::new(
            PackageId::new("dep@1").unwrap(),
            ModulePath::new("api").unwrap(),
        );
        let import = || OverlayImport::new(name("api"), dependency.clone(), [], span(file_id(), 9));
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_imports([import(), import()]),
            ),
            Err(OverlayError::ImportCollision { .. })
        ));
    }

    #[test]
    fn allows_only_explicit_public_imports() {
        let seal = seal();
        let dependency = ModuleKey::new(
            PackageId::new("dep@1").unwrap(),
            ModulePath::new("api").unwrap(),
        );
        let import = OverlayImport::new(
            name("api"),
            dependency.clone(),
            [(Namespace::Value, name("public_api"))],
            span(file_id(), 10),
        );
        let input = OverlayInput::new(package(), module())
            .with_imports([import])
            .with_references([OverlayReference::import(
                name("api"),
                Namespace::Value,
                name("public_api"),
                span(file_id(), 11),
            )]);
        let overlay = build(&seal, input).unwrap();
        assert!(matches!(
            overlay.references()[0].resolved(),
            ResolvedOverlayTarget::ImportedPublic { .. }
        ));

        let private = OverlayInput::new(package(), module())
            .with_imports([OverlayImport::new(
                name("api"),
                dependency,
                [],
                span(file_id(), 12),
            )])
            .with_references([OverlayReference::import(
                name("api"),
                Namespace::Value,
                name("private_api"),
                span(file_id(), 13),
            )]);
        assert!(matches!(
            build(&seal, private),
            Err(OverlayError::PrivateImport { .. })
        ));
    }

    #[test]
    fn rejects_production_source_class_and_self_import() {
        let seal = seal();
        let production =
            OverlayInput::new(package(), module()).with_source_class(TestSourceClass::Production);
        assert!(matches!(
            build(&seal, production),
            Err(OverlayError::SourceClassNotUnit(
                TestSourceClass::Production
            ))
        ));

        let self_import = OverlayImport::new(name("self_math"), key(), [], span(file_id(), 14));
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_imports([self_import]),
            ),
            Err(OverlayError::CompanionImport { .. })
        ));
    }

    #[test]
    fn rejects_coherence_namespace_and_unknown_references() {
        let seal = seal();
        let module_declaration = OverlayDeclaration::new(
            Namespace::Type,
            name("coherence"),
            OverlayDeclarationKind::CoherenceImplementation,
            Visibility::Private,
            span(file_id(), 15),
        );
        assert!(matches!(
            build(
                &seal,
                OverlayInput::new(package(), module()).with_declarations([module_declaration]),
            ),
            Err(OverlayError::ProductionCoherenceMutation { .. })
        ));

        let unknown =
            OverlayInput::new(package(), module()).with_references([OverlayReference::companion(
                Namespace::Value,
                name("missing"),
                span(file_id(), 16),
            )]);
        assert!(matches!(
            build(&seal, unknown),
            Err(OverlayError::ReferenceNotFound { .. })
        ));
    }

    #[test]
    fn helper_references_are_private_and_deterministic() {
        let seal = seal();
        let helper = OverlayDeclaration::private_helper(
            name("helper"),
            OverlayDeclarationKind::PrivateFunction,
            span(file_id(), 17),
        );
        let input = OverlayInput::new(package(), module())
            .with_declarations([helper])
            .with_references([OverlayReference::helper(
                Namespace::Value,
                name("helper"),
                span(file_id(), 18),
            )]);
        let overlay = build(&seal, input).unwrap();
        assert_eq!(overlay.declarations()[0].name().as_str(), "helper");
        assert!(matches!(
            overlay.references()[0].resolved(),
            ResolvedOverlayTarget::OverlayPrivate
        ));
    }

    #[test]
    fn resolved_adapter_filters_companion_files() {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::new(
                SourceId::new("src:production-adapter").unwrap(),
                module(),
                LogicalPath::new("src/math.to").unwrap(),
                SourceOrigin::Virtual,
                Arc::<[u8]>::from(
                    b"fn private_value(): Int { 1 }\npub fn public_value(): Int { 2 }\n".to_vec(),
                ),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        assert!(lexed.diagnostics().is_empty());
        let parsed = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        assert!(parsed.diagnostics().is_empty());
        let packages = crate::package::PackageGraph::loose(&sources, file).unwrap();
        let resolved = resolve(&packages, &sources, [(file, &parsed)], 100)
            .unwrap()
            .into_parts()
            .0;
        let seal = ProductionSeal::from_resolved(
            ModuleKey::new(packages.root().clone(), module()),
            &resolved,
            [file],
            ProductionProof::verified(hashes()),
        )
        .unwrap();
        assert_eq!(seal.production_files().len(), 1);
        assert_eq!(seal.declarations().count(), 2);
        assert_eq!(
            seal.declaration(Namespace::Value, &name("private_value"))
                .unwrap()
                .visibility(),
            Visibility::Private
        );
        assert!(matches!(
            ProductionSeal::from_resolved(
                ModuleKey::new(packages.root().clone(), module()),
                &resolved,
                [FileId::from_index(1).unwrap()],
                ProductionProof::verified(hashes()),
            ),
            Err(OverlayError::InvalidInput(_))
        ));
    }

    #[test]
    fn accessors_and_identity_guards_are_closed() {
        let hashes = hashes();
        assert!(hashes.interface().starts_with("sha256:"));
        assert!(hashes.capabilities().starts_with("sha256:"));
        assert!(hashes.coherence().starts_with("sha256:"));
        assert!(hashes.artifact().starts_with("sha256:"));

        let proof = ProductionProof::verified(hashes.clone());
        assert!(proof.resolution_complete());
        assert!(proof.semantic_check_complete());
        assert!(proof.coherence_check_complete());
        assert_eq!(proof.hashes(), &hashes);

        let module_key = key();
        assert_eq!(module_key.package().as_str(), "workspace:app@1");
        assert_eq!(module_key.module().as_str(), "math");
        assert_eq!(module_key.to_string(), "workspace:app@1::math");

        let declaration = ProductionDeclaration::new(
            Namespace::Type,
            name("PrivateType"),
            SymbolKind::Type,
            Visibility::Private,
            span(file_id(), 19),
            2,
            true,
        );
        assert_eq!(declaration.namespace(), Namespace::Type);
        assert_eq!(declaration.name().as_str(), "PrivateType");
        assert_eq!(declaration.kind(), SymbolKind::Type);
        assert_eq!(declaration.visibility(), Visibility::Private);
        assert_eq!(declaration.span().file(), file_id());
        assert_eq!(declaration.generic_arity(), 2);
        assert!(declaration.is_synthetic());

        let invalid_file = FileId::from_index(1).unwrap();
        assert!(matches!(
            ProductionSeal::new(
                key(),
                [file_id()],
                [ProductionDeclaration::new(
                    Namespace::Value,
                    name("wrong_file"),
                    SymbolKind::Function,
                    Visibility::Private,
                    span(invalid_file, 0),
                    0,
                    false,
                )],
                proof,
            ),
            Err(OverlayError::InvalidInput(_))
        ));

        let default = OverlayInput::default();
        assert!(default.package().is_none());
        assert!(default.module().is_none());
        assert_eq!(default.source_class(), TestSourceClass::UnitTest);
        assert!(default.imports().is_empty());
        assert!(default.declarations().is_empty());
        assert!(default.references().is_empty());
        assert!(default.test_tree().is_none());
        assert!(matches!(
            build(&seal(), default),
            Err(OverlayError::MissingPackage)
        ));

        let missing_module = OverlayInput {
            package: Some(package()),
            ..OverlayInput::default()
        };
        assert!(matches!(
            build(&seal(), missing_module),
            Err(OverlayError::MissingModule)
        ));
        assert!(matches!(
            build(
                &seal(),
                OverlayInput::new(PackageId::new("other@1").unwrap(), module())
            ),
            Err(OverlayError::PackageMismatch { .. })
        ));
        assert!(matches!(
            build(
                &seal(),
                OverlayInput::new(package(), ModulePath::new("other").unwrap())
            ),
            Err(OverlayError::ModuleMismatch { .. })
        ));

        let dependency = ModuleKey::new(
            PackageId::new("dep@1").unwrap(),
            ModulePath::new("api").unwrap(),
        );
        let import = OverlayImport::new(
            name("api"),
            dependency.clone(),
            [(Namespace::Value, name("public_api"))],
            span(file_id(), 20),
        );
        assert_eq!(import.alias().as_str(), "api");
        assert_eq!(import.target(), &dependency);
        assert!(import.exports(Namespace::Value, &name("public_api")));
        assert!(!import.exports(Namespace::Value, &name("private_api")));

        let reference = OverlayReference::new(
            OverlayReferenceTarget::Import(name("api")),
            Namespace::Value,
            name("public_api"),
            span(file_id(), 21),
        );
        assert!(matches!(
            reference.target(),
            OverlayReferenceTarget::Import(alias) if alias.as_str() == "api"
        ));
        assert_eq!(reference.namespace(), Namespace::Value);
        assert_eq!(reference.name().as_str(), "public_api");
        assert_eq!(reference.span().file(), file_id());

        let mut tree_sources = SourceDatabase::new();
        let tree_file = tree_sources
            .add(SourceInput::new(
                SourceId::new("src:overlay-tree").unwrap(),
                module(),
                LogicalPath::new("src/math_test.to").unwrap(),
                SourceOrigin::Virtual,
                Arc::<[u8]>::from(b"test helper {}\n".to_vec()),
            ))
            .unwrap();
        let tree_lexed = lex(&tree_sources, tree_file, LexMode::Module).unwrap();
        let tree_parsed = parse(
            &tree_sources,
            tree_file,
            tree_lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        let tree = build_tree(
            &tree_sources,
            [TestSourceInput::new(
                &package(),
                "app",
                TestSourceClass::UnitTest,
                &module(),
                &LogicalPath::new("src/math_test.to").unwrap(),
                tree_file,
                tree_parsed.cst(),
            )],
        )
        .unwrap();

        let input = OverlayInput::new(package(), module())
            .with_imports([import])
            .with_declarations([OverlayDeclaration::private_helper(
                name("helper"),
                OverlayDeclarationKind::PrivateFunction,
                span(file_id(), 22),
            )])
            .with_references([reference])
            .with_test_tree(tree);
        assert_eq!(input.package().unwrap().as_str(), package().as_str());
        assert_eq!(input.module().unwrap().as_str(), module().as_str());
        assert_eq!(input.imports().len(), 1);
        assert_eq!(input.declarations().len(), 1);
        assert_eq!(input.references().len(), 1);
        let overlay = build(&seal(), input).unwrap();
        assert_eq!(overlay.production().module(), &key());
        assert_eq!(overlay.imports().len(), 1);
        assert_eq!(overlay.declarations().len(), 1);
        assert_eq!(overlay.references().len(), 1);
        assert!(overlay.test_tree().is_some());
        assert!(matches!(
            overlay.references()[0].resolved(),
            ResolvedOverlayTarget::ImportedPublic { alias, module }
                if alias.as_str() == "api" && module == &dependency
        ));

        let integration_tree = build_tree(
            &tree_sources,
            [TestSourceInput::new(
                &package(),
                "app",
                TestSourceClass::IntegrationTest,
                &module(),
                &LogicalPath::new("src/math_test.to").unwrap(),
                tree_file,
                tree_parsed.cst(),
            )],
        )
        .unwrap();
        assert!(matches!(
            build(
                &seal(),
                OverlayInput::new(package(), module()).with_test_tree(integration_tree)
            ),
            Err(OverlayError::TestTreeMismatch { .. })
        ));

        let missing_import =
            OverlayInput::new(package(), module()).with_references([OverlayReference::import(
                name("missing"),
                Namespace::Value,
                name("x"),
                span(file_id(), 23),
            )]);
        assert!(matches!(
            build(&seal(), missing_import),
            Err(OverlayError::UnknownImport { .. })
        ));
    }
}
