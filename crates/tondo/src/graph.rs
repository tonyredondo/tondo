//! `tondo graph` (toolchain 7.11): what a program is made of. Its
//! packages and their dependencies, the modules of each with their files,
//! and what each module imports, as text, JSON, Graphviz or Mermaid.

use crate::diag::Diagnostics;
use crate::source::SourceMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Graph {
    /// The project's directory, or the file outside a project.
    pub root: PathBuf,
    pub project: bool,
    /// The module of the directory or file asked about, among the
    /// project's sources.
    pub focus: Option<String>,
    /// The root package first.
    pub packages: Vec<Package>,
    pub modules: Vec<Module>,
}

pub struct Package {
    pub name: String,
    pub dir: PathBuf,
    /// Each dependency's alias and package.
    pub dependencies: Vec<(String, String)>,
    pub dev_dependencies: Vec<(String, String)>,
    pub tests_only: bool,
}

pub struct Module {
    /// Its whole path: `app.models`, `http.client`, `std.json`.
    pub name: String,
    /// Its package; `std` for a standard module.
    pub package: String,
    pub files: Vec<PathBuf>,
    pub imports: Vec<Import>,
}

pub struct Import {
    /// The module imported, by its whole path.
    pub module: String,
    pub file: PathBuf,
    pub line: u32,
}

/// The graph of the project at or around `path`, or of the file alone.
pub fn graph(path: &Path) -> Result<Graph, String> {
    let path = path.canonicalize().map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let Some(root) = project_of(&path) else {
        if path.is_dir() {
            return Err(format!("{} is not in a Tondo project: add src/main.to or main.to", path.display()));
        }
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let package = Package {
            name: name.clone(),
            dir: path.parent().unwrap_or(Path::new("/")).to_path_buf(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            tests_only: false,
        };
        let module = Module { name: name.clone(), package: name, files: vec![path.clone()], imports: Vec::new() };
        let mut graph =
            Graph { root: path, project: false, focus: None, packages: vec![package], modules: vec![module] };
        resolve_imports(&mut graph, &[Vec::new()])?;
        return Ok(graph);
    };
    let resolved = crate::deps::resolve(&root, None)?;
    let mut packages = Vec::new();
    let mut modules: Vec<Module> = Vec::new();
    let mut aliases = Vec::new();
    for package in &resolved {
        let named = |list: &[(String, usize)]| -> Vec<(String, String)> {
            let mut named: Vec<_> = list.iter().map(|(alias, i)| (alias.clone(), resolved[*i].name.clone())).collect();
            named.sort();
            named
        };
        let (files, _) = crate::project::sources(&package.dir, &package.name)?;
        for file in files {
            let name = file.module.join(".");
            match modules.iter_mut().find(|m| m.name == name) {
                Some(module) => module.files.push(file.path),
                None => modules.push(Module {
                    name,
                    package: package.name.clone(),
                    files: vec![file.path],
                    imports: Vec::new(),
                }),
            }
        }
        let dependencies = named(&package.dependencies);
        let dev_dependencies = named(&package.dev_dependencies);
        aliases.push(dependencies.iter().chain(&dev_dependencies).cloned().collect::<Vec<_>>());
        packages.push(Package {
            name: package.name.clone(),
            dir: package.dir.clone(),
            dependencies,
            dev_dependencies,
            tests_only: package.tests_only,
        });
    }
    // By package, then by name.
    modules.sort_by_key(|m| (resolved.iter().position(|p| p.name == m.package), m.name.clone()));
    // The module of a directory or file of its sources; none for the
    // project itself or another of its files, as a test.
    let dir = if path.is_dir() { path.as_path() } else { path.parent().unwrap_or(&path) };
    let focus = modules
        .iter()
        .filter(|m| path != root && m.package == packages[0].name)
        .find(|m| m.files.iter().any(|f| f.parent() == Some(dir)))
        .map(|m| m.name.clone());
    let mut graph = Graph { root, project: true, focus, packages, modules };
    resolve_imports(&mut graph, &aliases)?;
    Ok(graph)
}

/// The project that a file or directory belongs to: the nearest directory
/// with `tondo.toml` or `src/main.to`, or the one with `main.to` that holds
/// it.
pub fn project_of(path: &Path) -> Option<PathBuf> {
    let candidates = || path.ancestors().filter(|dir| dir.is_dir());
    let holder = if path.is_dir() { Some(path) } else { path.parent() };
    candidates()
        .find(|dir| dir.join("tondo.toml").is_file())
        .or_else(|| candidates().find(|dir| dir.join("src/main.to").is_file()))
        .or_else(|| holder.filter(|dir| dir.join("main.to").is_file()))
        .map(Path::to_path_buf)
}

/// Fills in each module's imports, read from its files: a standard
/// module, one of its package, or one of a dependency by its alias.
fn resolve_imports(graph: &mut Graph, aliases: &[Vec<(String, String)>]) -> Result<(), String> {
    let paths: Vec<(usize, PathBuf)> =
        graph.modules.iter().enumerate().flat_map(|(i, m)| m.files.iter().map(move |f| (i, f.clone()))).collect();
    let parsed = crate::parallel::map(&paths, 1, |(_, path)| {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut sources = SourceMap::default();
        let file = sources.add(path, text);
        let module = crate::parser::parse_file(sources.file(file), &mut Diagnostics::default());
        let file = sources.file(file);
        let imports: Vec<(Vec<String>, u32)> = module
            .imports
            .iter()
            .map(|i| (i.path.iter().map(|p| p.name.clone()).collect(), file.line_col(i.span.start).0 as u32))
            .collect();
        Ok::<_, String>(imports)
    });
    let mut standard = BTreeSet::new();
    for ((index, path), imports) in paths.into_iter().zip(parsed) {
        let package = graph.packages.iter().position(|p| p.name == graph.modules[index].package).unwrap_or(0);
        for (written, line) in imports? {
            let module = match written[0].as_str() {
                "std" => {
                    standard.insert(written.join("."));
                    written.join(".")
                }
                first if first == graph.packages[package].name => written.join("."),
                first => match aliases[package].iter().find(|(alias, _)| alias == first) {
                    Some((_, name)) => std::iter::once(name.as_str())
                        .chain(written[1..].iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join("."),
                    None => written.join("."),
                },
            };
            let imports = &mut graph.modules[index].imports;
            if !imports.iter().any(|i| i.module == module) {
                imports.push(Import { module, file: path.clone(), line });
            }
        }
    }
    for name in standard {
        let module = Module { name, package: "std".into(), files: Vec::new(), imports: Vec::new() };
        graph.modules.push(module);
    }
    Ok(())
}

impl Graph {
    /// Keeps only the modules that `module` reaches through its imports,
    /// or with `reverse`, those that reach it.
    pub fn around(&mut self, module: &str, reverse: bool) -> Result<(), String> {
        if !self.modules.iter().any(|m| m.name == module) {
            let names: Vec<&str> = self.modules.iter().map(|m| m.name.as_str()).collect();
            return Err(format!("the program has no module `{module}`; its modules are {}", names.join(", ")));
        }
        let mut kept = BTreeSet::from([module.to_string()]);
        let mut pending = vec![module.to_string()];
        while let Some(name) = pending.pop() {
            let next: Vec<String> = if reverse {
                let importers = self.modules.iter().filter(|m| m.imports.iter().any(|i| i.module == name));
                importers.map(|m| m.name.clone()).collect()
            } else {
                let found = self.modules.iter().find(|m| m.name == name);
                found.map(|m| m.imports.iter().map(|i| i.module.clone()).collect()).unwrap_or_default()
            };
            pending.extend(next.into_iter().filter(|n| kept.insert(n.clone())));
        }
        self.modules.retain(|m| kept.contains(&m.name));
        for module in &mut self.modules {
            module.imports.retain(|i| kept.contains(&i.module));
        }
        Ok(())
    }

    /// `tondo-graph-0.1/1`, without the envelope.
    pub fn json(&self, packages_only: bool) -> serde_json::Value {
        let pairs = |list: &[(String, String)]| -> Vec<serde_json::Value> {
            list.iter().map(|(alias, package)| serde_json::json!({ "alias": alias, "package": package })).collect()
        };
        let packages: Vec<serde_json::Value> = self
            .packages
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "dir": p.dir,
                    "dependencies": pairs(&p.dependencies),
                    "dev_dependencies": pairs(&p.dev_dependencies),
                    "tests_only": p.tests_only,
                })
            })
            .collect();
        let mut value = serde_json::json!({
            "root": self.root,
            "project": self.project,
            "focus": self.focus,
            "packages": packages,
        });
        if !packages_only {
            let modules: Vec<serde_json::Value> = self
                .modules
                .iter()
                .map(|m| {
                    let imports: Vec<serde_json::Value> = m
                        .imports
                        .iter()
                        .map(|i| serde_json::json!({ "module": i.module, "file": i.file, "line": i.line }))
                        .collect();
                    serde_json::json!({ "name": m.name, "package": m.package, "files": m.files, "imports": imports })
                })
                .collect();
            value["modules"] = modules.into();
        }
        value
    }

    /// The nodes of a view, each with its package, and its edges, each from
    /// a node to another: packages, modules or files.
    fn edges(&self, view: View) -> (Pairs, Pairs) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        match view {
            View::Packages => {
                for package in &self.packages {
                    nodes.push((package.name.clone(), package.name.clone()));
                    for (_, dependency) in package.dependencies.iter().chain(&package.dev_dependencies) {
                        edges.push((package.name.clone(), dependency.clone()));
                    }
                }
            }
            View::Modules => {
                for module in &self.modules {
                    nodes.push((module.name.clone(), module.package.clone()));
                    edges.extend(module.imports.iter().map(|i| (module.name.clone(), i.module.clone())));
                }
            }
            View::Files => {
                for module in &self.modules {
                    if module.files.is_empty() {
                        nodes.push((module.name.clone(), module.package.clone()));
                    }
                    for file in &module.files {
                        let name = self.shown(file);
                        nodes.push((name.clone(), module.package.clone()));
                        let imports = module.imports.iter().filter(|i| &i.file == file);
                        for import in imports {
                            let target = self.modules.iter().find(|m| m.name == import.module);
                            match target.filter(|m| !m.files.is_empty()) {
                                Some(target) => {
                                    edges.extend(target.files.iter().map(|f| (name.clone(), self.shown(f))))
                                }
                                None => edges.push((name.clone(), import.module.clone())),
                            }
                        }
                    }
                }
            }
        }
        edges.dedup();
        (nodes, edges)
    }

    /// A file as the text views show it: relative to its package.
    fn shown(&self, file: &Path) -> String {
        for (index, package) in self.packages.iter().enumerate() {
            if let Ok(relative) = file.strip_prefix(&package.dir) {
                let relative = relative.display().to_string();
                return if index == 0 { relative } else { format!("{}/{relative}", package.name) };
            }
        }
        file.display().to_string()
    }

    pub fn text(&self, view: View) -> String {
        let mut out = String::new();
        match view {
            View::Packages => {
                for package in &self.packages {
                    let only = if package.tests_only { " (tests only)" } else { "" };
                    out.push_str(&format!("{}{only}  {}\n", package.name, package.dir.display()));
                    for (alias, name) in &package.dependencies {
                        out.push_str(&format!("  {}\n", depends(alias, name)));
                    }
                    for (alias, name) in &package.dev_dependencies {
                        out.push_str(&format!("  {} (dev)\n", depends(alias, name)));
                    }
                }
            }
            View::Modules => {
                for module in &self.modules {
                    let files: Vec<String> = module.files.iter().map(|f| self.shown(f)).collect();
                    out.push_str(format!("{}  {}", module.name, files.join(" ")).trim_end());
                    out.push('\n');
                    for import in &module.imports {
                        out.push_str(&format!("  -> {}\n", import.module));
                    }
                }
            }
            View::Files => {
                let (nodes, edges) = self.edges(View::Files);
                for (node, _) in nodes.iter().filter(|(n, _)| n.ends_with(".to")) {
                    out.push_str(&format!("{node}\n"));
                    for (_, to) in edges.iter().filter(|(from, _)| from == node) {
                        out.push_str(&format!("  -> {to}\n"));
                    }
                }
            }
        }
        out
    }

    /// Graphviz's `dot`: a cluster per package.
    pub fn dot(&self, view: View) -> String {
        let (nodes, edges) = self.edges(view);
        let mut out = String::from("digraph tondo {\n    rankdir=LR;\n    node [shape=box];\n");
        if view == View::Packages {
            for (node, _) in &nodes {
                out.push_str(&format!("    {node:?};\n"));
            }
        } else {
            for (index, package) in self.clusters(&nodes).iter().enumerate() {
                out.push_str(&format!("    subgraph cluster_{index} {{\n        label={:?};\n", package.0));
                for node in &package.1 {
                    out.push_str(&format!("        {node:?};\n"));
                }
                out.push_str("    }\n");
            }
        }
        for (from, to) in &edges {
            out.push_str(&format!("    {from:?} -> {to:?};\n"));
        }
        out.push_str("}\n");
        out
    }

    /// Mermaid's flowchart: a subgraph per package.
    pub fn mermaid(&self, view: View) -> String {
        let (nodes, edges) = self.edges(view);
        let id = |name: &str| {
            nodes.iter().position(|(n, _)| n == name).map_or_else(|| name.to_string(), |i| format!("n{i}"))
        };
        let mut out = String::from("flowchart LR\n");
        let node = |name: &str| format!("{}[\"{}\"]", id(name), name.replace('"', "#quot;"));
        if view == View::Packages {
            for (name, _) in &nodes {
                out.push_str(&format!("    {}\n", node(name)));
            }
        } else {
            for (index, (package, members)) in self.clusters(&nodes).iter().enumerate() {
                out.push_str(&format!("    subgraph p{index}[\"{package}\"]\n"));
                for member in members {
                    out.push_str(&format!("        {}\n", node(member)));
                }
                out.push_str("    end\n");
            }
        }
        for (from, to) in &edges {
            out.push_str(&format!("    {} --> {}\n", id(from), id(to)));
        }
        out
    }

    /// The nodes grouped by package, in the order of the packages, `std`
    /// last.
    fn clusters(&self, nodes: &[(String, String)]) -> Vec<(String, Vec<String>)> {
        let mut order: Vec<&str> = self.packages.iter().map(|p| p.name.as_str()).collect();
        order.push("std");
        order
            .into_iter()
            .map(|package| {
                let members = nodes.iter().filter(|(_, p)| p == package).map(|(n, _)| n.clone()).collect();
                (package.to_string(), members)
            })
            .filter(|(_, members): &(String, Vec<String>)| !members.is_empty())
            .collect()
    }
}

fn depends(alias: &str, name: &str) -> String {
    if alias == name { format!("-> {name}") } else { format!("-> {name} as {alias}") }
}

type Pairs = Vec<(String, String)>;

#[derive(Clone, Copy, PartialEq)]
pub enum View {
    Packages,
    Modules,
    Files,
}
