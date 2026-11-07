//! The contract between compiled code and the runtime: every function of
//! the runtime the compiler calls, by the signatures `codegen.rs` declares
//! and those of `NATIVES`, is one the runtime defines, with as many
//! arguments, of the same classes, and the same result. A function moved,
//! renamed or changed on one side only would otherwise be found by a crash
//! (docs/refactor.md, phase 0.3).

use std::collections::BTreeMap;
use std::path::Path;

/// How a value travels in a call, as Cranelift's types say it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

type Signature = (Vec<Class>, Option<Class>);

/// A type of Rust in an `extern "C"` signature, as it travels.
fn rust_class(ty: &str) -> Class {
    match ty.trim() {
        "u8" | "i8" | "bool" => Class::I8,
        "u16" | "i16" => Class::I16,
        "u32" | "i32" | "char" => Class::I32,
        "f32" => Class::F32,
        "f64" => Class::F64,
        // Integers of a word, pointers, references, function pointers and
        // the options of those.
        _ => Class::I64,
    }
}

fn clif_class(ty: &str) -> Class {
    match ty.trim().trim_start_matches("types::") {
        "I8" => Class::I8,
        "I16" => Class::I16,
        "I32" => Class::I32,
        "F32" => Class::F32,
        "F64" => Class::F64,
        "I64" => Class::I64,
        other => panic!("a type codegen.rs declares that this test does not know: {other}"),
    }
}

/// The arguments of a parameter list, split at the commas outside brackets.
fn split_top(text: &str) -> Vec<String> {
    let (mut parts, mut depth, mut current) = (Vec::new(), 0i32, String::new());
    for c in text.chars() {
        match c {
            '(' | '<' | '[' => depth += 1,
            // The arrow of a function's type closes nothing.
            '>' if current.ends_with('-') => {}
            ')' | '>' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// The files of Rust under `dir`, however deep.
fn sources(dir: &Path, into: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

/// The `extern "C"` functions of the runtime named `tondo_rt_*`, by name.
fn defined() -> BTreeMap<String, Signature> {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../tondo-runtime/src"), &mut files);
    let mut found = BTreeMap::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        // The functions `math.rs` makes with its macro, of a `Float` each.
        if let Some(at) = text.find("unary! {") {
            let block = &text[at..at + text[at..].find('}').unwrap()];
            for name in block.split_whitespace().filter(|word| word.starts_with("tondo_rt_")) {
                found.insert(name.to_string(), (vec![Class::F64], Some(Class::F64)));
            }
        }
        let mut rest = text.as_str();
        while let Some(at) = rest.find("extern \"C\" fn tondo_rt_") {
            rest = &rest[at + "extern \"C\" fn ".len()..];
            let name_end = rest.find(['(', '<']).unwrap();
            let name = rest[..name_end].to_string();
            // The parameters, to the parenthesis that closes them.
            let open = rest.find('(').unwrap();
            let (mut depth, mut close) = (0, open);
            for (i, c) in rest[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let params: Vec<Class> = split_top(&rest[open + 1..close])
                .iter()
                .map(|param| rust_class(param.split_once(':').map_or(param.as_str(), |(_, ty)| ty)))
                .collect();
            let after = &rest[close + 1..];
            let head = &after[..after.find('{').unwrap()];
            let result = head
                .trim()
                .strip_prefix("->")
                .map(|ty| ty.split(" where").next().unwrap().trim())
                .filter(|ty| *ty != "!")
                .map(rust_class);
            found.insert(name, (params, result));
            rest = after;
        }
    }
    found
}

/// The signatures `codegen.rs` declares, as `("tondo_rt_x", &[I64], &[I8])`.
fn declared() -> BTreeMap<String, Signature> {
    let text = include_str!("../src/codegen.rs");
    let mut found = BTreeMap::new();
    let mut rest = text;
    while let Some(at) = rest.find("(\"tondo_rt_") {
        rest = &rest[at + 2..];
        let name_end = rest.find('"').unwrap();
        let name = rest[..name_end].to_string();
        let tail = rest[name_end + 1..].trim_start();
        // A name alone, as a symbol looked up, is no signature.
        let Some(tail) = tail.strip_prefix(',') else { continue };
        let lists: Vec<&str> = tail.splitn(3, ']').take(2).collect();
        let list = |text: &str| -> Vec<Class> {
            let inner = text.split_once('[').map_or("", |(_, inner)| inner);
            inner.split(',').map(str::trim).filter(|t| !t.is_empty()).map(clif_class).collect()
        };
        if lists.len() < 2 || !lists[0].trim_start().starts_with('&') {
            continue;
        }
        let (params, results) = (list(lists[0]), list(lists[1]));
        assert!(results.len() <= 1, "{name} declares more than one result");
        found.insert(name, (params, results.first().copied()));
    }
    for native in tondo::hir::NATIVES {
        let class = |ty: tondo::hir::NativeTy| match ty {
            tondo::hir::NativeTy::Float => Class::F64,
            tondo::hir::NativeTy::Bool => Class::I8,
            tondo::hir::NativeTy::Char => Class::I32,
            _ => Class::I64,
        };
        let params = native.receiver.iter().chain(native.params).map(|&t| class(t)).collect();
        let result = (native.result != tondo::hir::NativeTy::Unit).then(|| class(native.result));
        found.insert(native.symbol.to_string(), (params, result));
    }
    found
}

#[test]
fn every_call_of_the_runtime_matches_its_definition() {
    let (defined, declared) = (defined(), declared());
    assert!(declared.len() > 700 && defined.len() > 700, "{} declared, {} defined", declared.len(), defined.len());
    let mut wrong = Vec::new();
    for (name, signature) in &declared {
        match defined.get(name) {
            None => wrong.push(format!("{name}: the runtime does not define it")),
            Some(found) if found != signature => {
                wrong.push(format!("{name}: declared {signature:?}, defined {found:?}"))
            }
            Some(_) => {}
        }
    }
    assert!(wrong.is_empty(), "{} calls of the runtime differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// What the compiler looks up by name, apart from its tables: the counter
/// compiled code reads before it asks whether a task must stop, the
/// metrics, and where an executable it links itself starts.
#[test]
fn the_runtime_names_what_the_compiler_looks_up() {
    let mut files = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../tondo-runtime/src"), &mut files);
    let text: String = files.iter().map(|path| std::fs::read_to_string(path).unwrap()).collect();
    for name in ["export_name = \"tondo_rt_attention\"", "fn tondo_rt_metrics", ".globl tondo_rt_start"] {
        assert!(text.contains(name), "the compiler looks up `{name}`, which the runtime does not define");
    }
}
