//! A state machine of the program checked with no model to write (library
//! 14.43): `tondo model <function>`, the editor's lens and the tool of
//! `tondo mcp` find a function that gives the next state for an event,
//! `fn(state: S, event: E): S`, or `S ! X`, and write the test that goes
//! through every state it reaches with `model.machine`, with a start state
//! built from its type.

use crate::ast::{self, Item, TypeBody, TypeExpr, TypeKind, VariantPayload};
use crate::diag::Diagnostics;
use crate::source::SourceMap;
use std::path::{Path, PathBuf};

/// The test of a state machine, and where it goes.
pub struct Scaffold {
    /// The file of tests next to the function's.
    pub file: PathBuf,
    /// The file as it is to be, with the test.
    pub text: String,
    /// The name of the test.
    pub test: String,
    /// It was there already, as the program may have added to it: it runs as
    /// it is.
    pub existed: bool,
}

/// The declarations of the files of a module, by name.
struct Declared<'a> {
    files: Vec<&'a ast::Module>,
}

impl Declared<'_> {
    fn record(&self, name: &str) -> Option<&Vec<ast::FieldDecl>> {
        self.files.iter().flat_map(|f| &f.items).find_map(|item| match item {
            Item::Type(t) if t.name.name == name => match &t.body {
                TypeBody::Record(fields) => Some(fields),
                _ => None,
            },
            _ => None,
        })
    }

    fn newtype(&self, name: &str) -> Option<&TypeExpr> {
        self.files.iter().flat_map(|f| &f.items).find_map(|item| match item {
            Item::Type(t) if t.name.name == name => match &t.body {
                TypeBody::Newtype(inner) => Some(inner),
                _ => None,
            },
            _ => None,
        })
    }

    fn enumeration(&self, name: &str) -> Option<&ast::EnumDecl> {
        self.files.iter().flat_map(|f| &f.items).find_map(|item| match item {
            Item::Enum(e) if e.name.name == name => Some(e),
            _ => None,
        })
    }

    /// A value of a type, the plainest there is: what a state starts with.
    fn start(&self, ty: &TypeExpr, depth: usize) -> Result<String, String> {
        if depth > 8 {
            return Err("its state holds itself; write the start state".into());
        }
        let values = |types: &[TypeExpr]| -> Result<Vec<String>, String> {
            types.iter().map(|ty| self.start(ty, depth + 1)).collect()
        };
        let fields = |fields: &[ast::FieldDecl]| -> Result<String, String> {
            let written: Result<Vec<String>, String> =
                fields.iter().map(|f| Ok(format!("{}: {}", f.name.name, self.start(&f.ty, depth + 1)?))).collect();
            Ok(written?.join(", "))
        };
        match &ty.kind {
            TypeKind::Optional(_) => Ok("none".into()),
            TypeKind::Tuple(items) => Ok(format!("({})", values(items)?.join(", "))),
            TypeKind::Path { segments, .. } => {
                let names: Vec<&str> = segments.iter().map(|s| s.name.as_str()).collect();
                match names.as_slice() {
                    ["Bool"] => Ok("false".into()),
                    ["String"] => Ok("\"\"".into()),
                    ["Float" | "Float64" | "Float32"] => Ok("0.0".into()),
                    [name] if crate::types::IntTy::from_name(name).is_some() || *name == "Byte" => Ok("0".into()),
                    [_, "Queue"] => Ok("model.Queue([])".into()),
                    [_, "Bag"] => Ok("model.Bag([])".into()),
                    [name] => {
                        if let Some(record) = self.record(name) {
                            return Ok(format!("{name} {{ {} }}", fields(record)?));
                        }
                        if let Some(inner) = self.newtype(name) {
                            return Ok(format!("{name}({})", self.start(inner, depth + 1)?));
                        }
                        let Some(e) = self.enumeration(name) else {
                            return Err(format!("`{name}` is not a type of this module; write the start state"));
                        };
                        let first = e.variants.first().ok_or_else(|| format!("`{name}` has no variants"))?;
                        Ok(match &first.payload {
                            VariantPayload::Unit => format!("{name}.{}", first.name.name),
                            VariantPayload::Tuple(items) => {
                                format!("{name}.{}({})", first.name.name, values(items)?.join(", "))
                            }
                            VariantPayload::Record(record) => {
                                format!("{name}.{} {{ {} }}", first.name.name, fields(record)?)
                            }
                        })
                    }
                    _ => Err(format!("{} has no value to start from; write the start state", names.join("."))),
                }
            }
            _ => Err("its state has no value to start from; write the start state".into()),
        }
    }
}

/// The name of the type a type expression names, if it is a plain one.
fn named(ty: &TypeExpr) -> Option<&str> {
    match &ty.kind {
        TypeKind::Path { segments, args } if segments.len() == 1 && args.is_empty() => Some(&segments[0].name),
        _ => None,
    }
}

/// Whether a function is a state machine: two parameters, a state and an
/// event of an enum of the module, and the state as its result, or a result
/// of it.
fn machine<'a>(f: &'a ast::FnDecl, declared: &Declared) -> Option<(&'a TypeExpr, &'a str)> {
    let [state, event] = f.params.as_slice() else { return None };
    let (state, event) = (state.ty.as_ref()?, event.ty.as_ref()?);
    let gives = match &f.result.as_ref()?.kind {
        TypeKind::Result { ok: Some(ok), .. } => named(ok)?,
        _ => named(f.result.as_ref()?)?,
    };
    let event = named(event)?;
    (named(state)? == gives && declared.enumeration(event).is_some()).then_some((state, event))
}

/// The functions of a file that are state machines, by name and the span of
/// their name, for an editor's lens.
pub fn machines(file: &ast::Module, module: &[ast::Module]) -> Vec<(String, crate::source::Span)> {
    let declared = Declared { files: module.iter().collect() };
    file.items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(f) if f.owner.is_none() && f.generics.is_empty() && machine(f, &declared).is_some() => {
                Some((f.name.name.clone(), f.name.span))
            }
            _ => None,
        })
        .collect()
}

/// The test of the state machine `function`, written in the file of tests
/// next to its file.
pub fn scaffold(sources: &SourceMap, function: &str) -> Result<Scaffold, String> {
    let mut diags = Diagnostics::default();
    // The program's own files, parsed, by their path.
    let files: Vec<(PathBuf, ast::Module, String)> = (0..sources.len())
        .map(|i| sources.file(i))
        .filter(|file| !file.logical.starts_with("<std>/") && file.path.is_absolute())
        .map(|file| (file.path.clone(), crate::parser::parse_file(file, &mut diags), file.module.clone()))
        .collect();
    let (path, f, module) = files
        .iter()
        .find_map(|(path, parsed, module)| {
            parsed.items.iter().find_map(|item| match item {
                Item::Fn(f) if f.name.name == function && f.owner.is_none() => Some((path, f, module)),
                _ => None,
            })
        })
        .ok_or_else(|| format!("the program has no function `{function}`"))?;
    let declared =
        Declared { files: files.iter().filter(|(_, _, m)| m == module).map(|(_, parsed, _)| parsed).collect() };
    let (state, _) = machine(f, &declared).ok_or_else(|| {
        format!(
            "`{function}` is not a state machine: write it as `fn {function}(state: S, event: E): S`, or `S ! X`, \
             with `E` an enum of its module"
        )
    })?;
    let start = declared.start(state, 0)?;
    let file = test_file(path);
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let test = format!("{function}IsAStateMachine");
    if existing.contains(&format!("test {test} ")) {
        return Ok(Scaffold { file, text: existing, test, existed: true });
    }
    let body =
        format!("test {test} {{\n    let start = {start}\n    model.expect(model.machine(start, {function}))\n}}\n");
    let text = match existing.is_empty() {
        true => format!("import std.model\n\n{body}"),
        false if existing.lines().any(|l| l.trim() == "import std.model") => {
            format!("{}\n\n{body}", existing.trim_end())
        }
        false => {
            // With the other imports, which come first.
            let lines: Vec<&str> = existing.lines().collect();
            let after = lines.iter().take_while(|l| l.starts_with("import ") || l.starts_with("//")).count();
            let mut written = lines[..after].to_vec();
            written.push("import std.model");
            written.extend(&lines[after..]);
            format!("{}\n\n{body}", written.join("\n").trim_end())
        }
    };
    Ok(Scaffold { file, text, test, existed: false })
}

/// The file of tests next to a file of the program: `door_test.to` for
/// `door.to`.
fn test_file(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("main");
    path.with_file_name(format!("{stem}_test.to"))
}
