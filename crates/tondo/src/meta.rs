//! `derive` (27.2 to 27.4 of the language): before the program is checked,
//! each `derive` asks the provider of each trait it names for an `impl` for
//! its type. The package that declares a trait names its provider in
//! `[derive]` of its `tondo.toml`. The provider is a Tondo program built for
//! the `meta` profile and run in a process of its own, with the request in a
//! file and its response in another; what it gives back must be exactly the
//! `impl` asked for, which joins the type's module.

use crate::ast;
use crate::diag::Diagnostics;
use crate::modules::{ModuleSource, PackageInfo};
use crate::source::SourceMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The budgets of a provider's run (27.7).
const STEPS: u64 = 100_000_000;
const MEMORY: u64 = 1 << 30;
const OUTPUT: u64 = 1 << 20;

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
                    match run(&provider, &request) {
                        Ok(Response { source, problems }) if problems.is_empty() => {
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
                        Err(Failure::Budget(what)) => {
                            let message = format!(
                                "the provider of `{trait_text}` spent its {what} budget for `{}`",
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

/// The declaration a `derive` names, as its request describes it.
struct Target {
    /// The type as written: `Page[T]`.
    written: String,
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
    let text = |ty: &ast::TypeExpr| sources.slice(ty.span).to_string();
    let fields = |declared: &[ast::FieldDecl]| -> Vec<Field> {
        declared.iter().map(|f| Field { name: f.name.name.clone(), ty: text(&f.ty), private: f.private }).collect()
    };
    let generics = derive.generics.iter().map(|g| (g.name.name.clone(), g.bounds.iter().map(text).collect())).collect();
    for (file, _) in &modules[module].files {
        for item in &file.items {
            let mut target = Target {
                written: written.clone(),
                name: name.name.clone(),
                kind: "record",
                generics: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
                underlying: None,
            };
            match item {
                ast::Item::Type(t) if t.name.name == name.name && !t.foreign => match &t.body {
                    ast::TypeBody::Record(declared) => target.fields = fields(declared),
                    ast::TypeBody::Newtype(inner) => {
                        target.kind = "newtype";
                        target.underlying = Some(text(inner));
                    }
                    ast::TypeBody::Opaque => continue,
                },
                ast::Item::Enum(e) if e.name.name == name.name => {
                    target.kind = "enum";
                    target.variants = e
                        .variants
                        .iter()
                        .map(|v| match &v.payload {
                            ast::VariantPayload::Unit => (v.name.name.clone(), Vec::new(), Vec::new()),
                            ast::VariantPayload::Tuple(types) => {
                                (v.name.name.clone(), Vec::new(), types.iter().map(text).collect())
                            }
                            ast::VariantPayload::Record(declared) => {
                                (v.name.name.clone(), fields(declared), Vec::new())
                            }
                        })
                        .collect();
                }
                _ => continue,
            }
            let declared = match item {
                ast::Item::Type(t) => t.generics.len(),
                ast::Item::Enum(e) => e.generics.len(),
                _ => 0,
            };
            if declared != binders.len() {
                wrong(diags, &format!("has {declared} type parameter(s)"));
                return None;
            }
            target.generics = generics;
            return Some(target);
        }
    }
    wrong(diags, "is not a record, enum or newtype of this module");
    None
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
            let source = &modules[module].files[file].0;
            let import =
                source.imports.iter().find(|i| i.alias.as_ref().unwrap_or(i.path.last().unwrap()).name == *qualifier);
            // As modules are linked: the first segment names this package
            // or a dependency, and the rest a module inside it.
            let declaring = import.and_then(|i| {
                let path: Vec<&str> = i.path.iter().map(|p| p.name.as_str()).collect();
                let package = &packages[modules[module].package];
                let target = if path[0] == package.name {
                    modules[module].package
                } else {
                    *package.dependencies.get(path[0])?
                };
                let full: Vec<String> = std::iter::once(packages[target].name.clone())
                    .chain(path[1..].iter().map(|s| s.to_string()))
                    .collect();
                modules.iter().position(|m| m.package == target && m.path == full)
            });
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

/// The request as JSON, which `std.meta` reads.
fn request(trait_text: &str, target: &Target, module: &[String]) -> String {
    let generics: Vec<String> = target
        .generics
        .iter()
        .map(|(name, bounds)| {
            let bounds: Vec<String> = bounds.iter().map(|b| json(b)).collect();
            format!("{{\"name\":{},\"bounds\":[{}]}}", json(name), bounds.join(","))
        })
        .collect();
    let variants: Vec<String> = target
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
    let underlying = target.underlying.as_deref().map_or("null".to_string(), json);
    format!(
        "{{\"trait\":{},\"target\":{{\"name\":{},\"module\":{},\"kind\":{},\"generics\":[{}],\"fields\":{},\"variants\":[{}],\"underlying\":{}}},\"limits\":{{\"output\":{OUTPUT}}}}}",
        json(trait_text),
        json(&target.name),
        json(&module.join(".")),
        json(target.kind),
        generics.join(","),
        fields_json(&target.fields),
        variants.join(","),
        underlying
    )
}

/// What a provider gave back: its source, or the problems it found.
struct Response {
    source: String,
    problems: Vec<String>,
}

enum Failure {
    /// The budget it spent: `step` or `memory`.
    Budget(String),
    /// Why it could not be built or did not answer.
    Provider(String),
    /// What it uses that the `meta` profile does not have.
    Denied(String),
}

/// Builds the provider, or takes it from the cache, and runs it.
fn run(provider: &Path, request: &str) -> Result<Response, Failure> {
    let executable = build(provider).map_err(|why| match why.contains("error[E2108]") {
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
        .env("TONDO_META_STEPS", STEPS.to_string())
        .env("TONDO_META_MEMORY", MEMORY.to_string())
        .current_dir(&dir)
        .output();
    let response = std::fs::read(&response_file);
    let _ = std::fs::remove_dir_all(&dir);
    let output = output.map_err(|e| Failure::Provider(format!("cannot run it: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(70) {
        let what = if stderr.contains("memory") { "memory" } else { "step" };
        return Err(Failure::Budget(what.to_string()));
    }
    if !output.status.success() {
        let why = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it ended without an answer");
        return Err(Failure::Provider(why.trim().to_string()));
    }
    let response = response.map_err(|_| Failure::Provider("it gave no response".into()))?;
    parse_response(&response).ok_or_else(|| Failure::Provider("its response is not what std.meta writes".into()))
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Sections of `kind length\n` and `length` bytes, then a newline.
fn parse_response(bytes: &[u8]) -> Option<Response> {
    let mut response = Response { source: String::new(), problems: Vec::new() };
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
            _ => return None,
        }
        rest = rest.get(end + 2 + length..)?;
    }
    Some(response)
}

/// The provider's executable: built from its file with a `main` that serves
/// the request, for the `meta` profile, once per compiler and source.
fn build(provider: &Path) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(provider).map_err(|e| format!("cannot read {}: {e}", provider.display()))?;
    let alias = meta_alias(&text).ok_or_else(|| {
        format!("{} does not import std.meta, which its `provide` function takes and gives", provider.display())
    })?;
    let source = format!("{text}\n\nfn main() {{\n    {alias}.serveDerive(provide)\n}}\n");
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
    crate::driver::link(&object, &partial, &analysis.libraries)?;
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
