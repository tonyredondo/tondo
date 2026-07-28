use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use tondo_conformance::manifest::{CaseGroup, LoadedSuite};
use tondo_conformance::runner::{ProcessAdapter, run_suite};

const USAGE: &str = "\
Tondo 0.1 conformance runner

Usage:
  tondo-conformance validate --root <directory> --manifest <path>
  tondo-conformance run --root <directory> --manifest <path> --adapter <executable> [--group <group>] [--output <path>]

Groups:
  lex-parse-format, compile-pass, compile-fail, semantic-queries, runtime,
  concurrency, hosted, memory, determinism, documentation";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("tondo-conformance: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    let command = arguments
        .first()
        .ok_or_else(|| format!("a command is required\n\n{USAGE}"))?;
    let mut root = None;
    let mut manifest = None;
    let mut adapter = None;
    let mut group = None;
    let mut output = None;
    let mut index = 1;
    while index < arguments.len() {
        let option = &arguments[index];
        index += 1;
        let value = arguments
            .get(index)
            .ok_or_else(|| format!("`{option}` requires a value"))?;
        index += 1;
        match option.as_str() {
            "--root" => set_once(&mut root, PathBuf::from(value), option)?,
            "--manifest" => set_once(&mut manifest, PathBuf::from(value), option)?,
            "--adapter" => set_once(&mut adapter, PathBuf::from(value), option)?,
            "--group" => set_once(&mut group, parse_group(value)?, option)?,
            "--output" => set_once(&mut output, PathBuf::from(value), option)?,
            _ => return Err(format!("unknown option `{option}`\n\n{USAGE}")),
        }
    }
    let root = root.ok_or_else(|| "`--root` is required".to_owned())?;
    let manifest = manifest.ok_or_else(|| "`--manifest` is required".to_owned())?;
    let suite = LoadedSuite::load(root, manifest).map_err(|error| error.to_string())?;
    match command.as_str() {
        "validate" => {
            if adapter.is_some() || group.is_some() || output.is_some() {
                return Err("validate accepts only --root and --manifest".into());
            }
            println!(
                "{} {} {}",
                suite.manifest().suite,
                suite.manifest().version,
                suite.manifest_sha256()
            );
            Ok(())
        }
        "run" => {
            let adapter =
                adapter.ok_or_else(|| "`--adapter` is required for the run command".to_owned())?;
            let mut adapter = ProcessAdapter::spawn(adapter)?;
            let result =
                run_suite(&suite, &mut adapter, group).map_err(|error| error.to_string())?;
            let encoded = serde_json::to_vec(&result)
                .map_err(|error| format!("cannot encode result: {error}"))?;
            if let Some(path) = output {
                fs::write(&path, &encoded)
                    .map_err(|error| format!("cannot write `{}`: {error}", path.display()))?;
            } else {
                println!("{}", String::from_utf8_lossy(&encoded));
            }
            Ok(())
        }
        _ => Err(format!("unknown command `{command}`\n\n{USAGE}")),
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        Err(format!("`{name}` may appear only once"))
    } else {
        Ok(())
    }
}

fn parse_group(value: &str) -> Result<CaseGroup, String> {
    match value {
        "lex-parse-format" => Ok(CaseGroup::LexParseFormat),
        "compile-pass" => Ok(CaseGroup::CompilePass),
        "compile-fail" => Ok(CaseGroup::CompileFail),
        "semantic-queries" => Ok(CaseGroup::SemanticQueries),
        "runtime" => Ok(CaseGroup::Runtime),
        "concurrency" => Ok(CaseGroup::Concurrency),
        "hosted" => Ok(CaseGroup::Hosted),
        "memory" => Ok(CaseGroup::Memory),
        "determinism" => Ok(CaseGroup::Determinism),
        "documentation" => Ok(CaseGroup::Documentation),
        _ => Err(format!("unknown group `{value}`")),
    }
}
