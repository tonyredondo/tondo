//! Links the modules of a package into one program (6.1–6.3, 7.5).
//!
//! Imports are checked here, and every reference to another module is
//! resolved: a declaration of a module other than the root is renamed after
//! its module path (`User` in `app.models` becomes `models.User`), as are
//! the paths that reach it (`models.User` through `import app.models`). The
//! checker then sees a single namespace, in which those names display as
//! written.

use crate::ast::*;
use crate::diag::Diagnostics;
use crate::source::Span;
use std::collections::HashMap;

pub struct ModuleSource {
    /// The module path, starting with the package name.
    pub path: Vec<String>,
    /// The package it belongs to; the root's is 0.
    pub package: usize,
    /// Its files, and whether each is the root file of the program.
    pub files: Vec<(Module, bool)>,
}

/// The names a module declares, and whether each is `pub`.
#[derive(Default)]
struct Declarations {
    /// Types, enums, aliases and traits.
    types: HashMap<String, bool>,
    /// Functions and newtype constructors.
    values: HashMap<String, bool>,
}

impl Declarations {
    fn get(&self, name: &str) -> Option<bool> {
        self.types.get(name).or(self.values.get(name)).copied()
    }
}

/// Where a path appears, which decides the names its first segment may
/// refer to.
#[derive(Copy, Clone, PartialEq)]
enum Position {
    /// A single name in an expression: a value, never a type.
    Value,
    /// A type, or the first segment of a longer path.
    Any,
}

/// A package of the program: its name and its dependencies by alias, as
/// indices of packages.
pub struct PackageInfo {
    pub name: String,
    pub dependencies: HashMap<String, usize>,
}

/// Merges the modules into one. Declarations record their module's
/// number, 0 for the root.
pub fn link(mut modules: Vec<ModuleSource>, root: usize, packages: &[PackageInfo], diags: &mut Diagnostics) -> Module {
    let first = modules.remove(root);
    modules.insert(0, first);
    let root = 0;
    let decls: Vec<Declarations> = modules.iter().map(declarations).collect();
    // The root package's modules are named after their path inside it, and
    // other packages' after their whole path, unless that is ambiguous.
    // Standard modules are named without `std`: `json.Value`.
    let prefix = |module: &ModuleSource, whole: bool| match &module.path[..] {
        [package] => package.clone(),
        [_, rest @ ..] if packages[module.package].name == "std" => rest.join("."),
        [_, rest @ ..] if module.package == 0 && !whole => rest.join("."),
        path => path.join("."),
    };
    let mut prefixes: Vec<Option<String>> =
        modules.iter().enumerate().map(|(i, m)| (i != root).then(|| prefix(m, false))).collect();
    let mut distinct = std::collections::HashSet::new();
    if !prefixes.iter().flatten().all(|p| distinct.insert(p.clone())) {
        prefixes = modules.iter().enumerate().map(|(i, m)| (i != root).then(|| prefix(m, true))).collect();
    }
    let by_path: HashMap<(usize, String), usize> =
        modules.iter().enumerate().map(|(i, m)| ((m.package, m.path.join(".")), i)).collect();
    let script = modules[root].files.iter().any(|(file, is_root)| *is_root && !file.statements.is_empty());

    // Each file's imports of this package's modules, by alias.
    let mut edges: Vec<Vec<(usize, Span)>> = vec![Vec::new(); modules.len()];
    let mut file_imports: Vec<Vec<HashMap<String, usize>>> = Vec::new();
    let mut std_imports: Vec<Import> = Vec::new();
    for (index, module) in modules.iter().enumerate() {
        let mut per_file = Vec::new();
        for (file, _) in &module.files {
            let mut aliases = HashMap::new();
            for import in &file.imports {
                let path: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
                let alias = import.alias.as_ref().unwrap_or(import.path.last().unwrap());
                if decls[index].get(&alias.name).is_some() {
                    let message = format!(
                        "the import `{}` would shadow a declaration of this module; import it `as` another name",
                        alias.name
                    );
                    diags.error("E1003", alias.span, message);
                }
                let package = &packages[module.package];
                let written =
                    packages.iter().position(|p| p.name == "std").and_then(|std| by_path.get(&(std, path.join("."))));
                if let Some(&target) = written {
                    // A standard module written in Tondo.
                    if aliases.insert(alias.name.clone(), target).is_some() {
                        let message = format!("`{}` is imported twice in this file", alias.name);
                        diags.error("E1002", alias.span, message);
                    }
                    edges[index].push((target, import.span));
                    continue;
                }
                if path == ["std", "testing"] && file.class == SourceClass::Production {
                    let message =
                        "`std.testing` is only for tests: import it in a `_test.to` file or a file under `tests/`";
                    diags.error("E2003", import.span, message);
                }
                if path[0] == "std" {
                    let duplicate = std_imports.iter().find(|i| i.alias_name() == alias.name);
                    match duplicate {
                        Some(other) if other.path_text() != import.path_text() => {
                            let message = format!("`{}` names two different modules in this package", alias.name);
                            diags.error("E1002", alias.span, message);
                        }
                        Some(_) => {}
                        None => std_imports.push(import.clone()),
                    }
                    continue;
                }
                // The first segment names this package or a dependency.
                let (target_package, inner) = match package.dependencies.get(path[0]) {
                    _ if path[0] == package.name => (module.package, path[1..].to_vec()),
                    Some(&dependency) => (dependency, path[1..].to_vec()),
                    None => {
                        let mut names: Vec<String> = package.dependencies.keys().map(|k| format!("`{k}`")).collect();
                        names.sort();
                        names.insert(0, format!("`{}`", package.name));
                        let message = format!(
                            "module `{}` is not available: imports start with `std`, {}",
                            path.join("."),
                            names.join(", ")
                        );
                        diags.error("E1008", import.span, message);
                        continue;
                    }
                };
                let full: Vec<&str> =
                    std::iter::once(packages[target_package].name.as_str()).chain(inner.iter().copied()).collect();
                let Some(&target) = by_path.get(&(target_package, full.join("."))) else {
                    let message = format!("the package `{}` has no module `{}`", path[0], path.join("."));
                    diags.error("E1008", import.span, message);
                    continue;
                };
                if target == index {
                    diags.error("E1006", import.span, "a module cannot import itself");
                    continue;
                }
                if target == root && script {
                    let message = "the root file is a script, so its module cannot be imported; move shared declarations to another module";
                    diags.error("E1801", import.span, message);
                    continue;
                }
                if aliases.insert(alias.name.clone(), target).is_some() {
                    let message = format!("`{}` is imported twice in this file", alias.name);
                    diags.error("E1002", alias.span, message);
                }
                edges[index].push((target, import.span));
            }
            per_file.push(aliases);
        }
        file_imports.push(per_file);
    }
    report_cycles(&modules, &edges, diags);

    let module_paths = modules.iter().map(|m| m.path.clone()).collect();
    let mut merged = Module { imports: std_imports, module_paths, ..Module::default() };
    let paths: Vec<String> = modules.iter().map(|m| m.path.join(".")).collect();
    for (index, module) in modules.into_iter().enumerate() {
        for ((file, is_root), imports) in module.files.into_iter().zip(&file_imports[index]) {
            let class = file.class;
            let mut linker =
                Linker { module: index, class, imports, decls: &decls, prefixes: &prefixes, paths: &paths, diags };
            let Module { items, statements, .. } = file;
            for mut item in items {
                linker.item(&mut item);
                merged.items.push(item);
            }
            if is_root {
                let mut statements = statements;
                statements.iter_mut().for_each(|s| linker.stmt(s));
                merged.statements = statements;
            } else if let Some(first) = statements.first() {
                let message = "top-level statements belong in the root file of the program; move them into a function";
                linker.diags.error("E1804", first.span, message);
            }
        }
    }
    merged
}

/// The name of a `pub` declaration.
fn public_name(item: &Item) -> Option<&Ident> {
    match item {
        Item::Fn(f) if f.public => Some(&f.name),
        Item::Type(t) if t.public => Some(&t.name),
        Item::Alias(a) if a.public => Some(&a.name),
        Item::Const(c) if c.public => Some(&c.name),
        Item::Enum(e) if e.public => Some(&e.name),
        Item::Trait(t) if t.public => Some(&t.name),
        _ => None,
    }
}

fn declarations(module: &ModuleSource) -> Declarations {
    let mut decls = Declarations::default();
    for (file, _) in &module.files {
        for item in &file.items {
            match item {
                Item::Fn(f) if f.owner.is_none() => {
                    decls.values.insert(f.name.name.clone(), f.public);
                }
                Item::Type(t) => {
                    decls.types.insert(t.name.name.clone(), t.public);
                    if let TypeBody::Newtype(_) = t.body {
                        decls.values.insert(t.name.name.clone(), t.public);
                    }
                }
                Item::Enum(e) => {
                    decls.types.insert(e.name.name.clone(), e.public);
                }
                Item::Alias(a) => {
                    decls.types.insert(a.name.name.clone(), a.public);
                }
                Item::Const(c) => {
                    decls.values.insert(c.name.name.clone(), c.public);
                }
                Item::Trait(t) => {
                    decls.types.insert(t.name.name.clone(), t.public);
                }
                Item::Fn(_) | Item::Impl(_) | Item::Test(_) | Item::Derive(_) => {}
            }
        }
    }
    decls
}

/// Reports each import cycle once, from the import that closes it.
fn report_cycles(modules: &[ModuleSource], edges: &[Vec<(usize, Span)>], diags: &mut Diagnostics) {
    // 0: unvisited, 1: on the current path, 2: done.
    let mut state = vec![0u8; modules.len()];
    let mut path: Vec<usize> = Vec::new();
    fn visit(
        node: usize,
        modules: &[ModuleSource],
        edges: &[Vec<(usize, Span)>],
        state: &mut [u8],
        path: &mut Vec<usize>,
        diags: &mut Diagnostics,
    ) {
        state[node] = 1;
        path.push(node);
        for &(next, span) in &edges[node] {
            match state[next] {
                0 => visit(next, modules, edges, state, path, diags),
                1 => {
                    let start = path.iter().position(|&n| n == next).unwrap();
                    let cycle: Vec<String> = path[start..]
                        .iter()
                        .chain([&next])
                        .map(|&n| format!("`{}`", modules[n].path.join(".")))
                        .collect();
                    let message = format!(
                        "modules cannot import each other in a cycle: {} imports {}",
                        cycle[0],
                        cycle[1..].join(", which imports ")
                    );
                    diags.error("E1006", span, message);
                }
                _ => {}
            }
        }
        path.pop();
        state[node] = 2;
    }
    for node in 0..modules.len() {
        if state[node] == 0 {
            visit(node, modules, edges, &mut state, &mut path, diags);
        }
    }
}

impl Import {
    fn alias_name(&self) -> &str {
        &self.alias.as_ref().unwrap_or(self.path.last().unwrap()).name
    }

    fn path_text(&self) -> String {
        self.path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".")
    }
}

struct Linker<'a> {
    module: usize,
    /// The class of the file being linked.
    class: SourceClass,
    /// This file's imports of the package's modules.
    imports: &'a HashMap<String, usize>,
    decls: &'a [Declarations],
    /// What declarations of each module are renamed after; the root's keep
    /// their names.
    prefixes: &'a [Option<String>],
    /// The path of each module.
    paths: &'a [String],
    diags: &'a mut Diagnostics,
}

impl Linker<'_> {
    fn renamed(&self, module: usize, name: &str) -> String {
        match &self.prefixes[module] {
            Some(prefix) => format!("{prefix}.{name}"),
            None => name.to_string(),
        }
    }

    /// A local binding, which must not take the name of a value this module
    /// declares or imports. The checker reports that in the root module,
    /// whose names keep their spelling; elsewhere they are renamed by now.
    fn binding(&mut self, name: &Ident) {
        if self.prefixes[self.module].is_none() || name.name == "self" {
            return;
        }
        if self.decls[self.module].values.contains_key(&name.name) || self.imports.contains_key(&name.name) {
            let message =
                format!("`{}` would shadow a visible binding with the same name: choose another name", name.name);
            self.diags.error("E1003", name.span, message);
        }
    }

    /// Renames a declaration of this module.
    fn declared(&self, name: &mut Ident) {
        name.name = self.renamed(self.module, &name.name);
    }

    /// Resolves the module part of a path: `alias.Name` through an import,
    /// or a name this module declares.
    fn path(&mut self, segments: &mut Vec<Ident>, position: Position) {
        if segments.len() >= 2
            && let Some(&target) = self.imports.get(&segments[0].name)
        {
            let (alias, name) = (&segments[0], &segments[1]);
            let module = &self.paths[target];
            match self.decls[target].get(&name.name) {
                None => {
                    let message = format!("module `{module}` declares no `{}`", name.name);
                    self.diags.error("E1001", name.span, message);
                }
                Some(false) => {
                    let message =
                        format!("`{}` is private to module `{module}`; declare it `pub` to use it here", name.name);
                    self.diags.error("E1501", name.span, message);
                }
                Some(true) => {}
            }
            let joined = Ident { name: self.renamed(target, &name.name), span: alias.span.to(name.span) };
            segments.splice(0..2, [joined]);
            return;
        }
        let Some(first) = segments.first_mut() else { return };
        let decls = &self.decls[self.module];
        let declared = match position {
            Position::Value => decls.values.contains_key(&first.name),
            Position::Any => decls.get(&first.name).is_some(),
        };
        if declared {
            first.name = self.renamed(self.module, &first.name);
        }
    }

    fn item(&mut self, item: &mut Item) {
        let module = self.module as u32;
        if self.class != SourceClass::Production
            && let Some(name) = public_name(item)
        {
            let message = format!("a test file cannot export `{}`: remove `pub`", name.name);
            self.diags.error("E2003", name.span, message);
        }
        match item {
            Item::Fn(f) => self.function(f, true),
            Item::Type(t) => {
                t.module = module;
                self.declared(&mut t.name);
                self.generics(&mut t.generics);
                match &mut t.body {
                    TypeBody::Record(fields) => fields.iter_mut().for_each(|f| self.ty(&mut f.ty)),
                    TypeBody::Newtype(inner) => self.ty(inner),
                    TypeBody::Opaque => {}
                }
            }
            Item::Alias(a) => {
                self.declared(&mut a.name);
                self.ty(&mut a.target);
            }
            Item::Const(c) => {
                c.module = module;
                self.declared(&mut c.name);
                if let Some(ty) = &mut c.ty {
                    self.ty(ty);
                }
                self.expr(&mut c.value);
            }
            Item::Enum(e) => {
                e.module = module;
                self.declared(&mut e.name);
                self.generics(&mut e.generics);
                for variant in &mut e.variants {
                    match &mut variant.payload {
                        VariantPayload::Unit => {}
                        VariantPayload::Tuple(types) => types.iter_mut().for_each(|t| self.ty(t)),
                        VariantPayload::Record(fields) => fields.iter_mut().for_each(|f| self.ty(&mut f.ty)),
                    }
                }
            }
            Item::Trait(t) => {
                t.module = module;
                self.declared(&mut t.name);
                self.generics(&mut t.generics);
                t.methods.iter_mut().for_each(|m| self.function(m, false));
            }
            Item::Impl(i) => {
                i.module = module;
                self.generics(&mut i.generics);
                self.ty(&mut i.trait_ref);
                self.ty(&mut i.target);
                i.methods.iter_mut().for_each(|m| self.function(m, false));
            }
            Item::Test(node) => self.test_node(node),
            // Expanded into implementations before modules are linked.
            Item::Derive(_) => {}
        }
    }

    /// A test or a suite: the module and class it belongs to, and the
    /// paths in its body; its members see the suite's setup bindings.
    fn test_node(&mut self, node: &mut TestNode) {
        match node {
            TestNode::Test(t) => {
                (t.module, t.class) = (self.module as u32, self.class);
                self.block(&mut t.body);
            }
            TestNode::Suite(s) => {
                (s.module, s.class) = (self.module as u32, self.class);
                s.setup.iter_mut().for_each(|stmt| self.stmt(stmt));
                s.members.iter_mut().for_each(|member| self.test_node(member));
            }
        }
    }

    /// A function; a free one at the top level is a declaration of the
    /// module, and a method's owner is one of its types.
    fn function(&mut self, f: &mut FnDecl, top: bool) {
        f.module = self.module as u32;
        match &mut f.owner {
            Some(owner) => {
                let mut path = vec![owner.clone()];
                self.path(&mut path, Position::Any);
                *owner = path.pop().unwrap();
            }
            None if top => self.declared(&mut f.name),
            None => {}
        }
        self.generics(&mut f.owner_generics);
        self.generics(&mut f.generics);
        for param in &mut f.params {
            if let Some(ty) = &mut param.ty {
                self.ty(ty);
                self.binding(&param.name);
            }
            if let Some(default) = &mut param.default {
                self.expr(default);
            }
        }
        if let Some(result) = &mut f.result {
            self.ty(result);
        }
        if let Some(body) = &mut f.body {
            self.block(body);
        }
    }

    fn generics(&mut self, generics: &mut [GenericParam]) {
        for generic in generics {
            generic.bounds.iter_mut().for_each(|b| self.ty(b));
        }
    }

    fn ty(&mut self, ty: &mut TypeExpr) {
        match &mut ty.kind {
            TypeKind::Path { segments, args } => {
                self.path(segments, Position::Any);
                args.iter_mut().for_each(|a| self.ty(a));
            }
            TypeKind::Optional(inner) => self.ty(inner),
            TypeKind::Result { ok, err } => {
                if let Some(ok) = ok {
                    self.ty(ok);
                }
                self.ty(err);
            }
            TypeKind::Union(items) | TypeKind::Tuple(items) | TypeKind::Opaque(items) => {
                items.iter_mut().for_each(|t| self.ty(t))
            }
            TypeKind::Fn { params, result, .. } => {
                params.iter_mut().for_each(|t| self.ty(t));
                if let Some(result) = result {
                    self.ty(result);
                }
            }
            TypeKind::NotCopy => {}
        }
    }

    fn block(&mut self, block: &mut Block) {
        block.stmts.iter_mut().for_each(|s| self.stmt(s));
        if let Some(tail) = &mut block.tail {
            self.expr(tail);
        }
    }

    fn stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Let { pattern, ty, value, .. } => {
                self.pattern(pattern);
                if let Some(ty) = ty {
                    self.ty(ty);
                }
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            StmtKind::Assign { target, value, .. } => {
                self.expr(target);
                self.expr(value);
            }
            StmtKind::Discard(value) | StmtKind::Fail(value) | StmtKind::Expr(value) => self.expr(value),
            StmtKind::Return(value) => {
                if let Some(value) = value {
                    self.expr(value);
                }
            }
            StmtKind::Break | StmtKind::Continue => {}
            StmtKind::For { header, body } => {
                match header {
                    ForHeader::Infinite => {}
                    ForHeader::Condition(condition) => self.expr(condition),
                    ForHeader::In { pattern, source } => {
                        self.pattern(pattern);
                        self.expr(source);
                    }
                }
                self.block(body);
            }
            StmtKind::Defer(Defer::Call(call)) => self.expr(call),
            StmtKind::Defer(Defer::Block(block)) => self.block(block),
        }
    }

    fn expr(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            ExprKind::Path(segments) => {
                let position = if segments.len() == 1 { Position::Value } else { Position::Any };
                self.path(segments, position);
            }
            ExprKind::Record { path, type_args, fields } => {
                self.path(path, Position::Any);
                type_args.iter_mut().for_each(|t| self.ty(t));
                fields.iter_mut().filter_map(|f| f.value.as_mut()).for_each(|v| self.expr(v));
            }
            ExprKind::With { base, updates } => {
                self.expr(base);
                updates.iter_mut().filter_map(|f| f.value.as_mut()).for_each(|v| self.expr(v));
            }
            ExprKind::Call { callee, args } => {
                self.expr(callee);
                args.iter_mut().for_each(|a| self.expr(&mut a.value));
            }
            ExprKind::Generic { base, args } => {
                self.expr(base);
                args.iter_mut().for_each(|t| self.ty(t));
            }
            ExprKind::Field(inner, _)
            | ExprKind::TupleSlot(inner, _)
            | ExprKind::Unary(_, inner)
            | ExprKind::Try(inner)
            | ExprKind::Spawn(inner)
            | ExprKind::Construct(_, inner) => self.expr(inner),
            ExprKind::Binary(_, left, right) => {
                self.expr(left);
                self.expr(right);
            }
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::Slice { base, start, end, step } => {
                self.expr(base);
                for part in [start, end, step].into_iter().flatten() {
                    self.expr(part);
                }
            }
            ExprKind::If { condition, then, otherwise } => {
                self.expr(condition);
                self.block(then);
                if let Some(otherwise) = otherwise {
                    self.expr(otherwise);
                }
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    self.pattern(&mut arm.pattern);
                    if let Some(guard) = &mut arm.guard {
                        self.expr(guard);
                    }
                    match &mut arm.body {
                        ArmBody::Expr(body) => self.expr(body),
                        ArmBody::Statement(stmt) => self.stmt(stmt),
                    }
                }
            }
            ExprKind::SyncLiteral(_, literal) => self.expr(literal),
            ExprKind::Select(arms) => {
                for arm in arms {
                    if let Some(pattern) = &mut arm.pattern {
                        self.pattern(pattern);
                    }
                    if let Some(operation) = &mut arm.operation {
                        self.expr(operation);
                    }
                    match &mut arm.body {
                        ArmBody::Expr(body) => self.expr(body),
                        ArmBody::Statement(stmt) => self.stmt(stmt),
                    }
                }
            }
            ExprKind::Tuple(items) | ExprKind::Array(items) | ExprKind::Set(items) => {
                items.iter_mut().for_each(|e| self.expr(e))
            }
            ExprKind::Map(entries) => {
                for (key, value) in entries {
                    self.expr(key);
                    self.expr(value);
                }
            }
            ExprKind::Str(segments) => {
                for segment in segments {
                    if let StrSegment::Expr(value) = segment {
                        self.expr(value);
                    }
                }
            }
            ExprKind::Unsafe(block) => self.block(block),
            ExprKind::Closure(closure) => {
                for param in &mut closure.params {
                    self.binding(&param.name);
                    if let Some(ty) = &mut param.ty {
                        self.ty(ty);
                    }
                }
                if let Some(result) = &mut closure.result {
                    self.ty(result);
                }
                self.block(&mut closure.body);
            }
            ExprKind::Int { .. }
            | ExprKind::Float { .. }
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::Unit
            | ExprKind::None
            | ExprKind::Error => {}
        }
    }

    fn pattern(&mut self, pattern: &mut Pattern) {
        match &mut pattern.kind {
            PatternKind::Constructor { path, args } => {
                self.path(path, Position::Any);
                args.iter_mut().for_each(|p| self.pattern(p));
            }
            PatternKind::Record { path, fields, .. } => {
                self.path(path, Position::Any);
                for field in fields {
                    match &mut field.pattern {
                        Some(pattern) => self.pattern(pattern),
                        None => self.binding(&field.name),
                    }
                }
            }
            PatternKind::Path(path) => self.path(path, Position::Any),
            PatternKind::Tuple(items) | PatternKind::Array { items, .. } => {
                items.iter_mut().for_each(|p| self.pattern(p))
            }
            PatternKind::Member { ty, inner } => {
                self.ty(ty);
                self.pattern(inner);
            }
            PatternKind::Binding(name) | PatternKind::Loan(_, name) => self.binding(name),
            PatternKind::Literal(_) | PatternKind::Wildcard | PatternKind::None => {}
        }
    }
}
