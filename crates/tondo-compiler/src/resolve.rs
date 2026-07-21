use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::diagnostics::{Diagnostic, DiagnosticCode, DiagnosticError, Related, Severity};
use crate::package::{
    DeclarationPath, ImportResolutionError, ModuleId, Name, NameError, Namespace, PackageGraph,
    PackageGraphError, SymbolIdentity,
};
use crate::source::{FileId, SourceDatabase, SourceError, Span, TextRange};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};

mod api;
mod members;
mod names;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(u32);

impl SymbolId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberId(u32);

impl MemberId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberName(String);

impl MemberName {
    pub fn new(value: impl AsRef<str>) -> Result<Self, NameError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(NameError::Empty);
        }
        let normalized = value.nfc().collect::<String>();
        if normalized == "_" {
            return Err(NameError::Discard);
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

impl fmt::Display for MemberName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(u32);

impl LocalId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalKind {
    GenericParameter,
    Parameter,
    Binding,
    Pattern,
    ForPattern,
    ClosureParameter,
}

#[derive(Debug, Clone)]
pub struct LocalBinding {
    id: LocalId,
    name: Name,
    kind: LocalKind,
    span: Span,
}

impl LocalBinding {
    pub fn id(&self) -> LocalId {
        self.id
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn kind(&self) -> LocalKind {
        self.kind
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedName {
    Symbol(SymbolId),
    Local(LocalId),
    Receiver,
    ContextualSelf,
    Prelude {
        namespace: Namespace,
        name: Name,
    },
    External {
        module: ModuleId,
        namespace: Namespace,
        name: Name,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedEntity {
    Name(ResolvedName),
    Module(ModuleId),
    ContextualCandidates {
        type_name: ResolvedName,
        value_name: ResolvedName,
    },
}

#[derive(Debug, Clone)]
pub struct ResolvedReference {
    file: FileId,
    range: TextRange,
    entity: ResolvedEntity,
}

impl ResolvedReference {
    pub fn file(&self) -> FileId {
        self.file
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn entity(&self) -> &ResolvedEntity {
        &self.entity
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Visibility {
    Private,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SymbolKind {
    Constant,
    Function,
    Type,
    Alias,
    Enum,
    Trait,
    NewtypeConstructor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberOwner {
    Type(SymbolId),
    Variant(MemberId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberKind {
    RecordField,
    NewtypeValue,
    EnumVariant,
    VariantField,
    InherentMethod,
    AssociatedFunction,
    TraitMethod,
    TraitAssociatedFunction,
}

impl MemberKind {
    pub fn is_field(self) -> bool {
        matches!(
            self,
            Self::RecordField | Self::NewtypeValue | Self::VariantField
        )
    }

    pub fn is_method(self) -> bool {
        matches!(self, Self::InherentMethod | Self::TraitMethod)
    }

    pub fn is_callable(self) -> bool {
        matches!(
            self,
            Self::InherentMethod
                | Self::AssociatedFunction
                | Self::TraitMethod
                | Self::TraitAssociatedFunction
        )
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    id: MemberId,
    owner: MemberOwner,
    name: MemberName,
    kind: MemberKind,
    visibility: Visibility,
    span: Span,
    generic_arity: u32,
    synthetic: bool,
}

impl Member {
    pub fn id(&self) -> MemberId {
        self.id
    }

    pub fn owner(&self) -> MemberOwner {
        self.owner
    }

    pub fn name(&self) -> &MemberName {
        &self.name
    }

    pub fn kind(&self) -> MemberKind {
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

#[derive(Debug, Clone)]
pub struct Symbol {
    id: SymbolId,
    identity: SymbolIdentity,
    name: Name,
    kind: SymbolKind,
    visibility: Visibility,
    span: Span,
    generic_arity: u32,
}

impl Symbol {
    pub fn id(&self) -> SymbolId {
        self.id
    }

    pub fn identity(&self) -> &SymbolIdentity {
        &self.identity
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
}

#[derive(Debug, Clone)]
pub struct ImportBinding {
    alias: Name,
    module: ModuleId,
    span: Span,
}

impl ImportBinding {
    pub fn alias(&self) -> &Name {
        &self.alias
    }

    pub fn module(&self) -> &ModuleId {
        &self.module
    }

    pub fn span(&self) -> Span {
        self.span
    }
}

#[derive(Debug, Clone, Default)]
pub struct FileResolution {
    imports: BTreeMap<Name, ImportBinding>,
}

impl FileResolution {
    pub fn imports(&self) -> &BTreeMap<Name, ImportBinding> {
        &self.imports
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    id: ModuleId,
    files: Vec<FileId>,
    types: BTreeMap<Name, SymbolId>,
    values: BTreeMap<Name, SymbolId>,
}

impl ResolvedModule {
    pub fn id(&self) -> &ModuleId {
        &self.id
    }

    pub fn files(&self) -> &[FileId] {
        &self.files
    }

    pub fn lookup(&self, namespace: Namespace, name: &Name) -> Option<SymbolId> {
        match namespace {
            Namespace::Type => self.types.get(name).copied(),
            Namespace::Value => self.values.get(name).copied(),
            Namespace::Module => None,
        }
    }
}

#[derive(Debug)]
pub struct ResolvedProgram {
    modules: BTreeMap<ModuleId, ResolvedModule>,
    files: BTreeMap<FileId, FileResolution>,
    symbols: Vec<Symbol>,
    members: Vec<Member>,
    members_by_owner: BTreeMap<(MemberOwner, MemberName), Vec<MemberId>>,
    locals: Vec<LocalBinding>,
    references: BTreeMap<(FileId, u32, u32), ResolvedReference>,
}

impl ResolvedProgram {
    pub fn modules(&self) -> impl ExactSizeIterator<Item = &ResolvedModule> {
        self.modules.values()
    }

    pub fn file(&self, file: FileId) -> Option<&FileResolution> {
        self.files.get(&file)
    }

    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(id.0 as usize)
    }

    pub fn symbols(&self) -> impl ExactSizeIterator<Item = &Symbol> {
        self.symbols.iter()
    }

    pub fn member(&self, id: MemberId) -> Option<&Member> {
        self.members.get(id.0 as usize)
    }

    pub fn members(&self) -> impl ExactSizeIterator<Item = &Member> {
        self.members.iter()
    }

    pub fn member_at(&self, file: FileId, range: TextRange) -> Option<&Member> {
        self.members
            .iter()
            .find(|member| member.span.file() == file && member.span.range() == range)
    }

    pub fn lookup_members(&self, owner: MemberOwner, name: &MemberName) -> Option<&[MemberId]> {
        self.members_by_owner
            .get(&(owner, name.clone()))
            .map(Vec::as_slice)
    }

    pub fn module(&self, id: &ModuleId) -> Option<&ResolvedModule> {
        self.modules.get(id)
    }

    pub fn locals(&self) -> impl ExactSizeIterator<Item = &LocalBinding> {
        self.locals.iter()
    }

    pub fn local(&self, id: LocalId) -> Option<&LocalBinding> {
        self.locals.get(id.0 as usize)
    }

    pub fn local_at(&self, file: FileId, range: TextRange) -> Option<&LocalBinding> {
        self.locals
            .iter()
            .find(|local| local.span.file() == file && local.span.range() == range)
    }

    pub fn reference(&self, file: FileId, range: TextRange) -> Option<&ResolvedReference> {
        self.references.get(&(file, range.start(), range.end()))
    }

    pub fn references(&self) -> impl ExactSizeIterator<Item = &ResolvedReference> {
        self.references.values()
    }
}

#[derive(Debug)]
pub struct ResolveOutput {
    program: ResolvedProgram,
    diagnostics: Vec<Diagnostic>,
}

impl ResolveOutput {
    pub fn program(&self) -> &ResolvedProgram {
        &self.program
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_parts(self) -> (ResolvedProgram, Vec<Diagnostic>) {
        (self.program, self.diagnostics)
    }
}

#[derive(Debug)]
pub enum ResolveError {
    Source(SourceError),
    PackageGraph(PackageGraphError),
    Diagnostic(DiagnosticError),
    DiagnosticLimit { file: FileId, offset: u32 },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::PackageGraph(error) => error.fmt(formatter),
            Self::Diagnostic(error) => error.fmt(formatter),
            Self::DiagnosticLimit { offset, .. } => {
                write!(
                    formatter,
                    "primary diagnostic count limit reached at byte {offset}"
                )
            }
        }
    }
}

impl Error for ResolveError {}

impl From<SourceError> for ResolveError {
    fn from(error: SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<PackageGraphError> for ResolveError {
    fn from(error: PackageGraphError) -> Self {
        Self::PackageGraph(error)
    }
}

impl From<DiagnosticError> for ResolveError {
    fn from(error: DiagnosticError) -> Self {
        Self::Diagnostic(error)
    }
}

#[derive(Debug, Clone)]
struct DeclarationCandidate {
    module: ModuleId,
    namespace: Namespace,
    name: Name,
    kind: SymbolKind,
    visibility: Visibility,
    file: FileId,
    range: TextRange,
    generic_arity: u32,
}

#[derive(Debug, Clone)]
struct ImportEdge {
    from: ModuleId,
    to: ModuleId,
    file: FileId,
    range: TextRange,
}

struct Resolver<'a> {
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: BTreeMap<FileId, &'a Parsed>,
    diagnostics: Vec<Diagnostic>,
    max_diagnostics: usize,
}

pub fn resolve<'a>(
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: impl IntoIterator<Item = (FileId, &'a Parsed)>,
    max_diagnostics: usize,
) -> Result<ResolveOutput, ResolveError> {
    let parsed = parsed.into_iter().collect::<BTreeMap<_, _>>();
    let mut resolver = Resolver {
        packages,
        sources,
        parsed,
        diagnostics: Vec::new(),
        max_diagnostics,
    };
    resolver.resolve()
}

impl Resolver<'_> {
    fn resolve(&mut self) -> Result<ResolveOutput, ResolveError> {
        let mut declarations = Vec::new();
        let mut files = BTreeMap::new();
        let mut edges = Vec::new();
        let ordered_files = self.ordered_files()?;
        for file in &ordered_files {
            self.collect_file(*file, &mut declarations, &mut files, &mut edges)?;
        }
        let (modules, symbols) = self.build_symbols(declarations, &ordered_files)?;
        self.diagnose_import_declaration_conflicts(&modules, &files, &symbols, &ordered_files)?;
        self.diagnose_import_cycles(&edges)?;
        let mut program = ResolvedProgram {
            modules,
            files,
            symbols,
            members: Vec::new(),
            members_by_owner: BTreeMap::new(),
            locals: Vec::new(),
            references: BTreeMap::new(),
        };
        members::collect_members(
            self.packages,
            self.sources,
            &self.parsed,
            &ordered_files,
            &mut program,
            &mut self.diagnostics,
            self.max_diagnostics,
        )?;
        names::resolve_names(
            self.packages,
            self.sources,
            &self.parsed,
            &ordered_files,
            &mut program,
            &mut self.diagnostics,
            self.max_diagnostics,
        )?;
        api::validate_public_apis(
            self.packages,
            self.sources,
            &self.parsed,
            &ordered_files,
            &program,
            &mut self.diagnostics,
            self.max_diagnostics,
        )?;
        Ok(ResolveOutput {
            program,
            diagnostics: std::mem::take(&mut self.diagnostics),
        })
    }

    fn ordered_files(&self) -> Result<Vec<FileId>, ResolveError> {
        let mut files = self.parsed.keys().copied().collect::<Vec<_>>();
        files.sort_by_key(|file| self.file_key(*file));
        Ok(files)
    }

    fn collect_file(
        &mut self,
        file: FileId,
        declarations: &mut Vec<DeclarationCandidate>,
        files: &mut BTreeMap<FileId, FileResolution>,
        edges: &mut Vec<ImportEdge>,
    ) -> Result<(), ResolveError> {
        let parsed = self.parsed[&file];
        let module = self.packages.module_for_file(self.sources, file)?;
        let root = parsed.cst().root_node();
        let mut file_resolution = FileResolution::default();
        let mut encountered_non_import = false;
        for child in root.child_nodes() {
            match child.kind() {
                SyntaxKind::ImportDecl => {
                    if encountered_non_import {
                        self.push_diagnostic(
                            file,
                            child.range(),
                            "E1007",
                            "imports must appear before declarations and statements",
                        )?;
                    }
                    self.collect_import(file, &module, child, &mut file_resolution, edges)?;
                }
                SyntaxKind::ConstDecl
                | SyntaxKind::TypeDecl
                | SyntaxKind::AliasDecl
                | SyntaxKind::EnumDecl
                | SyntaxKind::TraitDecl
                | SyntaxKind::ImplDecl
                | SyntaxKind::FunctionDecl
                | SyntaxKind::BindingDecl
                | SyntaxKind::Assignment
                | SyntaxKind::ReturnStmt
                | SyntaxKind::FailStmt
                | SyntaxKind::BreakStmt
                | SyntaxKind::ContinueStmt
                | SyntaxKind::DeferStmt
                | SyntaxKind::ForStmt
                | SyntaxKind::ExpressionStmt
                | SyntaxKind::TailExpression => {
                    encountered_non_import = true;
                    if let Some(candidate) =
                        self.declaration_candidate(file, module.clone(), child)?
                    {
                        declarations.push(candidate.clone());
                        if candidate.kind == SymbolKind::Type
                            && child.kind() == SyntaxKind::TypeDecl
                            && child
                                .child_nodes()
                                .all(|node| node.kind() != SyntaxKind::RecordBody)
                        {
                            declarations.push(DeclarationCandidate {
                                namespace: Namespace::Value,
                                kind: SymbolKind::NewtypeConstructor,
                                ..candidate
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        files.insert(file, file_resolution);
        Ok(())
    }

    fn collect_import(
        &mut self,
        file: FileId,
        from: &ModuleId,
        node: SyntaxNodeRef<'_>,
        resolution: &mut FileResolution,
        edges: &mut Vec<ImportEdge>,
    ) -> Result<(), ResolveError> {
        let Some(path_node) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ModulePath)
        else {
            return Ok(());
        };
        let segments = path_node
            .descendant_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .map(normalized_name)
            .collect::<Result<Vec<_>, _>>();
        let Ok(segments) = segments else {
            return Ok(());
        };
        let explicit_alias = node
            .child_tokens()
            .find(|token| token.kind() == TokenKind::Identifier);
        let alias_token = explicit_alias.or_else(|| {
            path_node
                .descendant_tokens()
                .filter(|token| token.kind() == TokenKind::Identifier)
                .last()
        });
        let Some(alias_token) = alias_token else {
            return Ok(());
        };
        let Ok(alias) = normalized_name(alias_token) else {
            return Ok(());
        };
        if is_reserved_unqualified(&alias) || alias.as_str() == "std" {
            self.push_diagnostic(
                file,
                alias_token.range(),
                "E1005",
                format!("`{alias}` is reserved and cannot be an import alias"),
            )?;
            return Ok(());
        }
        let target = match self.packages.resolve_import(from.package(), &segments) {
            Ok(module) => module,
            Err(error) => {
                self.push_diagnostic(
                    file,
                    path_node.range(),
                    "E1008",
                    import_error_message(&error),
                )?;
                return Ok(());
            }
        };
        let span = self.sources.span(file, alias_token.range())?;
        if let Some(previous) = resolution.imports.get(&alias) {
            let diagnostic = Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1002")?,
                format!("module name `{alias}` is imported more than once in this file"),
                crate::diagnostics::PrimaryLocation::Source(span),
            )?
            .with_related(Related::new("first import with this name", previous.span)?);
            self.push(file, alias_token.range().start(), diagnostic)?;
            return Ok(());
        }
        resolution.imports.insert(
            alias.clone(),
            ImportBinding {
                alias,
                module: target.clone(),
                span,
            },
        );
        edges.push(ImportEdge {
            from: from.clone(),
            to: target,
            file,
            range: node.range(),
        });
        Ok(())
    }

    fn declaration_candidate(
        &mut self,
        file: FileId,
        module: ModuleId,
        node: SyntaxNodeRef<'_>,
    ) -> Result<Option<DeclarationCandidate>, ResolveError> {
        let (namespace, kind, token) = match node.kind() {
            SyntaxKind::ConstDecl => (
                Namespace::Value,
                SymbolKind::Constant,
                first_identifier(node),
            ),
            SyntaxKind::TypeDecl => (Namespace::Type, SymbolKind::Type, first_identifier(node)),
            SyntaxKind::AliasDecl => (Namespace::Type, SymbolKind::Alias, first_identifier(node)),
            SyntaxKind::EnumDecl => (Namespace::Type, SymbolKind::Enum, first_identifier(node)),
            SyntaxKind::TraitDecl => (Namespace::Type, SymbolKind::Trait, first_identifier(node)),
            SyntaxKind::FunctionDecl => {
                let Some(head) = node
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::FunctionHead)
                else {
                    return Ok(None);
                };
                let identifiers = head
                    .child_tokens()
                    .filter(|token| token.kind() == TokenKind::Identifier)
                    .collect::<Vec<_>>();
                if identifiers.len() != 1 {
                    return Ok(None);
                }
                (
                    Namespace::Value,
                    SymbolKind::Function,
                    identifiers.first().copied(),
                )
            }
            _ => return Ok(None),
        };
        let Some(token) = token else {
            return Ok(None);
        };
        let name = match normalized_name(token) {
            Ok(name) => name,
            Err(_) => {
                self.push_diagnostic(
                    file,
                    token.range(),
                    "E1005",
                    "`_` cannot name a module declaration",
                )?;
                return Ok(None);
            }
        };
        if is_reserved_unqualified(&name) {
            self.push_diagnostic(
                file,
                token.range(),
                "E1005",
                format!("`{name}` is reserved by the Tondo prelude"),
            )?;
            return Ok(None);
        }
        let visibility = if node
            .child_nodes()
            .any(|child| child.kind() == SyntaxKind::Visibility)
        {
            Visibility::Public
        } else {
            Visibility::Private
        };
        Ok(Some(DeclarationCandidate {
            module,
            namespace,
            name,
            kind,
            visibility,
            file,
            range: token.range(),
            generic_arity: declaration_generic_arity(node),
        }))
    }

    fn build_symbols(
        &mut self,
        mut declarations: Vec<DeclarationCandidate>,
        ordered_files: &[FileId],
    ) -> Result<(BTreeMap<ModuleId, ResolvedModule>, Vec<Symbol>), ResolveError> {
        declarations.sort_by_key(|candidate| self.candidate_key(candidate));
        let mut modules = BTreeMap::<ModuleId, ResolvedModule>::new();
        for file in ordered_files {
            let module = self.packages.module_for_file(self.sources, *file)?;
            modules
                .entry(module.clone())
                .or_insert_with(|| ResolvedModule {
                    id: module,
                    files: Vec::new(),
                    types: BTreeMap::new(),
                    values: BTreeMap::new(),
                })
                .files
                .push(*file);
        }

        let mut symbols = Vec::new();
        let mut index = 0;
        while index < declarations.len() {
            let start = index;
            let key = (
                declarations[index].module.clone(),
                declarations[index].namespace,
                declarations[index].name.clone(),
            );
            index += 1;
            while index < declarations.len()
                && declarations[index].module == key.0
                && declarations[index].namespace == key.1
                && declarations[index].name == key.2
            {
                index += 1;
            }
            let first = &declarations[start];
            let first_span = self.sources.span(first.file, first.range)?;
            for duplicate in &declarations[start + 1..index] {
                let span = self.sources.span(duplicate.file, duplicate.range)?;
                let diagnostic = Diagnostic::new(
                    Severity::Error,
                    DiagnosticCode::new("E1002")?,
                    format!(
                        "`{}` is already declared in the {} namespace of module `{}`",
                        duplicate.name,
                        duplicate.namespace,
                        duplicate.module.path()
                    ),
                    crate::diagnostics::PrimaryLocation::Source(span),
                )?
                .with_related(Related::new(
                    "first declaration with this name",
                    first_span,
                )?);
                self.push(duplicate.file, duplicate.range.start(), diagnostic)?;
            }

            let identity = self.packages.symbol_identity(
                first.module.clone(),
                first.namespace,
                DeclarationPath::single(first.name.clone()),
            )?;
            let id = SymbolId(
                u32::try_from(symbols.len()).expect("symbol count is bounded by syntax nodes"),
            );
            let symbol = Symbol {
                id,
                identity,
                name: first.name.clone(),
                kind: first.kind,
                visibility: first.visibility,
                span: first_span,
                generic_arity: first.generic_arity,
            };
            let module = modules
                .get_mut(&first.module)
                .expect("each declaration belongs to a parsed module");
            match first.namespace {
                Namespace::Type => module.types.insert(first.name.clone(), id),
                Namespace::Value => module.values.insert(first.name.clone(), id),
                Namespace::Module => unreachable!("module aliases are file-local"),
            };
            symbols.push(symbol);
        }
        Ok((modules, symbols))
    }

    fn diagnose_import_cycles(&mut self, edges: &[ImportEdge]) -> Result<(), ResolveError> {
        let mut adjacency = BTreeMap::<ModuleId, BTreeSet<ModuleId>>::new();
        for edge in edges {
            adjacency
                .entry(edge.from.clone())
                .or_default()
                .insert(edge.to.clone());
            adjacency.entry(edge.to.clone()).or_default();
        }
        for component in strongly_connected_components(&adjacency) {
            let cyclic = component.len() > 1
                || component.first().is_some_and(|module| {
                    adjacency
                        .get(module)
                        .is_some_and(|targets| targets.contains(module))
                });
            if !cyclic {
                continue;
            }
            let component = component.into_iter().collect::<BTreeSet<_>>();
            let mut internal = edges
                .iter()
                .filter(|edge| component.contains(&edge.from) && component.contains(&edge.to))
                .cloned()
                .collect::<Vec<_>>();
            internal.sort_by_key(|edge| self.edge_key(edge));
            let cycle = find_cycle(&component, &internal)
                .expect("a cyclic strongly connected component contains a cycle");
            let primary = &cycle[0];
            let span = self.sources.span(primary.file, primary.range)?;
            let mut path = cycle
                .iter()
                .map(|edge| edge.from.to_string())
                .collect::<Vec<_>>();
            path.push(primary.from.to_string());
            let mut diagnostic = Diagnostic::new(
                Severity::Error,
                DiagnosticCode::new("E1006")?,
                format!("module import cycle: {}", path.join(" -> ")),
                crate::diagnostics::PrimaryLocation::Source(span),
            )?;
            for edge in cycle.iter().skip(1) {
                diagnostic = diagnostic.with_related(Related::new(
                    format!("`{}` imports `{}` here", edge.from, edge.to),
                    self.sources.span(edge.file, edge.range)?,
                )?);
            }
            self.push(primary.file, primary.range.start(), diagnostic)?;
        }
        Ok(())
    }

    fn diagnose_import_declaration_conflicts(
        &mut self,
        modules: &BTreeMap<ModuleId, ResolvedModule>,
        files: &BTreeMap<FileId, FileResolution>,
        symbols: &[Symbol],
        ordered_files: &[FileId],
    ) -> Result<(), ResolveError> {
        for file in ordered_files {
            let module = self.packages.module_for_file(self.sources, *file)?;
            let Some(resolved_module) = modules.get(&module) else {
                continue;
            };
            for import in files
                .get(file)
                .expect("every parsed file has import resolution")
                .imports
                .values()
            {
                let mut conflicts = [Namespace::Type, Namespace::Value]
                    .into_iter()
                    .filter_map(|namespace| resolved_module.lookup(namespace, import.alias()))
                    .collect::<Vec<_>>();
                conflicts.sort();
                conflicts.dedup();
                if conflicts.is_empty() {
                    continue;
                }
                let mut diagnostic = Diagnostic::new(
                    Severity::Error,
                    DiagnosticCode::new("E1003")?,
                    format!(
                        "import alias `{}` would hide a declaration of module `{}`",
                        import.alias(),
                        module.path()
                    ),
                    crate::diagnostics::PrimaryLocation::Source(import.span()),
                )?;
                for conflict in conflicts {
                    diagnostic = diagnostic.with_related(Related::new(
                        "conflicting module declaration",
                        symbols[conflict.0 as usize].span(),
                    )?);
                }
                self.push(*file, import.span().range().start(), diagnostic)?;
            }
        }
        Ok(())
    }

    fn push_diagnostic(
        &mut self,
        file: FileId,
        range: TextRange,
        code: &str,
        message: impl Into<String>,
    ) -> Result<(), ResolveError> {
        let diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            crate::diagnostics::PrimaryLocation::Source(self.sources.span(file, range)?),
        )?;
        self.push(file, range.start(), diagnostic)
    }

    fn push(
        &mut self,
        file: FileId,
        offset: u32,
        diagnostic: Diagnostic,
    ) -> Result<(), ResolveError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(ResolveError::DiagnosticLimit { file, offset });
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }

    fn file_key(&self, file: FileId) -> (String, String, String) {
        let source = self
            .sources
            .get(file)
            .expect("parsed files belong to the source database");
        (
            source.source_id().as_str().to_owned(),
            source.module().as_str().to_owned(),
            source.path().as_str().to_owned(),
        )
    }

    fn candidate_key(
        &self,
        candidate: &DeclarationCandidate,
    ) -> (
        ModuleId,
        Namespace,
        Name,
        (String, String, String),
        u32,
        SymbolKind,
    ) {
        (
            candidate.module.clone(),
            candidate.namespace,
            candidate.name.clone(),
            self.file_key(candidate.file),
            candidate.range.start(),
            candidate.kind,
        )
    }

    fn edge_key(&self, edge: &ImportEdge) -> (ModuleId, ModuleId, (String, String, String), u32) {
        (
            edge.from.clone(),
            edge.to.clone(),
            self.file_key(edge.file),
            edge.range.start(),
        )
    }
}

fn first_identifier(node: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    node.descendant_tokens()
        .find(|token| token.kind() == TokenKind::Identifier)
}

fn declaration_generic_arity(node: SyntaxNodeRef<'_>) -> u32 {
    let owner = if node.kind() == SyntaxKind::FunctionDecl {
        node.child_nodes()
            .find(|child| child.kind() == SyntaxKind::FunctionHead)
            .unwrap_or(node)
    } else {
        node
    };
    owner
        .child_nodes()
        .filter(|child| child.kind() == SyntaxKind::GenericParams)
        .flat_map(SyntaxNodeRef::child_nodes)
        .filter(|child| child.kind() == SyntaxKind::GenericParam)
        .count()
        .try_into()
        .expect("generic parameter count is bounded by syntax nodes")
}

fn normalized_name(token: SyntaxTokenRef<'_>) -> Result<Name, NameError> {
    Name::new(
        token
            .token()
            .normalized_identifier()
            .expect("identifier tokens carry a normalized spelling"),
    )
}

fn is_reserved_unqualified(name: &Name) -> bool {
    matches!(
        name.as_str(),
        "Bool"
            | "Int"
            | "Float"
            | "Byte"
            | "Char"
            | "String"
            | "Unit"
            | "Never"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float32"
            | "Float64"
            | "Option"
            | "Result"
            | "Array"
            | "Map"
            | "Set"
            | "Range"
            | "Iterator"
            | "Ref"
            | "Pointer"
            | "Join"
            | "Command"
            | "Pipeline"
            | "Copy"
            | "Discard"
            | "Equatable"
            | "Key"
            | "Send"
            | "Share"
            | "Call"
            | "CallMut"
            | "CallOnce"
            | "Display"
            | "NumericConversionError"
            | "panic"
            | "assert"
            | "Self"
    )
}

fn import_error_message(error: &ImportResolutionError) -> String {
    match error {
        ImportResolutionError::EmptyPath => "import path is empty".to_owned(),
        ImportResolutionError::UnknownPackageAlias(alias) => {
            format!("`{alias}` is not the current package, `std`, or a dependency alias")
        }
        ImportResolutionError::MissingModulePath(alias) => {
            format!("import `{alias}` does not name a module inside the package")
        }
        ImportResolutionError::UnknownModule(module) => {
            format!("module `{module}` is not available for this target")
        }
        ImportResolutionError::MissingTargetCapability { module, capability } => format!(
            "module `{module}` is not available because target capability `{capability}` is missing"
        ),
        ImportResolutionError::UnknownFromPackage(package) => {
            format!("importing package `{package}` is not in the closed graph")
        }
    }
}

fn strongly_connected_components(
    adjacency: &BTreeMap<ModuleId, BTreeSet<ModuleId>>,
) -> Vec<Vec<ModuleId>> {
    let mut visited = BTreeSet::new();
    let mut finish = Vec::new();
    for start in adjacency.keys() {
        if visited.contains(start) {
            continue;
        }
        visited.insert(start.clone());
        let mut stack = vec![(start.clone(), false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                finish.push(node);
                continue;
            }
            stack.push((node.clone(), true));
            if let Some(targets) = adjacency.get(&node) {
                for target in targets.iter().rev() {
                    if visited.insert(target.clone()) {
                        stack.push((target.clone(), false));
                    }
                }
            }
        }
    }

    let mut reverse = adjacency
        .keys()
        .cloned()
        .map(|node| (node, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for (from, targets) in adjacency {
        for target in targets {
            reverse
                .entry(target.clone())
                .or_default()
                .insert(from.clone());
        }
    }
    visited.clear();
    let mut components = Vec::new();
    while let Some(start) = finish.pop() {
        if !visited.insert(start.clone()) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node.clone());
            if let Some(targets) = reverse.get(&node) {
                for target in targets.iter().rev() {
                    if visited.insert(target.clone()) {
                        stack.push(target.clone());
                    }
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort();
    components
}

fn find_cycle(component: &BTreeSet<ModuleId>, edges: &[ImportEdge]) -> Option<Vec<ImportEdge>> {
    let start = component.first()?.clone();
    let outgoing = edges
        .iter()
        .filter(|edge| edge.from == start)
        .cloned()
        .collect::<Vec<_>>();
    for first in outgoing {
        if first.to == start {
            return Some(vec![first]);
        }
        let mut queue = VecDeque::from([first.to.clone()]);
        let mut predecessor = BTreeMap::<ModuleId, ImportEdge>::new();
        let mut seen = BTreeSet::from([first.to.clone()]);
        while let Some(node) = queue.pop_front() {
            for edge in edges.iter().filter(|edge| edge.from == node) {
                if !component.contains(&edge.to) {
                    continue;
                }
                if edge.to == start {
                    let mut path = Vec::new();
                    let mut cursor = node.clone();
                    while cursor != first.to {
                        let previous = predecessor.get(&cursor)?.clone();
                        cursor = previous.from.clone();
                        path.push(previous);
                    }
                    path.reverse();
                    let mut cycle = vec![first.clone()];
                    cycle.extend(path);
                    cycle.push(edge.clone());
                    return Some(cycle);
                }
                if seen.insert(edge.to.clone()) {
                    predecessor.insert(edge.to.clone(), edge.clone());
                    queue.push_back(edge.to.clone());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::diagnostics::DiagnosticBag;
    use crate::package::{Edition, PackageAlias, PackageId, PackageNode};
    use crate::source::{LogicalPath, ModulePath, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    use super::*;

    fn resolve_sources(
        inputs: &[(&str, &str, &str)],
        modules: &[&str],
    ) -> (SourceDatabase, ResolveOutput) {
        let mut sources = SourceDatabase::new();
        let mut parsed = Vec::new();
        for (module, path, source) in inputs {
            let file = sources
                .add(SourceInput::virtual_file(
                    SourceId::new("source:app").unwrap(),
                    ModulePath::new(module).unwrap(),
                    LogicalPath::new(path).unwrap(),
                    Arc::<[u8]>::from(source.as_bytes()),
                ))
                .unwrap();
            let lexed = lex(&sources, file, LexMode::Module).unwrap();
            let syntax = parse(
                &sources,
                file,
                lexed,
                ParseMode::Module,
                ParseLimits::default(),
            )
            .unwrap();
            assert!(syntax.diagnostics().is_empty(), "{source}");
            parsed.push((file, syntax));
        }
        let graph = PackageGraph::new(
            PackageId::new("pkg:app").unwrap(),
            PackageId::new("pkg:std").unwrap(),
            [
                PackageNode::new(
                    PackageId::new("pkg:app").unwrap(),
                    SourceId::new("source:app").unwrap(),
                    PackageAlias::new("app").unwrap(),
                    Edition::V0_1,
                    modules
                        .iter()
                        .map(|module| ModulePath::new(module).unwrap()),
                    [],
                )
                .unwrap(),
                PackageNode::new(
                    PackageId::new("pkg:std").unwrap(),
                    SourceId::new("source:std").unwrap(),
                    PackageAlias::new("tondoStd").unwrap(),
                    Edition::V0_1,
                    [],
                    [],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let output = resolve(
            &graph,
            &sources,
            parsed.iter().map(|(file, parsed)| (*file, parsed)),
            100,
        )
        .unwrap();
        (sources, output)
    }

    fn codes(sources: &SourceDatabase, output: ResolveOutput) -> Vec<String> {
        let (_, diagnostics) = output.into_parts();
        let mut bag = DiagnosticBag::new();
        bag.extend(diagnostics);
        bag.resolve("0.1", sources)
            .unwrap()
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.code().to_owned())
            .collect()
    }

    #[test]
    fn declarations_from_multiple_files_share_namespaces_and_stable_identity() {
        let (sources, output) = resolve_sources(
            &[
                ("main", "z.to", "type User = {\n    id: Int\n}\n"),
                (
                    "main",
                    "a.to",
                    "fn User(value: Int): Int {\n    value\n}\nconst Limit = 3\n",
                ),
            ],
            &["main"],
        );
        let actual = codes(&sources, output);
        assert!(actual.is_empty(), "{actual:?}");

        let (sources, output) = resolve_sources(
            &[
                ("main", "a.to", "type UserId = Int\n"),
                (
                    "main",
                    "b.to",
                    "fn UserId(value: Int): Int {\n    value\n}\n",
                ),
            ],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1002"]);
    }

    #[test]
    fn imports_report_position_path_alias_and_complete_cycles() {
        let (sources, output) = resolve_sources(
            &[
                ("a", "a.to", "import app.b\nfn a() {}\n"),
                ("b", "b.to", "import app.a\nfn b() {}\n"),
                (
                    "main",
                    "main.to",
                    "fn main() {}\nimport app.missing\nimport app.a as String\n",
                ),
            ],
            &["a", "b", "main"],
        );
        assert_eq!(
            codes(&sources, output),
            ["E1006", "E1007", "E1008", "E1007", "E1005"]
        );
    }

    #[test]
    fn duplicate_import_alias_is_file_local() {
        let (sources, output) = resolve_sources(
            &[
                (
                    "main",
                    "main.to",
                    "import app.a as dependency\nimport app.b as dependency\nfn main() {}\n",
                ),
                ("a", "a.to", "fn a() {}\n"),
                ("b", "b.to", "fn b() {}\n"),
            ],
            &["main", "a", "b"],
        );
        assert_eq!(codes(&sources, output), ["E1002"]);
    }

    #[test]
    fn lexical_scopes_resolve_after_initialization_and_reject_shadowing() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "fn transform(value: Int): Int {\n    let before = before\n    let current = value\n    if true {\n        let value = current\n        value\n    } else {\n        let sibling = current\n        sibling\n    }\n    let sibling = current\n    sibling\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1001", "E1003"]);
    }

    #[test]
    fn duplicate_bindings_are_local_to_one_scope_and_sibling_scopes_can_reuse_names() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "fn inspect(flag: Bool): Int {\n    if flag {\n        let item = 1\n        item\n    } else {\n        let item = 2\n        item\n    }\n    let result = 3\n    let result = 4\n    result\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1002"]);
    }

    #[test]
    fn type_and_value_namespaces_are_separate_but_locals_cannot_hide_values() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type Token = {\n    value: Int\n}\nfn Token(): Int {\n    1\n}\nfn consume(item: Token): Int {\n    Token()\n}\nfn conflict(Token: Int): Int {\n    Token\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1003"]);
    }

    #[test]
    fn imported_modules_are_file_local_and_enforce_declaration_visibility() {
        let (sources, output) = resolve_sources(
            &[
                (
                    "api",
                    "api.to",
                    "pub type PublicValue = Int\ntype PrivateValue = Int\npub fn publicValue(): Int {\n    1\n}\nfn privateValue(): Int {\n    2\n}\n",
                ),
                (
                    "main",
                    "main.to",
                    "import app.api\nfn valid(value: api.PublicValue): Int {\n    api.publicValue()\n}\nfn invalid(value: api.PrivateValue): Int {\n    api.privateValue()\n}\n",
                ),
            ],
            &["api", "main"],
        );
        assert_eq!(codes(&sources, output), ["E1501", "E1501"]);
    }

    #[test]
    fn imports_reserved_names_and_unknown_names_have_stable_diagnostics() {
        let (sources, output) = resolve_sources(
            &[
                (
                    "dependency",
                    "dependency.to",
                    "pub fn read(): Int {\n    1\n}\n",
                ),
                (
                    "main",
                    "main.to",
                    "import app.dependency\nfn inspect[String](dependency: Int, value: Missing): Int {\n    missing(value)\n}\n",
                ),
            ],
            &["dependency", "main"],
        );
        assert_eq!(
            codes(&sources, output),
            ["E1005", "E1003", "E1001", "E1001"]
        );
    }

    #[test]
    fn module_resolution_and_symbol_ids_ignore_input_file_order() {
        let first = [
            ("main", "z.to", "fn first(): Int {\n    second()\n}\n"),
            ("main", "a.to", "fn second(): Int {\n    2\n}\n"),
        ];
        let second = [first[1], first[0]];
        let (first_sources, first_output) = resolve_sources(&first, &["main"]);
        let (second_sources, second_output) = resolve_sources(&second, &["main"]);
        assert!(first_output.diagnostics().is_empty());
        assert!(second_output.diagnostics().is_empty());

        let snapshot = |output: &ResolveOutput| {
            output
                .program()
                .symbols()
                .map(|symbol| {
                    (
                        symbol.id().index(),
                        symbol.identity().canonical_name(),
                        symbol.kind(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(snapshot(&first_output), snapshot(&second_output));
        assert_eq!(first_sources.len(), second_sources.len());
    }

    #[test]
    fn self_is_contextual_in_traits_implementations_and_inherent_methods() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "trait Compare {\n    fn compare(self, other: Self): Self\n}\ntype Item = {\n    value: Int\n}\nimpl Compare for Item {\n    fn compare(self, other: Self): Self {\n        other\n    }\n}\nfn Item.copy(self): Self {\n    self\n}\ntype Invalid = {\n    value: Self\n}\nfn invalid(value: Self) {}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1001", "E1001"]);
    }

    #[test]
    fn preliminary_brackets_resolve_generic_type_arguments_and_value_indices() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type User = {\n    id: Int\n}\nfn identity[T](value: T): T {\n    value\n}\nfn use(user: User, users: Array[User], index: Int): User {\n    let selected = users[index]\n    identity[User](selected)\n}\n",
            )],
            &["main"],
        );
        let actual = codes(&sources, output);
        assert!(actual.is_empty(), "{actual:?}");
    }

    #[test]
    fn member_namespace_records_fields_variants_methods_and_newtype_value() {
        let (_sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "pub type Event = {\n    type: String\n    priv secret: Int\n}\ntype UserId = Int\npub enum Choice {\n    Empty\n    Item { value: Int }\n}\npub trait Factory {\n    fn create(): Self\n    fn label(self): String\n}\npub fn Event.label(self): String {\n    self.type\n}\npub fn Event.create(value: String): Event {\n    Event { type: value, secret: 0 }\n}\n",
            )],
            &["main"],
        );
        assert!(
            output.diagnostics().is_empty(),
            "{:#?}",
            output.diagnostics()
        );
        assert_eq!(output.program().members().count(), 10);

        let symbol = |name: &str| {
            output
                .program()
                .symbols()
                .find(|symbol| symbol.name().as_str() == name)
                .expect("test symbol exists")
                .id()
        };
        let member = |owner, name: &str| {
            let ids = output
                .program()
                .lookup_members(owner, &MemberName::new(name).unwrap())
                .expect("test member exists");
            assert_eq!(ids.len(), 1, "{name}");
            output.program().member(ids[0]).unwrap()
        };

        let event = symbol("Event");
        assert_eq!(
            member(MemberOwner::Type(event), "type").kind(),
            MemberKind::RecordField
        );
        assert_eq!(
            member(MemberOwner::Type(event), "type").visibility(),
            Visibility::Public
        );
        assert_eq!(
            member(MemberOwner::Type(event), "secret").visibility(),
            Visibility::Private
        );
        assert_eq!(
            member(MemberOwner::Type(event), "label").kind(),
            MemberKind::InherentMethod
        );
        assert_eq!(
            member(MemberOwner::Type(event), "create").kind(),
            MemberKind::AssociatedFunction
        );

        let newtype_value = member(MemberOwner::Type(symbol("UserId")), "value");
        assert_eq!(newtype_value.kind(), MemberKind::NewtypeValue);
        assert!(newtype_value.is_synthetic());
        assert_eq!(newtype_value.visibility(), Visibility::Private);

        let item = member(MemberOwner::Type(symbol("Choice")), "Item");
        assert_eq!(item.kind(), MemberKind::EnumVariant);
        let item_value = member(MemberOwner::Variant(item.id()), "value");
        assert_eq!(item_value.kind(), MemberKind::VariantField);
        assert_eq!(item_value.visibility(), Visibility::Public);

        assert_eq!(
            member(MemberOwner::Type(symbol("Factory")), "create").kind(),
            MemberKind::TraitAssociatedFunction
        );
        assert_eq!(
            member(MemberOwner::Type(symbol("Factory")), "label").kind(),
            MemberKind::TraitMethod
        );
    }

    #[test]
    fn member_conflicts_and_redundant_private_fields_are_diagnosed() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type PrivateRecord = {\n    priv hidden: Int\n}\ntype Record = {\n    value: Int\n    value: String\n}\nfn Record.value(self) {}\nenum Choice {\n    Item\n    Item(Int)\n}\nfn Choice.Item() {}\ntrait Action {\n    fn act()\n    fn act()\n}\nimpl Action for Record {\n    fn act() {}\n    fn act() {}\n}\n",
            )],
            &["main"],
        );
        assert_eq!(
            codes(&sources, output),
            [
                "E1115", "E1505", "E1505", "E1505", "E1505", "E1505", "E1002"
            ]
        );
    }

    #[test]
    fn inherent_method_owner_must_be_local_nominal_and_public_when_exported() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "alias Number = Int\ntype Private = Int\nfn Number.invalid() {}\nfn Int.invalid() {}\npub fn Private.exposed(self) {}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1504", "E1504", "E1503"]);
    }

    #[test]
    fn every_public_signature_surface_rejects_private_types() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type Secret = Int\ntrait Hidden {}\npub const Default: Secret = Secret(0)\npub type Identifier = Secret\npub alias Alias = Secret\npub type Record = {\n    secret: Secret\n}\npub enum Choice {\n    One(Secret)\n    Two { secret: Secret }\n}\npub trait Public[T: Hidden] {\n    fn convert(value: Secret): Secret\n}\npub fn expose[T: Hidden](value: Secret): Secret {\n    value\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), vec!["E1503"; 12]);
    }

    #[test]
    fn private_fields_and_function_bodies_are_not_public_api_positions() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type Secret = Int\npub type Wrapper = {\n    value: Int\n    priv secret: Secret\n}\npub fn answer(): Int {\n    let hidden: Secret = Secret(42)\n    hidden.value\n}\n",
            )],
            &["main"],
        );
        let actual = codes(&sources, output);
        assert!(actual.is_empty(), "{actual:?}");
    }

    #[test]
    fn inaccessible_imported_type_uses_e1501_instead_of_public_api_duplicate() {
        let (sources, output) = resolve_sources(
            &[
                ("api", "api.to", "type Secret = Int\n"),
                (
                    "main",
                    "main.to",
                    "import app.api\npub fn expose(value: api.Secret): Int {\n    1\n}\n",
                ),
            ],
            &["api", "main"],
        );
        assert_eq!(codes(&sources, output), ["E1501"]);
    }

    #[test]
    fn assignments_receivers_and_record_shorthand_are_name_resolved() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type Point = {\n    x: Int\n    type: String\n}\nfn Point.update(mut self, x: Int) {\n    self.x = x\n}\nfn Point.associated() {\n    self.x = 1\n}\nfn build(x: Int): Point {\n    missing = 1\n    Point { x, type }\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1001", "E1001", "E1115"]);
    }

    #[test]
    fn qualified_type_value_collision_is_ambiguous_but_newtype_pair_is_not() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "type Name = {\n    field: Int\n}\nfn Name(): Int {\n    1\n}\ntype Identifier = Int\nfn inspect(identifier: Identifier): Int {\n    Name.field\n    identifier.value\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1004"]);
    }

    #[test]
    fn for_match_and_closure_bindings_exist_only_in_their_lexical_regions() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "enum Choice {\n    Item(Int)\n}\nfn inspect(items: Array[Int], choice: Choice): Int {\n    for item in items {\n        item\n    }\n    item\n    let transform = (value: Int): Int {\n        value\n    }\n    value\n    let matched = match choice {\n        Choice.Item(payload) => payload\n    }\n    payload\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1001", "E1001", "E1001"]);
    }

    #[test]
    fn for_match_and_closure_parameters_cannot_shadow_visible_bindings() {
        let (sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "enum Choice {\n    Item(Int)\n}\nfn inspect(items: Array[Int], choice: Choice): Int {\n    let value = 0\n    for value in items {\n        value\n    }\n    let transform = (value: Int): Int {\n        value\n    }\n    match choice {\n        Choice.Item(value) => value\n    }\n}\n",
            )],
            &["main"],
        );
        assert_eq!(codes(&sources, output), ["E1003", "E1003", "E1003"]);
    }

    #[test]
    fn file_local_import_alias_cannot_hide_any_module_declaration_namespace() {
        let (sources, output) = resolve_sources(
            &[
                (
                    "main",
                    "declarations.to",
                    "type dependency = {\n    value: Int\n}\nfn dependency(): Int {\n    1\n}\n",
                ),
                (
                    "main",
                    "imports.to",
                    "import app.external as dependency\nfn use(): Int {\n    dependency.read()\n}\n",
                ),
                (
                    "external",
                    "external.to",
                    "pub fn read(): Int {\n    1\n}\n",
                ),
            ],
            &["main", "external"],
        );
        assert_eq!(codes(&sources, output), ["E1003"]);
    }

    #[test]
    fn generic_arguments_inside_path_types_are_resolved_in_their_binder_scope() {
        let (_sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "alias Pair[T] = (T, T)\ntype Wrapped[T] = { value: Pair[T] }\n",
            )],
            &["main"],
        );
        assert!(output.diagnostics().is_empty());
        let locals = output
            .program()
            .references()
            .filter_map(|reference| match reference.entity() {
                ResolvedEntity::Name(ResolvedName::Local(local)) => Some(local.index()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(locals, [0, 0, 1]);
    }

    #[test]
    fn generic_arguments_inside_expression_specializations_use_the_callable_binder() {
        let (_sources, output) = resolve_sources(
            &[(
                "main",
                "main.to",
                "fn identity[T](value: T): T {\n    value\n}\nfn forward[T](value: T): T {\n    identity[T](value)\n}\nfn wrap[T](value: T): T? {\n    identity[T?](some(value))\n}\nfn nest[T](value: T): Array[T] {\n    identity[Array[T]]([value])\n}\n",
            )],
            &["main"],
        );
        assert!(output.diagnostics().is_empty());
    }
}
