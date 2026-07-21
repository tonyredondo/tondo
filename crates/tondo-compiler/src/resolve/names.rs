use std::collections::BTreeMap;

use crate::diagnostics::{Diagnostic, DiagnosticCode, PrimaryLocation, Related, Severity};
use crate::package::{ModuleId, Name, Namespace, PackageGraph};
use crate::source::{FileId, SourceDatabase, Span, TextRange};
use crate::syntax::{Parsed, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};

use super::{
    FileResolution, LocalBinding, LocalId, LocalKind, ResolveError, ResolvedEntity, ResolvedName,
    ResolvedProgram, ResolvedReference, SymbolId, Visibility, is_reserved_unqualified,
    normalized_name,
};

#[derive(Debug, Clone)]
struct ScopeEntry {
    id: LocalId,
    span: Span,
}

#[derive(Debug, Default)]
struct Scope {
    types: BTreeMap<Name, ScopeEntry>,
    values: BTreeMap<Name, ScopeEntry>,
}

pub(super) fn resolve_names<'a>(
    packages: &'a PackageGraph,
    sources: &'a SourceDatabase,
    parsed: &BTreeMap<FileId, &'a Parsed>,
    ordered_files: &[FileId],
    program: &mut ResolvedProgram,
    diagnostics: &mut Vec<Diagnostic>,
    max_diagnostics: usize,
) -> Result<(), ResolveError> {
    for file in ordered_files {
        let syntax = parsed[file];
        let module = packages.module_for_file(sources, *file)?;
        let file_resolution = program
            .files
            .get(file)
            .expect("every parsed file has an import resolution")
            .clone();
        let mut resolver = NameResolver {
            sources,
            file: *file,
            module,
            file_resolution,
            program,
            diagnostics,
            max_diagnostics,
            scopes: Vec::new(),
            contextual_self: false,
            receiver_available: false,
        };
        resolver.resolve_file(syntax.cst().root_node())?;
    }
    Ok(())
}

struct NameResolver<'a> {
    sources: &'a SourceDatabase,
    file: FileId,
    module: ModuleId,
    file_resolution: FileResolution,
    program: &'a mut ResolvedProgram,
    diagnostics: &'a mut Vec<Diagnostic>,
    max_diagnostics: usize,
    scopes: Vec<Scope>,
    contextual_self: bool,
    receiver_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValuePathShape {
    Plain,
    Bracketed,
    Qualified,
}

impl NameResolver<'_> {
    fn resolve_file(&mut self, root: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        self.scopes.push(Scope::default());
        for child in root.child_nodes() {
            match child.kind() {
                SyntaxKind::ImportDecl => {}
                SyntaxKind::FunctionDecl => self.resolve_function(child)?,
                SyntaxKind::TypeDecl
                | SyntaxKind::AliasDecl
                | SyntaxKind::EnumDecl
                | SyntaxKind::TraitDecl
                | SyntaxKind::ImplDecl => self.resolve_type_owner(child)?,
                SyntaxKind::ConstDecl => self.walk(child, None)?,
                _ => self.walk(child, None)?,
            }
        }
        self.scopes.pop();
        Ok(())
    }

    fn resolve_type_owner(&mut self, node: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let previous_contextual_self = self.contextual_self;
        self.contextual_self = matches!(node.kind(), SyntaxKind::TraitDecl | SyntaxKind::ImplDecl);
        self.scopes.push(Scope::default());
        self.declare_generic_parameters(node)?;
        for child in node.child_nodes() {
            match child.kind() {
                SyntaxKind::GenericParams => self.walk(child, Some(node.kind()))?,
                SyntaxKind::TraitMethod | SyntaxKind::ImplementationMethod => {
                    self.resolve_callable(child, None)?
                }
                _ => self.walk(child, Some(node.kind()))?,
            }
        }
        self.scopes.pop();
        self.contextual_self = previous_contextual_self;
        Ok(())
    }

    fn resolve_function(&mut self, node: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let head = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::FunctionHead);
        let previous_contextual_self = self.contextual_self;
        self.contextual_self = head.is_some_and(|head| {
            head.child_tokens()
                .any(|token| token.kind() == TokenKind::Dot)
        });
        let result = self.resolve_callable(node, head);
        self.contextual_self = previous_contextual_self;
        result
    }

    fn resolve_callable(
        &mut self,
        node: SyntaxNodeRef<'_>,
        head: Option<SyntaxNodeRef<'_>>,
    ) -> Result<(), ResolveError> {
        let previous_receiver = self.receiver_available;
        self.receiver_available = has_receiver(node);
        self.scopes.push(Scope::default());
        if let Some(head) = head {
            let names = head
                .child_tokens()
                .filter(|token| token.kind() == TokenKind::Identifier)
                .collect::<Vec<_>>();
            if names.len() > 1 {
                self.resolve_single_token(names[0], Namespace::Type)?;
            }
            self.declare_generic_parameters(head)?;
            for generic in head
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::GenericParams)
            {
                self.walk(generic, Some(SyntaxKind::FunctionHead))?;
            }
        } else {
            self.declare_generic_parameters(node)?;
            for generic in node
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::GenericParams)
            {
                self.walk(generic, Some(node.kind()))?;
            }
        }

        if let Some(parameters) = node
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ParameterList)
        {
            for parameter in parameters
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::Parameter)
            {
                for child in parameter.child_nodes() {
                    self.walk(child, Some(SyntaxKind::Parameter))?;
                }
            }
            self.scopes.push(Scope::default());
            let parameter_tokens = parameters
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::Parameter)
                .filter_map(parameter_name_token)
                .collect::<Vec<_>>();
            self.declare_tokens(parameter_tokens, Namespace::Value, LocalKind::Parameter)?;
        } else {
            self.scopes.push(Scope::default());
        }

        for child in node.child_nodes() {
            match child.kind() {
                SyntaxKind::FunctionHead
                | SyntaxKind::GenericParams
                | SyntaxKind::ParameterList => {}
                SyntaxKind::Block => self.resolve_block(child)?,
                _ => self.walk(child, Some(node.kind()))?,
            }
        }
        self.scopes.pop();
        self.scopes.pop();
        self.receiver_available = previous_receiver;
        Ok(())
    }

    fn declare_generic_parameters(&mut self, owner: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let tokens = owner
            .child_nodes()
            .filter(|child| child.kind() == SyntaxKind::GenericParams)
            .flat_map(|generic| {
                generic
                    .child_nodes()
                    .filter(|child| child.kind() == SyntaxKind::GenericParam)
                    .filter_map(|parameter| {
                        parameter
                            .child_tokens()
                            .find(|token| token.kind() == TokenKind::Identifier)
                    })
            })
            .collect::<Vec<_>>();
        self.declare_tokens(tokens, Namespace::Type, LocalKind::GenericParameter)
    }

    fn resolve_block(&mut self, block: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        self.scopes.push(Scope::default());
        for item in block.child_nodes() {
            match item.kind() {
                SyntaxKind::BindingDecl => self.resolve_binding(item)?,
                SyntaxKind::ForStmt => self.resolve_for(item)?,
                SyntaxKind::MatchExpr => self.resolve_match(item)?,
                _ => self.walk(item, Some(SyntaxKind::Block))?,
            }
        }
        self.scopes.pop();
        Ok(())
    }

    fn resolve_binding(&mut self, binding: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let pattern = binding.child_nodes().find(|child| is_pattern(child.kind()));
        for child in binding.child_nodes() {
            if Some(child) != pattern {
                self.walk(child, Some(SyntaxKind::BindingDecl))?;
            }
        }
        if let Some(pattern) = pattern {
            self.resolve_pattern_references(pattern)?;
            let mut tokens = Vec::new();
            collect_pattern_bindings(pattern, &mut tokens);
            self.declare_tokens(tokens, Namespace::Value, LocalKind::Binding)?;
        }
        Ok(())
    }

    fn resolve_for(&mut self, statement: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let header = statement
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ForHeader);
        let pattern =
            header.and_then(|header| header.child_nodes().find(|child| is_pattern(child.kind())));
        if let Some(header) = header {
            for child in header.child_nodes() {
                if Some(child) != pattern {
                    self.walk(child, Some(SyntaxKind::ForHeader))?;
                }
            }
        }
        self.scopes.push(Scope::default());
        if let Some(pattern) = pattern {
            self.resolve_pattern_references(pattern)?;
            let mut tokens = Vec::new();
            collect_pattern_bindings(pattern, &mut tokens);
            self.declare_tokens(tokens, Namespace::Value, LocalKind::ForPattern)?;
        }
        if let Some(block) = statement
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::Block)
        {
            self.resolve_block(block)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn resolve_match(&mut self, expression: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        for child in expression.child_nodes() {
            if child.kind() == SyntaxKind::MatchArm {
                self.resolve_match_arm(child)?;
            } else {
                self.walk(child, Some(SyntaxKind::MatchExpr))?;
            }
        }
        Ok(())
    }

    fn resolve_match_arm(&mut self, arm: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        self.scopes.push(Scope::default());
        let pattern = arm.child_nodes().find(|child| is_pattern(child.kind()));
        if let Some(pattern) = pattern {
            self.resolve_pattern_references(pattern)?;
            let mut tokens = Vec::new();
            collect_pattern_bindings(pattern, &mut tokens);
            self.declare_tokens(tokens, Namespace::Value, LocalKind::Pattern)?;
        }
        for child in arm.child_nodes() {
            if Some(child) != pattern {
                self.walk(child, Some(SyntaxKind::MatchArm))?;
            }
        }
        self.scopes.pop();
        Ok(())
    }

    fn resolve_closure(&mut self, closure: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let parameters = closure
            .child_nodes()
            .find(|child| child.kind() == SyntaxKind::ClosureParameterList);
        if let Some(parameters) = parameters {
            for parameter in parameters
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::ClosureParameter)
            {
                for child in parameter.child_nodes() {
                    self.walk(child, Some(SyntaxKind::ClosureParameter))?;
                }
            }
        }
        self.scopes.push(Scope::default());
        if let Some(parameters) = parameters {
            let tokens = parameters
                .child_nodes()
                .filter(|child| child.kind() == SyntaxKind::ClosureParameter)
                .filter_map(parameter_name_token)
                .collect::<Vec<_>>();
            self.declare_tokens(tokens, Namespace::Value, LocalKind::ClosureParameter)?;
        }
        for child in closure.child_nodes() {
            if Some(child) == parameters {
                continue;
            }
            if child.kind() == SyntaxKind::Block {
                self.resolve_block(child)?;
            } else {
                self.walk(child, Some(SyntaxKind::ClosureExpr))?;
            }
        }
        self.scopes.pop();
        Ok(())
    }

    fn walk(
        &mut self,
        node: SyntaxNodeRef<'_>,
        parent: Option<SyntaxKind>,
    ) -> Result<(), ResolveError> {
        match node.kind() {
            SyntaxKind::ImportDecl | SyntaxKind::ModulePath => return Ok(()),
            SyntaxKind::Block => return self.resolve_block(node),
            SyntaxKind::BindingDecl => return self.resolve_binding(node),
            SyntaxKind::ForStmt => return self.resolve_for(node),
            SyntaxKind::MatchExpr => return self.resolve_match(node),
            SyntaxKind::ClosureExpr => return self.resolve_closure(node),
            SyntaxKind::BracketPostfix => return self.resolve_preliminary_bracket(node),
            SyntaxKind::SelfExpr => return self.resolve_receiver(node),
            SyntaxKind::Lvalue => return self.resolve_lvalue(node),
            SyntaxKind::RecordInitializer => return self.resolve_record_initializer(node),
            SyntaxKind::RecordLikeExpr => {
                for child in node.child_nodes() {
                    if child.kind() == SyntaxKind::PathExpr {
                        self.resolve_path(child, Namespace::Type)?;
                        self.walk_path_arguments(child)?;
                    } else {
                        self.walk(child, Some(SyntaxKind::RecordLikeExpr))?;
                    }
                }
                return Ok(());
            }
            SyntaxKind::PathType | SyntaxKind::TypePath => {
                self.resolve_path(node, Namespace::Type)?;
                self.walk_path_arguments(node)?;
                return Ok(());
            }
            SyntaxKind::PathExpr => {
                let namespace = if parent == Some(SyntaxKind::RecordLikeExpr) {
                    Namespace::Type
                } else {
                    Namespace::Value
                };
                self.resolve_path(node, namespace)?;
                self.walk_path_arguments(node)?;
                return Ok(());
            }
            SyntaxKind::ConstructorPattern
            | SyntaxKind::RecordPattern
            | SyntaxKind::QualifiedValuePattern => {
                return self.resolve_pattern_references(node);
            }
            _ => {}
        }
        for child in node.child_nodes() {
            self.walk(child, Some(node.kind()))?;
        }
        Ok(())
    }

    fn walk_path_arguments(&mut self, path: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let lexical_path = if path.kind() == SyntaxKind::PathType {
            path.child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypePath)
                .expect("a parsed path type contains its type path")
        } else {
            path
        };
        for child in lexical_path.child_nodes() {
            if matches!(
                child.kind(),
                SyntaxKind::GenericArgs | SyntaxKind::BracketPostfix
            ) {
                self.walk(child, Some(lexical_path.kind()))?;
            }
        }
        Ok(())
    }

    fn resolve_receiver(&mut self, node: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let Some(token) = node
            .child_tokens()
            .find(|token| token.kind() == TokenKind::SelfKw)
        else {
            return Ok(());
        };
        if self.receiver_available {
            self.record_reference(token.range(), ResolvedEntity::Name(ResolvedName::Receiver));
        } else {
            self.emit(
                token.range(),
                "E1001",
                "receiver `self` is only available in a method that declares it",
                None,
            )?;
        }
        Ok(())
    }

    fn resolve_lvalue(&mut self, lvalue: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        if let Some(root) = lvalue
            .child_tokens()
            .find(|token| matches!(token.kind(), TokenKind::Identifier | TokenKind::SelfKw))
        {
            if root.kind() == TokenKind::SelfKw {
                if self.receiver_available {
                    self.record_reference(
                        root.range(),
                        ResolvedEntity::Name(ResolvedName::Receiver),
                    );
                } else {
                    self.emit(
                        root.range(),
                        "E1001",
                        "receiver `self` is only available in a method that declares it",
                        None,
                    )?;
                }
            } else {
                self.resolve_single_token(root, Namespace::Value)?;
            }
        }
        for child in lvalue.child_nodes() {
            if child.kind() == SyntaxKind::BracketPostfix {
                self.resolve_preliminary_bracket(child)?;
            }
        }
        Ok(())
    }

    fn resolve_record_initializer(
        &mut self,
        initializer: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        let expressions = initializer.child_nodes().collect::<Vec<_>>();
        if expressions.is_empty() {
            let Some(token) = initializer
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier || token.kind().is_keyword())
            else {
                return Ok(());
            };
            if token.kind() == TokenKind::Identifier {
                self.resolve_single_token(token, Namespace::Value)?;
            } else {
                self.emit(
                    token.range(),
                    "E1115",
                    "a keyword field requires an explicit `field: value` initializer",
                    None,
                )?;
            }
            return Ok(());
        }
        for expression in expressions {
            self.walk(expression, Some(SyntaxKind::RecordInitializer))?;
        }
        Ok(())
    }

    fn resolve_preliminary_bracket(
        &mut self,
        bracket: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        for item in bracket.child_nodes() {
            if item.kind() == SyntaxKind::BracketItem {
                let children = item.child_nodes().collect::<Vec<_>>();
                if children.len() == 1 && children[0].kind() == SyntaxKind::PathExpr {
                    self.resolve_either_path(children[0])?;
                    self.walk_path_arguments(children[0])?;
                } else {
                    self.walk(item, Some(SyntaxKind::BracketPostfix))?;
                }
            } else {
                self.walk(item, Some(SyntaxKind::BracketPostfix))?;
            }
        }
        Ok(())
    }

    fn resolve_either_path(&mut self, path: SyntaxNodeRef<'_>) -> Result<(), ResolveError> {
        let tokens = path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let Some(first_token) = tokens.first().copied() else {
            return Ok(());
        };
        let Ok(first) = normalized_name(first_token) else {
            self.emit(
                first_token.range(),
                "E1001",
                "the discard `_` is not a value or type name",
                None,
            )?;
            return Ok(());
        };

        if let Some(import) = self.file_resolution.imports.get(&first).cloned() {
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Module(import.module.clone()),
            );
            let Some(member_token) = tokens.get(1).copied() else {
                self.emit(
                    path.range(),
                    "E1001",
                    format!("module `{first}` must be followed by a declaration name"),
                    None,
                )?;
                return Ok(());
            };
            let member = normalized_name(member_token)
                .expect("qualified path identifiers are ordinary names");
            return self.resolve_either_module_member(&import.module, member, member_token);
        }

        let type_name = self.lookup_name(Namespace::Type, &first);
        let value_name = self.lookup_name(Namespace::Value, &first);
        match (type_name, value_name) {
            (Some(type_name), Some(value_name)) => self.record_reference(
                first_token.range(),
                ResolvedEntity::ContextualCandidates {
                    type_name,
                    value_name,
                },
            ),
            (Some(name), None) | (None, Some(name)) => {
                self.record_reference(first_token.range(), ResolvedEntity::Name(name));
            }
            (None, None) => {
                self.emit(
                    first_token.range(),
                    "E1001",
                    format!("unknown value or type name `{first}`"),
                    None,
                )?;
            }
        }
        Ok(())
    }

    fn resolve_either_module_member(
        &mut self,
        module: &ModuleId,
        name: Name,
        token: SyntaxTokenRef<'_>,
    ) -> Result<(), ResolveError> {
        let Some(resolved_module) = self.program.module(module) else {
            self.record_reference(
                token.range(),
                ResolvedEntity::ContextualCandidates {
                    type_name: ResolvedName::External {
                        module: module.clone(),
                        namespace: Namespace::Type,
                        name: name.clone(),
                    },
                    value_name: ResolvedName::External {
                        module: module.clone(),
                        namespace: Namespace::Value,
                        name,
                    },
                },
            );
            return Ok(());
        };
        let type_symbol = resolved_module.lookup(Namespace::Type, &name);
        let value_symbol = resolved_module.lookup(Namespace::Value, &name);
        match (type_symbol, value_symbol) {
            (Some(type_symbol), Some(value_symbol)) => {
                self.check_symbol_visibility(module, name.clone(), token, type_symbol)?;
                self.check_symbol_visibility(module, name, token, value_symbol)?;
                self.record_reference(
                    token.range(),
                    ResolvedEntity::ContextualCandidates {
                        type_name: ResolvedName::Symbol(type_symbol),
                        value_name: ResolvedName::Symbol(value_symbol),
                    },
                );
            }
            (Some(symbol), None) | (None, Some(symbol)) => {
                self.check_symbol_visibility(module, name, token, symbol)?;
                self.record_reference(
                    token.range(),
                    ResolvedEntity::Name(ResolvedName::Symbol(symbol)),
                );
            }
            (None, None) => {
                self.emit(
                    token.range(),
                    "E1001",
                    format!(
                        "module `{}` has no type or value named `{name}`",
                        module.path()
                    ),
                    None,
                )?;
            }
        }
        Ok(())
    }

    fn resolve_pattern_references(
        &mut self,
        pattern: SyntaxNodeRef<'_>,
    ) -> Result<(), ResolveError> {
        match pattern.kind() {
            SyntaxKind::RecordPattern => {
                if let Some(path) = pattern
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::BindingPattern)
                {
                    self.resolve_path(path, Namespace::Type)?;
                    self.walk_path_arguments(path)?;
                }
            }
            SyntaxKind::ConstructorPattern | SyntaxKind::QualifiedValuePattern => {
                if let Some(path) = pattern
                    .child_nodes()
                    .find(|child| child.kind() == SyntaxKind::BindingPattern)
                {
                    self.resolve_path(path, Namespace::Type)?;
                    self.walk_path_arguments(path)?;
                }
            }
            _ => {}
        }
        let mut skipped_path = false;
        for child in pattern.child_nodes() {
            if !skipped_path
                && child.kind() == SyntaxKind::BindingPattern
                && matches!(
                    pattern.kind(),
                    SyntaxKind::RecordPattern
                        | SyntaxKind::ConstructorPattern
                        | SyntaxKind::QualifiedValuePattern
                )
            {
                skipped_path = true;
                continue;
            }
            if is_pattern(child.kind()) || child.kind() == SyntaxKind::RecordPatternField {
                self.resolve_pattern_references(child)?;
            }
        }
        Ok(())
    }

    fn resolve_single_token(
        &mut self,
        token: SyntaxTokenRef<'_>,
        namespace: Namespace,
    ) -> Result<(), ResolveError> {
        self.resolve_tokens(
            [token].into_iter(),
            token.range(),
            namespace,
            ValuePathShape::Plain,
        )
    }

    fn resolve_path(
        &mut self,
        path: SyntaxNodeRef<'_>,
        namespace: Namespace,
    ) -> Result<(), ResolveError> {
        let lexical_path = if path.kind() == SyntaxKind::PathType {
            path.child_nodes()
                .find(|child| child.kind() == SyntaxKind::TypePath)
                .expect("a parsed path type contains its type path")
        } else {
            path
        };
        let tokens = lexical_path
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .collect::<Vec<_>>();
        let shape = if tokens.len() > 1 {
            ValuePathShape::Qualified
        } else if lexical_path
            .child_nodes()
            .any(|child| child.kind() == SyntaxKind::BracketPostfix)
        {
            ValuePathShape::Bracketed
        } else {
            ValuePathShape::Plain
        };
        self.resolve_tokens(tokens.into_iter(), path.range(), namespace, shape)
    }

    fn resolve_tokens<'syntax>(
        &mut self,
        tokens: impl Iterator<Item = SyntaxTokenRef<'syntax>>,
        path_range: TextRange,
        namespace: Namespace,
        shape: ValuePathShape,
    ) -> Result<(), ResolveError> {
        let tokens = tokens.collect::<Vec<_>>();
        let Some(first_token) = tokens.first().copied() else {
            return Ok(());
        };
        let first = match normalized_name(first_token) {
            Ok(name) => name,
            Err(_) => {
                self.emit(
                    first_token.range(),
                    "E1001",
                    "the discard `_` is not a value or type name",
                    None,
                )?;
                return Ok(());
            }
        };

        if first.as_str() == "Self" && namespace == Namespace::Type {
            if self.contextual_self {
                self.record_reference(
                    first_token.range(),
                    ResolvedEntity::Name(ResolvedName::ContextualSelf),
                );
            } else {
                self.emit(
                    first_token.range(),
                    "E1001",
                    "contextual type `Self` is only available in a trait, implementation, or inherent method",
                    None,
                )?;
            }
            return Ok(());
        }

        if let Some(import) = self.file_resolution.imports.get(&first).cloned() {
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Module(import.module.clone()),
            );
            let Some(second_token) = tokens.get(1).copied() else {
                self.emit(
                    path_range,
                    "E1001",
                    format!("module `{first}` must be followed by a declaration name"),
                    None,
                )?;
                return Ok(());
            };
            let second = normalized_name(second_token)
                .expect("qualified path identifiers are ordinary names");
            self.resolve_module_member(&import.module, second, second_token, namespace)?;
            return Ok(());
        }

        if namespace == Namespace::Value && shape != ValuePathShape::Plain {
            let type_name = self.lookup_name(Namespace::Type, &first);
            let value_name = self.lookup_name(Namespace::Value, &first);
            match (type_name, value_name) {
                (Some(type_name), Some(value_name)) if type_name == value_name => {
                    self.record_reference(first_token.range(), ResolvedEntity::Name(type_name));
                }
                (Some(type_name), Some(value_name))
                    if self.is_constructor_pair(&type_name, &value_name) =>
                {
                    self.record_reference(first_token.range(), ResolvedEntity::Name(type_name));
                }
                (Some(type_name), Some(value_name)) => {
                    self.record_reference(
                        first_token.range(),
                        ResolvedEntity::ContextualCandidates {
                            type_name,
                            value_name,
                        },
                    );
                    if shape == ValuePathShape::Qualified {
                        self.emit(
                            first_token.range(),
                            "E1004",
                            format!(
                                "`{first}` is ambiguous between the type and value namespaces; qualify or rename one declaration"
                            ),
                            None,
                        )?;
                    }
                }
                (Some(name), None) | (None, Some(name)) => {
                    self.record_reference(first_token.range(), ResolvedEntity::Name(name));
                }
                (None, None) => {
                    self.emit(
                        first_token.range(),
                        "E1001",
                        format!("unknown value or type name `{first}`"),
                        None,
                    )?;
                }
            }
            return Ok(());
        }

        if let Some(entry) = self.lookup_local(namespace, &first) {
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Name(ResolvedName::Local(entry.id)),
            );
            return Ok(());
        }
        if let Some(symbol) = self.lookup_module_symbol(&self.module, namespace, &first) {
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Name(ResolvedName::Symbol(symbol)),
            );
            return Ok(());
        }
        if namespace == Namespace::Value
            && tokens.len() > 1
            && let Some(symbol) = self.lookup_module_symbol(&self.module, Namespace::Type, &first)
        {
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Name(ResolvedName::Symbol(symbol)),
            );
            return Ok(());
        }
        if is_prelude_name(namespace, &first)
            || (namespace == Namespace::Value && is_prelude_constructor(&first))
        {
            let resolved_namespace = if namespace == Namespace::Value
                && is_prelude_constructor(&first)
                && !is_prelude_name(Namespace::Value, &first)
            {
                Namespace::Type
            } else {
                namespace
            };
            self.record_reference(
                first_token.range(),
                ResolvedEntity::Name(ResolvedName::Prelude {
                    namespace: resolved_namespace,
                    name: first,
                }),
            );
            return Ok(());
        }

        self.emit(
            first_token.range(),
            "E1001",
            format!("unknown {} name `{first}`", namespace.as_str()),
            None,
        )
    }

    fn resolve_module_member(
        &mut self,
        module: &ModuleId,
        name: Name,
        token: SyntaxTokenRef<'_>,
        namespace: Namespace,
    ) -> Result<(), ResolveError> {
        let Some(resolved_module) = self.program.module(module) else {
            self.record_reference(
                token.range(),
                ResolvedEntity::Name(ResolvedName::External {
                    module: module.clone(),
                    namespace,
                    name,
                }),
            );
            return Ok(());
        };
        let Some(symbol) = resolved_module.lookup(namespace, &name) else {
            self.emit(
                token.range(),
                "E1001",
                format!(
                    "module `{}` has no {} named `{name}`",
                    module.path(),
                    namespace
                ),
                None,
            )?;
            return Ok(());
        };
        self.check_symbol_visibility(module, name, token, symbol)?;
        self.record_reference(
            token.range(),
            ResolvedEntity::Name(ResolvedName::Symbol(symbol)),
        );
        Ok(())
    }

    fn lookup_name(&self, namespace: Namespace, name: &Name) -> Option<ResolvedName> {
        if namespace == Namespace::Type && name.as_str() == "Self" && self.contextual_self {
            return Some(ResolvedName::ContextualSelf);
        }
        if let Some(entry) = self.lookup_local(namespace, name) {
            return Some(ResolvedName::Local(entry.id));
        }
        if let Some(symbol) = self.lookup_module_symbol(&self.module, namespace, name) {
            return Some(ResolvedName::Symbol(symbol));
        }
        if is_prelude_name(namespace, name)
            || (namespace == Namespace::Value && is_prelude_constructor(name))
        {
            let namespace = if namespace == Namespace::Value
                && is_prelude_constructor(name)
                && !is_prelude_name(Namespace::Value, name)
            {
                Namespace::Type
            } else {
                namespace
            };
            return Some(ResolvedName::Prelude {
                namespace,
                name: name.clone(),
            });
        }
        None
    }

    fn is_constructor_pair(&self, ty: &ResolvedName, value: &ResolvedName) -> bool {
        let (ResolvedName::Symbol(ty), ResolvedName::Symbol(value)) = (ty, value) else {
            return false;
        };
        let Some(ty) = self.program.symbol(*ty) else {
            return false;
        };
        let Some(value) = self.program.symbol(*value) else {
            return false;
        };
        value.kind() == super::SymbolKind::NewtypeConstructor && value.name() == ty.name()
    }

    fn check_symbol_visibility(
        &mut self,
        module: &ModuleId,
        name: Name,
        token: SyntaxTokenRef<'_>,
        symbol: SymbolId,
    ) -> Result<(), ResolveError> {
        let declaration = self
            .program
            .symbol(symbol)
            .expect("module symbol tables contain valid IDs");
        let inaccessible =
            declaration.visibility() == Visibility::Private && module != &self.module;
        let declaration_span = declaration.span();
        if inaccessible {
            self.emit(
                token.range(),
                "E1501",
                format!("`{name}` is private to module `{}`", module.path()),
                Some(("private declaration", declaration_span)),
            )?;
        }
        Ok(())
    }

    fn declare_tokens(
        &mut self,
        tokens: Vec<SyntaxTokenRef<'_>>,
        namespace: Namespace,
        kind: LocalKind,
    ) -> Result<(), ResolveError> {
        for token in tokens {
            let name = match normalized_name(token) {
                Ok(name) => name,
                Err(_) => continue,
            };
            if is_reserved_unqualified(&name) {
                self.emit(
                    token.range(),
                    "E1005",
                    format!("`{name}` is reserved by the Tondo prelude"),
                    None,
                )?;
                continue;
            }
            let span = self.sources.span(self.file, token.range())?;
            let current = self
                .scopes
                .last()
                .expect("name resolution always has a lexical scope");
            let current_map = scope_map(current, namespace);
            if let Some(previous) = current_map.get(&name) {
                self.emit(
                    token.range(),
                    "E1002",
                    format!("`{name}` is declared more than once in this scope"),
                    Some(("first binding with this name", previous.span)),
                )?;
                continue;
            }
            if let Some(previous) = self.lookup_outer(namespace, &name) {
                self.emit(
                    token.range(),
                    "E1003",
                    format!("`{name}` would shadow a visible binding"),
                    Some(("visible binding declared here", previous.span)),
                )?;
            } else if let Some(import) = self.file_resolution.imports.get(&name) {
                self.emit(
                    token.range(),
                    "E1003",
                    format!("`{name}` would shadow an imported module"),
                    Some(("module imported here", import.span())),
                )?;
            } else if let Some(symbol) = self.lookup_module_symbol(&self.module, namespace, &name) {
                let declaration = self
                    .program
                    .symbol(symbol)
                    .expect("module symbol tables contain valid IDs");
                self.emit(
                    token.range(),
                    "E1003",
                    format!("`{name}` would shadow a module declaration"),
                    Some(("module declaration", declaration.span())),
                )?;
            }
            let id = LocalId(
                u32::try_from(self.program.locals.len())
                    .expect("local count is bounded by syntax nodes"),
            );
            self.program.locals.push(LocalBinding {
                id,
                name: name.clone(),
                kind,
                span,
            });
            scope_map_mut(
                self.scopes
                    .last_mut()
                    .expect("name resolution always has a lexical scope"),
                namespace,
            )
            .insert(name, ScopeEntry { id, span });
        }
        Ok(())
    }

    fn lookup_local(&self, namespace: Namespace, name: &Name) -> Option<ScopeEntry> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope_map(scope, namespace).get(name).cloned())
    }

    fn lookup_outer(&self, namespace: Namespace, name: &Name) -> Option<ScopeEntry> {
        self.scopes
            .iter()
            .rev()
            .skip(1)
            .find_map(|scope| scope_map(scope, namespace).get(name).cloned())
    }

    fn lookup_module_symbol(
        &self,
        module: &ModuleId,
        namespace: Namespace,
        name: &Name,
    ) -> Option<SymbolId> {
        self.program.module(module)?.lookup(namespace, name)
    }

    fn record_reference(&mut self, range: TextRange, entity: ResolvedEntity) {
        self.program.references.insert(
            (self.file, range.start(), range.end()),
            ResolvedReference {
                file: self.file,
                range,
                entity,
            },
        );
    }

    fn emit(
        &mut self,
        range: TextRange,
        code: &str,
        message: impl Into<String>,
        related: Option<(&str, Span)>,
    ) -> Result<(), ResolveError> {
        if self.diagnostics.len() >= self.max_diagnostics {
            return Err(ResolveError::DiagnosticLimit {
                file: self.file,
                offset: range.start(),
            });
        }
        let mut diagnostic = Diagnostic::new(
            Severity::Error,
            DiagnosticCode::new(code)?,
            message,
            PrimaryLocation::Source(self.sources.span(self.file, range)?),
        )?;
        if let Some((message, span)) = related {
            diagnostic = diagnostic.with_related(Related::new(message, span)?);
        }
        self.diagnostics.push(diagnostic);
        Ok(())
    }
}

fn scope_map(scope: &Scope, namespace: Namespace) -> &BTreeMap<Name, ScopeEntry> {
    match namespace {
        Namespace::Type => &scope.types,
        Namespace::Value => &scope.values,
        Namespace::Module => unreachable!("module imports are file-local, not lexical"),
    }
}

fn scope_map_mut(scope: &mut Scope, namespace: Namespace) -> &mut BTreeMap<Name, ScopeEntry> {
    match namespace {
        Namespace::Type => &mut scope.types,
        Namespace::Value => &mut scope.values,
        Namespace::Module => unreachable!("module imports are file-local, not lexical"),
    }
}

fn parameter_name_token(parameter: SyntaxNodeRef<'_>) -> Option<SyntaxTokenRef<'_>> {
    parameter
        .child_tokens()
        .find(|token| token.kind() == TokenKind::Identifier)
}

fn has_receiver(node: SyntaxNodeRef<'_>) -> bool {
    node.child_nodes()
        .find(|child| child.kind() == SyntaxKind::ParameterList)
        .and_then(|parameters| {
            parameters
                .child_nodes()
                .find(|child| child.kind() == SyntaxKind::Parameter)
        })
        .is_some_and(|parameter| {
            parameter
                .child_tokens()
                .any(|token| token.kind() == TokenKind::SelfKw)
        })
}

fn is_pattern(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WildcardPattern
            | SyntaxKind::UnitPattern
            | SyntaxKind::LiteralPattern
            | SyntaxKind::OptionResultPattern
            | SyntaxKind::TuplePattern
            | SyntaxKind::ArrayPattern
            | SyntaxKind::ArrayRestPattern
            | SyntaxKind::ConstructorPattern
            | SyntaxKind::RecordPattern
            | SyntaxKind::QualifiedValuePattern
            | SyntaxKind::BorrowBindingPattern
            | SyntaxKind::BindingPattern
    )
}

fn collect_pattern_bindings<'a>(pattern: SyntaxNodeRef<'a>, output: &mut Vec<SyntaxTokenRef<'a>>) {
    match pattern.kind() {
        SyntaxKind::BindingPattern
        | SyntaxKind::BorrowBindingPattern
        | SyntaxKind::ArrayRestPattern => {
            if let Some(token) = pattern
                .child_tokens()
                .find(|token| token.kind() == TokenKind::Identifier)
            {
                output.push(token);
            }
        }
        SyntaxKind::ConstructorPattern
        | SyntaxKind::RecordPattern
        | SyntaxKind::QualifiedValuePattern => {
            let mut skipped_path = false;
            for child in pattern.child_nodes() {
                if !skipped_path && child.kind() == SyntaxKind::BindingPattern {
                    skipped_path = true;
                    continue;
                }
                if child.kind() == SyntaxKind::RecordPatternField {
                    collect_record_field_bindings(child, output);
                } else if is_pattern(child.kind()) {
                    collect_pattern_bindings(child, output);
                }
            }
        }
        _ => {
            for child in pattern.child_nodes() {
                if child.kind() == SyntaxKind::RecordPatternField {
                    collect_record_field_bindings(child, output);
                } else if is_pattern(child.kind()) {
                    collect_pattern_bindings(child, output);
                }
            }
        }
    }
}

fn collect_record_field_bindings<'a>(
    field: SyntaxNodeRef<'a>,
    output: &mut Vec<SyntaxTokenRef<'a>>,
) {
    let patterns = field
        .child_nodes()
        .filter(|child| is_pattern(child.kind()))
        .collect::<Vec<_>>();
    if patterns.is_empty() {
        if let Some(token) = field
            .child_tokens()
            .find(|token| token.kind() == TokenKind::Identifier)
        {
            output.push(token);
        }
    } else {
        for pattern in patterns {
            collect_pattern_bindings(pattern, output);
        }
    }
}

fn is_prelude_name(namespace: Namespace, name: &Name) -> bool {
    match namespace {
        Namespace::Type => matches!(
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
        ),
        Namespace::Value => matches!(name.as_str(), "panic" | "assert"),
        Namespace::Module => false,
    }
}

fn is_prelude_constructor(name: &Name) -> bool {
    matches!(
        name.as_str(),
        "Int"
            | "Float"
            | "Byte"
            | "Char"
            | "String"
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
            | "Ref"
    )
}
