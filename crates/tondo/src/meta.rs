//! `derive` (27.2 to 27.4 of the language): before the program is checked,
//! each `derive` asks the provider of each trait it names for an `impl` for
//! its type. The package that declares a trait names its provider in
//! `[derive]` of its `tondo.toml`. The provider is a Tondo program built for
//! the `meta` profile and run in a process of its own, with the request in a
//! file and its response in another; what it gives back must be exactly the
//! `impl` asked for, which joins the type's module. Generators (27.5) run
//! the same way, from `[generators.<name>]`, and write modules of their
//! package.

use crate::ast;
use crate::diag::Diagnostics;
use crate::modules::{ModuleSource, PackageInfo};
use crate::source::SourceMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The budgets of a run (27.7): steps, bytes of memory and of source.
#[derive(Clone, Copy)]
struct Budgets {
    steps: u64,
    memory: u64,
    output: u64,
}

/// A provider's budgets, and a generator's unless it declares its own.
const DEFAULT: Budgets = Budgets { steps: 100_000_000, memory: 1 << 30, output: 1 << 20 };

/// What a program built for `meta` serves: a `derive`'s request with its
/// `provide`, or a generator's with its `generate`.
#[derive(Clone, Copy, PartialEq)]
enum Entry {
    Provide,
    Generate,
}

/// An `impl` a provider gave, for `tondo expand`.
pub struct Expansion {
    /// `derive show.Show for Point`, and where it is.
    pub request: String,
    pub location: String,
    pub source: String,
}

/// Expands every `derive` of the modules; `providers` are those of each
/// package, by its index.
pub fn expand(
    modules: &mut [ModuleSource],
    packages: &[PackageInfo],
    providers: &[Vec<(String, PathBuf)>],
    sources: &mut SourceMap,
    diags: &mut Diagnostics,
) -> Vec<Expansion> {
    let mut expansions = Vec::new();
    let paths: Vec<Vec<String>> = modules.iter().map(|m| m.path.clone()).collect();
    for module in 0..modules.len() {
        for file in 0..modules[module].files.len() {
            let derives: Vec<ast::DeriveDecl> = {
                let items = &mut modules[module].files[file].0.items;
                let (derives, rest): (Vec<ast::Item>, Vec<ast::Item>) =
                    std::mem::take(items).into_iter().partition(|i| matches!(i, ast::Item::Derive(_)));
                *items = rest;
                derives
                    .into_iter()
                    .filter_map(|i| match i {
                        ast::Item::Derive(d) => Some(d),
                        _ => None,
                    })
                    .collect()
            };
            let mut seen: Vec<(String, String)> = Vec::new();
            for derive in derives {
                let Some(target) = target(modules, module, &derive, sources, diags) else { continue };
                for trait_ref in &derive.traits {
                    let trait_text = sources.slice(trait_ref.span).to_string();
                    let pair = (trait_text.clone(), target.written.clone());
                    if seen.contains(&pair) {
                        let message =
                            format!("`{trait_text}` is already derived for `{}` in this module", target.written);
                        diags.error("E2103", trait_ref.span, message);
                        continue;
                    }
                    seen.push(pair);
                    let Some(provider) =
                        provider(modules, packages, module, file, trait_ref, &trait_text, providers, diags)
                    else {
                        continue;
                    };
                    let request = request(&trait_text, &target, &paths[module]);
                    match run(&provider, Entry::Provide, &request, DEFAULT) {
                        Ok(Response { source, problems, .. }) if problems.is_empty() => {
                            let name = format!("{}.{}.derived.to", paths[module].join("."), trait_text);
                            match accept(&source, &trait_text, &target.written, &name, sources) {
                                Some((items, formatted)) => {
                                    modules[module].files[file].0.items.extend(items);
                                    expansions.push(Expansion {
                                        request: format!("derive {trait_text} for {}", target.written),
                                        location: sources.location(derive.span),
                                        source: formatted,
                                    });
                                }
                                None => {
                                    let message = format!(
                                        "the provider of `{trait_text}` gave something other than one `impl {trait_text} for {}`; `tondo expand` shows what it gave",
                                        target.written
                                    );
                                    diags.error("E2105", derive.span, message);
                                    expansions.push(Expansion {
                                        request: format!("derive {trait_text} for {} (rejected)", target.written),
                                        location: sources.location(derive.span),
                                        source,
                                    });
                                }
                            }
                        }
                        Ok(Response { problems, .. }) => {
                            for problem in problems {
                                diags.error("E2104", derive.span, format!("`{trait_text}`: {problem}"));
                            }
                        }
                        Err(Failure::Budget(_, what)) => {
                            let message = format!(
                                "the provider of `{trait_text}` spent its budget of {what} for `{}`",
                                target.written
                            );
                            diags.error("E2107", derive.span, message);
                        }
                        Err(Failure::Provider(why)) => {
                            diags.error("E2104", derive.span, format!("the provider of `{trait_text}` failed: {why}"));
                        }
                        Err(Failure::Denied(why)) => {
                            let message = format!("the provider of `{trait_text}` uses what a provider cannot: {why}");
                            diags.error("E2108", derive.span, message);
                        }
                    }
                }
            }
        }
    }
    expansions
}

/// Runs each generator of each package (27.5) and adds the modules it
/// writes; `generators` are those of each package, by its index.
pub fn generate(
    modules: &mut Vec<ModuleSource>,
    packages: &[PackageInfo],
    generators: &[Vec<crate::deps::Generator>],
    sources: &mut SourceMap,
    diags: &mut Diagnostics,
) -> Vec<Expansion> {
    let mut expansions = Vec::new();
    let path_of = |package: usize, output: &str| -> Vec<String> {
        std::iter::once(packages[package].name.clone()).chain(output.split('.').map(String::from)).collect()
    };
    let generated: Vec<Vec<String>> = generators
        .iter()
        .enumerate()
        .flat_map(|(package, list)| list.iter().flat_map(move |g| g.outputs.iter().map(move |o| (package, o))))
        .map(|(package, output)| path_of(package, output))
        .collect();
    let mut written = Vec::new();
    for (package, list) in generators.iter().enumerate() {
        for generator in list {
            let place = format!("`[generators.{}]` of the package `{}`", generator.name, packages[package].name);
            // Any problem discards everything the generator wrote.
            let mut problems: Vec<(&'static str, String)> = Vec::new();
            let declarations =
                declarations(modules, packages, package, &generator.roots, &generated, sources, &mut problems);
            let mut inputs = Vec::new();
            for (name, file) in &generator.inputs {
                match std::fs::read(file) {
                    Ok(bytes) => {
                        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                        let hash = crate::hash::sha256_hex(&bytes);
                        inputs.push(format!(
                            "{{\"name\":{},\"hex\":{},\"sha256\":{}}}",
                            json(name),
                            json(&hex),
                            json(&hash)
                        ));
                    }
                    Err(e) => problems.push(("E2106", format!("cannot read its input `{name}`: {e}"))),
                }
            }
            let budgets = Budgets {
                steps: generator.steps.unwrap_or(DEFAULT.steps),
                memory: generator.memory.unwrap_or(DEFAULT.memory),
                output: generator.output.unwrap_or(DEFAULT.output),
            };
            let mut accepted = Vec::new();
            if problems.is_empty() {
                let outputs: Vec<String> = generator.outputs.iter().map(|o| json(o)).collect();
                let request = format!(
                    "{{\"inputs\":[{}],\"outputs\":[{}],\"declarations\":[{}],\"limits\":{{\"output\":{}}}}}",
                    inputs.join(","),
                    outputs.join(","),
                    declarations.join(","),
                    budgets.output
                );
                match run(&generator.program, Entry::Generate, &request, budgets) {
                    Ok(response) if !response.problems.is_empty() => {
                        problems.extend(response.problems.into_iter().map(|p| ("E2106", p)));
                    }
                    Ok(response) => {
                        for output in &generator.outputs {
                            if !response.files.iter().any(|(name, _)| name == output) {
                                problems.push(("E2106", format!("it did not write its output `{output}`")));
                            }
                        }
                        for (output, source) in response.files {
                            let path = path_of(package, &output);
                            if !generator.outputs.contains(&output) {
                                problems
                                    .push(("E2106", format!("it wrote `{output}`, which is not one of its `outputs`")));
                            } else if modules.iter().any(|m| m.path == path) {
                                let why = format!("its output `{output}` would replace a module written by hand");
                                problems.push(("E2106", why));
                            } else {
                                accepted.push((output, path, source));
                            }
                        }
                    }
                    Err(Failure::Budget(limit, what)) => {
                        let why =
                            format!("it spent its budget of {what}; `limits = {{ {limit} = \"...\" }}` raises it");
                        problems.push(("E2107", why));
                    }
                    Err(Failure::Provider(why)) => problems.push(("E2106", format!("it failed: {why}"))),
                    Err(Failure::Denied(why)) => {
                        problems.push(("E2108", format!("it uses what a generator cannot: {why}")));
                    }
                }
            }
            let mut parsed = Vec::new();
            for (output, path, source) in accepted {
                let name = format!("{}.to", path.join("/"));
                let id = sources.add(Path::new(&name), source.clone());
                sources.set_identity(id, format!("<generated>/{name}"), path.join("."));
                let mut errors = Diagnostics::default();
                let module = crate::parser::parse_file(sources.file(id), &mut errors);
                let derives = module.items.iter().any(|i| matches!(i, ast::Item::Derive(_)));
                let request = format!("generator {} wrote {output}", generator.name);
                if errors.has_errors() || derives || !module.statements.is_empty() {
                    let why = if derives { "has a `derive`" } else { "is not valid Tondo source" };
                    problems.push(("E2105", format!("its output `{output}` {why}; `tondo expand` shows it")));
                    expansions.push(Expansion {
                        request: format!("{request} (rejected)"),
                        location: place.clone(),
                        source,
                    });
                    continue;
                }
                let formatted = crate::fmt::format(sources.file(id), &mut Diagnostics::default()).unwrap_or(source);
                expansions.push(Expansion { request, location: place.clone(), source: formatted });
                parsed.push(ModuleSource { path, package, files: vec![(module, false)] });
            }
            if problems.is_empty() {
                written.extend(parsed);
            }
            for (code, why) in problems {
                diags.error_without_span(code, format!("the generator {place}: {why}"));
            }
        }
    }
    modules.extend(written);
    expansions
}

/// The declarations a generator sees (27.5), as a request describes them:
/// each root, and the public records, enums and newtypes that their public
/// fields, variants and wrapped types name, with the module of each. One
/// that only an output of this build declares is `E2109`.
#[allow(clippy::too_many_arguments)]
fn declarations(
    modules: &[ModuleSource],
    packages: &[PackageInfo],
    package: usize,
    roots: &[String],
    generated: &[Vec<String>],
    sources: &SourceMap,
    problems: &mut Vec<(&'static str, String)>,
) -> Vec<String> {
    let cycle = |problems: &mut Vec<(&'static str, String)>, what: String| {
        let why =
            format!("{what}, which a generator of this build writes; a generator sees only modules written by hand");
        problems.push(("E2109", why));
    };
    let mut reached: Vec<(usize, String)> = Vec::new();
    for root in roots {
        // A module of the package, as `outputs` names them, and a type.
        let (module, name) = root.rsplit_once('.').unwrap_or(("", root));
        let path: Vec<String> = std::iter::once(packages[package].name.clone())
            .chain(module.split('.').filter(|s| !s.is_empty()).map(String::from))
            .collect();
        match modules.iter().position(|m| m.package == package && m.path == path) {
            Some(m) if declared(&modules[m], name).is_some_and(|(_, item)| view(item, sources, true).is_some()) => {
                reached.push((m, name.to_string()));
            }
            None if generated.contains(&path) => {
                cycle(problems, format!("its root `{root}` is in `{}`", path.join(".")));
            }
            _ => problems.push(("E2106", format!("its root `{root}` is not a public record, enum or newtype"))),
        }
    }
    let mut out = Vec::new();
    let mut next = 0;
    while let Some((module, name)) = reached.get(next).cloned() {
        next += 1;
        let Some((file, item)) = declared(&modules[module], &name) else { continue };
        let Some(view) = view(item, sources, true) else { continue };
        out.push(view_json(&view, &modules[module].path.join(".")));
        let mut named = Vec::new();
        for ty in public_types(item) {
            paths(ty, &mut named);
        }
        for segments in named {
            let found = match segments {
                [one] if !view.generics.iter().any(|(g, _)| *g == one.name) => Some((module, one.name.clone())),
                [qualifier, one] => match imported(modules, packages, module, file, &qualifier.name) {
                    Some((package, path)) => {
                        match modules.iter().position(|m| m.package == package && m.path == path) {
                            Some(m) => Some((m, one.name.clone())),
                            None if generated.contains(&path) => {
                                let written = format!("{}.{}", qualifier.name, one.name);
                                cycle(
                                    problems,
                                    format!("`{}.{name}` names `{written}`", modules[module].path.join(".")),
                                );
                                None
                            }
                            None => None,
                        }
                    }
                    None => None,
                },
                _ => None,
            };
            if let Some(found) = found.filter(|f| !reached.contains(f) && declared(&modules[f.0], &f.1).is_some()) {
                reached.push(found);
            }
        }
    }
    out
}

/// The record, enum or newtype a module declares under `name`, and its file.
fn declared<'a>(module: &'a ModuleSource, name: &str) -> Option<(usize, &'a ast::Item)> {
    module.files.iter().enumerate().find_map(|(file, (source, _))| {
        source.items.iter().find_map(|item| match item {
            ast::Item::Type(t) if t.name.name == name => Some((file, item)),
            ast::Item::Enum(e) if e.name.name == name => Some((file, item)),
            _ => None,
        })
    })
}

/// The types of a declaration's public fields, variants and wrapped type.
fn public_types(item: &ast::Item) -> Vec<&ast::TypeExpr> {
    fn fields(declared: &[ast::FieldDecl]) -> Vec<&ast::TypeExpr> {
        declared.iter().filter(|f| !f.private).map(|f| &f.ty).collect()
    }
    match item {
        ast::Item::Type(t) => match &t.body {
            ast::TypeBody::Record(declared) => fields(declared),
            ast::TypeBody::Newtype(inner) => vec![inner],
            ast::TypeBody::Opaque => Vec::new(),
        },
        ast::Item::Enum(e) => e
            .variants
            .iter()
            .flat_map(|v| match &v.payload {
                ast::VariantPayload::Unit => Vec::new(),
                ast::VariantPayload::Tuple(types) => types.iter().collect(),
                ast::VariantPayload::Record(declared) => fields(declared),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The paths a type names, with those of its arguments.
fn paths<'a>(ty: &'a ast::TypeExpr, out: &mut Vec<&'a [ast::Ident]>) {
    match &ty.kind {
        ast::TypeKind::Path { segments, args } => {
            out.push(segments);
            args.iter().for_each(|a| paths(a, out));
        }
        ast::TypeKind::Optional(inner) => paths(inner, out),
        ast::TypeKind::Result { ok, err } => {
            ok.iter().for_each(|t| paths(t, out));
            paths(err, out);
        }
        ast::TypeKind::Union(types) | ast::TypeKind::Tuple(types) | ast::TypeKind::Opaque(types) => {
            types.iter().for_each(|t| paths(t, out));
        }
        ast::TypeKind::Fn { params, result, .. } => {
            params.iter().for_each(|t| paths(t, out));
            result.iter().for_each(|t| paths(t, out));
        }
        ast::TypeKind::NotCopy => {}
    }
}

/// A declaration as a request describes it: its kind, parameters, fields,
/// variants and the type a newtype wraps, each type as written.
struct View {
    name: String,
    kind: &'static str,
    generics: Vec<(String, Vec<String>)>,
    fields: Vec<Field>,
    variants: Vec<(String, Vec<Field>, Vec<String>)>,
    underlying: Option<String>,
}

struct Field {
    name: String,
    ty: String,
    private: bool,
}

/// The view of a record, enum or newtype; with `public`, only a `pub` one
/// and its public fields.
fn view(item: &ast::Item, sources: &SourceMap, public: bool) -> Option<View> {
    let text = |ty: &ast::TypeExpr| sources.slice(ty.span).to_string();
    let fields = |declared: &[ast::FieldDecl]| -> Vec<Field> {
        declared
            .iter()
            .filter(|f| !(public && f.private))
            .map(|f| Field { name: f.name.name.clone(), ty: text(&f.ty), private: f.private })
            .collect()
    };
    let generics = |declared: &[ast::GenericParam]| -> Vec<(String, Vec<String>)> {
        declared.iter().map(|g| (g.name.name.clone(), g.bounds.iter().map(text).collect())).collect()
    };
    let mut view = View {
        name: String::new(),
        kind: "record",
        generics: Vec::new(),
        fields: Vec::new(),
        variants: Vec::new(),
        underlying: None,
    };
    match item {
        ast::Item::Type(t) if !t.foreign && (t.public || !public) => {
            view.name = t.name.name.rsplit('.').next().unwrap_or_default().to_string();
            view.generics = generics(&t.generics);
            match &t.body {
                ast::TypeBody::Record(declared) => view.fields = fields(declared),
                ast::TypeBody::Newtype(inner) => {
                    view.kind = "newtype";
                    view.underlying = Some(text(inner));
                }
                ast::TypeBody::Opaque => return None,
            }
        }
        ast::Item::Enum(e) if e.public || !public => {
            view.name = e.name.name.rsplit('.').next().unwrap_or_default().to_string();
            view.kind = "enum";
            view.generics = generics(&e.generics);
            view.variants = e
                .variants
                .iter()
                .map(|v| match &v.payload {
                    ast::VariantPayload::Unit => (v.name.name.clone(), Vec::new(), Vec::new()),
                    ast::VariantPayload::Tuple(types) => {
                        (v.name.name.clone(), Vec::new(), types.iter().map(text).collect())
                    }
                    ast::VariantPayload::Record(declared) => (v.name.name.clone(), fields(declared), Vec::new()),
                })
                .collect();
        }
        _ => return None,
    }
    Some(view)
}

/// The declaration a `derive` names: the type as written, and its view with
/// the binders and bounds of the `derive`.
struct Target {
    written: String,
    view: View,
}

/// The record, enum or newtype of this module the `derive` names, with
/// exactly its binders (`E2101`).
fn target(
    modules: &[ModuleSource],
    module: usize,
    derive: &ast::DeriveDecl,
    sources: &SourceMap,
    diags: &mut Diagnostics,
) -> Option<Target> {
    let written = sources.slice(derive.target.span).to_string();
    let wrong = |diags: &mut Diagnostics, why: &str| {
        let message = format!("`derive` names `{written}`, which {why}");
        diags.error("E2101", derive.target.span, message);
    };
    let ast::TypeKind::Path { segments, args } = &derive.target.kind else {
        wrong(diags, "is not a record, enum or newtype of this module");
        return None;
    };
    let [name] = segments.as_slice() else {
        wrong(diags, "is not a record, enum or newtype of this module");
        return None;
    };
    let binders: Vec<&str> = derive.generics.iter().map(|g| g.name.name.as_str()).collect();
    let given: Vec<String> = args.iter().map(|a| sources.slice(a.span).to_string()).collect();
    if given != binders {
        wrong(diags, "must be written with exactly the binders of `derive`, in order: `derive[T] Trait for Type[T]`");
        return None;
    }
    let found = modules[module].files.iter().flat_map(|(f, _)| &f.items).find_map(|item| match item {
        ast::Item::Type(t) if t.name.name == name.name => view(item, sources, false),
        ast::Item::Enum(e) if e.name.name == name.name => view(item, sources, false),
        _ => None,
    });
    let Some(mut view) = found else {
        wrong(diags, "is not a record, enum or newtype of this module");
        return None;
    };
    if view.generics.len() != binders.len() {
        wrong(diags, &format!("has {} type parameter(s)", view.generics.len()));
        return None;
    }
    let text = |ty: &ast::TypeExpr| sources.slice(ty.span).to_string();
    view.generics =
        derive.generics.iter().map(|g| (g.name.name.clone(), g.bounds.iter().map(text).collect())).collect();
    Some(Target { written, view })
}

/// The provider of the trait: the file its package names in `[derive]`
/// (`E2102`), once the trait is one (`E2103`).
#[allow(clippy::too_many_arguments)]
fn provider(
    modules: &[ModuleSource],
    packages: &[PackageInfo],
    module: usize,
    file: usize,
    trait_ref: &ast::TypeExpr,
    written: &str,
    providers: &[Vec<(String, PathBuf)>],
    diags: &mut Diagnostics,
) -> Option<PathBuf> {
    let ast::TypeKind::Path { segments, args } = &trait_ref.kind else {
        diags.error("E2103", trait_ref.span, format!("`{written}` is not a trait"));
        return None;
    };
    let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
    // The module that declares it: this one, or one the file imports under
    // the name before the trait's.
    let (declaring, name) = match names.as_slice() {
        [name] => (Some(module), *name),
        [qualifier, name] => {
            let declaring = imported(modules, packages, module, file, qualifier)
                .and_then(|(package, path)| modules.iter().position(|m| m.package == package && m.path == path));
            (declaring, *name)
        }
        _ => (None, ""),
    };
    let is_trait = declaring.is_some_and(|m| {
        modules[m].files.iter().any(|(f, _)| {
            f.items
                .iter()
                .any(|i| matches!(i, ast::Item::Trait(t) if t.name.name == name && t.generics.len() == args.len()))
        })
    });
    let Some(declaring) = declaring.filter(|_| is_trait) else {
        let message = match declaring {
            Some(found) => format!("the module `{}` declares no trait `{name}`", modules[found].path.join(".")),
            None => format!("`{written}` is not a trait this file can name: import the module that declares it"),
        };
        diags.error("E2103", trait_ref.span, message);
        return None;
    };
    let package = modules[declaring].package;
    match providers.get(package).and_then(|p| p.iter().find(|(t, _)| t == name)) {
        Some((_, file)) => Some(file.clone()),
        None => {
            let message = format!(
                "`{written}` has no provider: the package that declares it names one in its tondo.toml, as `[derive]` with `{name} = \"derive/{}.to\"`",
                name.to_lowercase()
            );
            diags.error("E2102", trait_ref.span, message);
            None
        }
    }
}

/// The package and path of the module a file imports under `qualifier`,
/// as modules are linked: the import's first segment names this package or
/// a dependency, and the rest a module inside it.
fn imported(
    modules: &[ModuleSource],
    packages: &[PackageInfo],
    module: usize,
    file: usize,
    qualifier: &str,
) -> Option<(usize, Vec<String>)> {
    let source = &modules[module].files[file].0;
    let import =
        source.imports.iter().find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == qualifier)?;
    let path: Vec<&str> = import.path.iter().map(|p| p.name.as_str()).collect();
    let package = &packages[modules[module].package];
    let target = if path[0] == package.name { modules[module].package } else { *package.dependencies.get(path[0])? };
    let full = std::iter::once(packages[target].name.clone()).chain(path[1..].iter().map(|s| s.to_string())).collect();
    Some((target, full))
}

fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn fields_json(fields: &[Field]) -> String {
    let fields: Vec<String> = fields
        .iter()
        .map(|f| format!("{{\"name\":{},\"type\":{},\"private\":{}}}", json(&f.name), json(&f.ty), f.private))
        .collect();
    format!("[{}]", fields.join(","))
}

/// A view as JSON, which `std.meta` reads.
fn view_json(view: &View, module: &str) -> String {
    let generics: Vec<String> = view
        .generics
        .iter()
        .map(|(name, bounds)| {
            let bounds: Vec<String> = bounds.iter().map(|b| json(b)).collect();
            format!("{{\"name\":{},\"bounds\":[{}]}}", json(name), bounds.join(","))
        })
        .collect();
    let variants: Vec<String> = view
        .variants
        .iter()
        .map(|(name, fields, elements)| {
            let elements: Vec<String> = elements.iter().map(|e| json(e)).collect();
            format!(
                "{{\"name\":{},\"fields\":{},\"elements\":[{}]}}",
                json(name),
                fields_json(fields),
                elements.join(",")
            )
        })
        .collect();
    let underlying = view.underlying.as_deref().map_or("null".to_string(), json);
    format!(
        "{{\"name\":{},\"module\":{},\"kind\":{},\"generics\":[{}],\"fields\":{},\"variants\":[{}],\"underlying\":{}}}",
        json(&view.name),
        json(module),
        json(view.kind),
        generics.join(","),
        fields_json(&view.fields),
        variants.join(","),
        underlying
    )
}

/// The request of a provider.
fn request(trait_text: &str, target: &Target, module: &[String]) -> String {
    format!(
        "{{\"trait\":{},\"target\":{},\"limits\":{{\"output\":{}}}}}",
        json(trait_text),
        view_json(&target.view, &module.join(".")),
        DEFAULT.output
    )
}

/// What a provider or generator gave back: its source or the modules it
/// wrote, and the problems it found.
struct Response {
    source: String,
    files: Vec<(String, String)>,
    problems: Vec<String>,
}

enum Failure {
    /// The budget it spent: its key in `limits`, and how much it was.
    Budget(&'static str, String),
    /// Why it could not be built or did not answer.
    Provider(String),
    /// What it uses that the `meta` profile does not have.
    Denied(String),
}

/// Builds the program, or takes it from the cache, and runs it.
fn run(program: &Path, entry: Entry, request: &str, budgets: Budgets) -> Result<Response, Failure> {
    let executable = build(program, entry).map_err(|why| match why.contains("error[E2108]") {
        true => Failure::Denied(why),
        false => Failure::Provider(why),
    })?;
    let dir = std::env::temp_dir().join(format!("tondo-meta-{}-{}", std::process::id(), unique()));
    std::fs::create_dir_all(&dir).map_err(|e| Failure::Provider(e.to_string()))?;
    let (request_file, response_file) = (dir.join("request.json"), dir.join("response"));
    std::fs::write(&request_file, request).map_err(|e| Failure::Provider(e.to_string()))?;
    let output = Command::new(&executable)
        .env_clear()
        .env("TONDO_META_REQUEST", &request_file)
        .env("TONDO_META_RESPONSE", &response_file)
        .env("TONDO_META_STEPS", budgets.steps.to_string())
        .env("TONDO_META_MEMORY", budgets.memory.to_string())
        .current_dir(&dir)
        .output();
    let response = std::fs::read(&response_file);
    let _ = std::fs::remove_dir_all(&dir);
    let output = output.map_err(|e| Failure::Provider(format!("cannot run it: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(70) {
        return Err(match stderr.contains("memory") {
            true => Failure::Budget("memory", format!("{} bytes of memory", budgets.memory)),
            false => Failure::Budget("steps", format!("{} steps", budgets.steps)),
        });
    }
    if !output.status.success() {
        // A panic's message and where it happened, or the last thing it said.
        let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let why = match lines.iter().position(|l| l.starts_with("panic[")) {
            Some(at) => match lines.get(at + 1) {
                Some(place) if place.starts_with("-->") => {
                    format!("{} ({})", lines[at], place.trim_start_matches("--> "))
                }
                _ => lines[at].to_string(),
            },
            None => lines.last().map_or("it ended without an answer".to_string(), |l| l.to_string()),
        };
        return Err(Failure::Provider(why));
    }
    let response = response.map_err(|_| Failure::Provider("it gave no response".into()))?;
    let response = parse_response(&response)
        .ok_or_else(|| Failure::Provider("its response is not what std.meta writes".into()))?;
    let size = response.source.len() + response.files.iter().map(|(_, source)| source.len()).sum::<usize>();
    if size as u64 > budgets.output {
        return Err(Failure::Budget("output", format!("{} bytes of source", budgets.output)));
    }
    Ok(response)
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Sections of `kind length\n` and `length` bytes, then a newline.
fn parse_response(bytes: &[u8]) -> Option<Response> {
    let mut response = Response { source: String::new(), files: Vec::new(), problems: Vec::new() };
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest.iter().position(|&b| b == b'\n')?;
        let header = std::str::from_utf8(&rest[..end]).ok()?;
        let (kind, length) = header.split_once(' ')?;
        let length: usize = length.parse().ok()?;
        let body = rest.get(end + 1..end + 1 + length)?;
        let text = String::from_utf8(body.to_vec()).ok()?;
        match kind {
            "source" => response.source = text,
            "error" => response.problems.push(text),
            _ => response.files.push((kind.strip_prefix("output:")?.to_string(), text)),
        }
        rest = rest.get(end + 2 + length..)?;
    }
    Some(response)
}

/// The program's executable: built from its file with a `main` that serves
/// the request, for the `meta` profile, once per compiler and source.
fn build(provider: &Path, entry: Entry) -> Result<PathBuf, String> {
    let (function, serve) = match entry {
        Entry::Provide => ("provide", "serveDerive"),
        Entry::Generate => ("generate", "serveGenerate"),
    };
    let text = std::fs::read_to_string(provider).map_err(|e| format!("cannot read {}: {e}", provider.display()))?;
    let alias = meta_alias(&text).ok_or_else(|| {
        format!("{} does not import std.meta, which its `{function}` function takes and gives", provider.display())
    })?;
    let source = format!("{text}\n\nfn main() {{\n    {alias}.{serve}({function})\n}}\n");
    let identity = crate::driver::compiler_identity();
    let key = crate::driver::fingerprint(&[identity.as_bytes(), source.as_bytes()]);
    let dir = crate::driver::cache_dir().join("meta").join(key);
    let executable = dir.join("provider");
    if executable.is_file() {
        return Ok(executable);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = provider.file_name().map_or("provider.to".into(), |n| n.to_string_lossy().into_owned());
    let file = dir.join(&name);
    std::fs::write(&file, &source).map_err(|e| e.to_string())?;
    let analysis = crate::driver::analyze_meta(&file)?;
    if analysis.diags.has_errors() {
        let shown = analysis
            .diags
            .render(&analysis.sources)
            .replace(&file.display().to_string(), &provider.display().to_string());
        return Err(format!("it does not compile:\n{shown}"));
    }
    let object = crate::driver::object(&analysis)?;
    let partial = dir.join(format!("provider.partial-{}", std::process::id()));
    crate::driver::link(&object, &partial, &analysis.libraries, false)?;
    std::fs::rename(&partial, &executable).map_err(|e| e.to_string())?;
    Ok(executable)
}

/// The alias the provider imports `std.meta` under.
fn meta_alias(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("import std.meta")?;
        match rest.trim().strip_prefix("as") {
            Some(alias) => Some(alias.trim().to_string()),
            None if rest.trim().is_empty() => Some("meta".to_string()),
            None => None,
        }
    })
}

/// The items of the source, when they are exactly the `impl` asked for,
/// and the source in its canonical form.
fn accept(
    source: &str,
    trait_text: &str,
    target: &str,
    name: &str,
    sources: &mut SourceMap,
) -> Option<(Vec<ast::Item>, String)> {
    let id = sources.add(Path::new(name), source.to_string());
    sources.set_identity(id, format!("<derived>/{name}"), name.to_string());
    let mut errors = Diagnostics::default();
    let parsed = crate::parser::parse_file(sources.file(id), &mut errors);
    let squeeze = |text: &str| text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let ok = !errors.has_errors()
        && parsed.imports.is_empty()
        && parsed.statements.is_empty()
        && matches!(parsed.items.as_slice(), [ast::Item::Impl(i)]
            if squeeze(sources.slice(i.trait_ref.span)) == squeeze(trait_text)
                && squeeze(sources.slice(i.target.span)) == squeeze(target));
    if !ok {
        return None;
    }
    // Shown in its canonical form.
    let formatted =
        crate::fmt::format(sources.file(id), &mut Diagnostics::default()).unwrap_or_else(|| source.to_string());
    Some((parsed.items, formatted))
}
