use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use tondo_conformance::lineage::DraftLineage;
use tondo_conformance::manifest::CaseGroup;
use tondo_conformance::runner::{ProcessAdapter, run_suite};

const USAGE: &str = "\
Tondo draft conformance runner

Usage:
  tondo-conformance validate --root <directory> --manifest <draft-path> --lineage draft
  tondo-conformance run --root <directory> --manifest <draft-path> --lineage draft --adapter <executable> [--group <group>] [--output <path>]
  tondo-conformance seal --root <directory> --manifest <draft-path> --lineage draft

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
    let mut lineage = None;
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
            "--lineage" => set_once(&mut lineage, parse_lineage(value)?, option)?,
            "--adapter" => set_once(&mut adapter, PathBuf::from(value), option)?,
            "--group" => set_once(&mut group, parse_group(value)?, option)?,
            "--output" => set_once(&mut output, PathBuf::from(value), option)?,
            _ => return Err(format!("unknown option `{option}`\n\n{USAGE}")),
        }
    }
    let root = root.ok_or_else(|| "`--root` is required".to_owned())?;
    let manifest = manifest.ok_or_else(|| "`--manifest` is required".to_owned())?;
    let selection = lineage.ok_or_else(|| "`--lineage` is required".to_owned())?;
    let lineage = DraftLineage::load(root, manifest).map_err(|error| error.to_string())?;
    match command.as_str() {
        "validate" => {
            if adapter.is_some() || group.is_some() || output.is_some() {
                return Err("validate accepts only --root, --manifest, and --lineage".into());
            }
            let _ = selection;
            println!(
                "{} {} {} {} {}",
                lineage.manifest().lineage,
                lineage.manifest().edition,
                lineage.manifest().state,
                lineage.manifest().revision,
                lineage.manifest_sha256()
            );
            Ok(())
        }
        "run" => {
            let adapter =
                adapter.ok_or_else(|| "`--adapter` is required for the run command".to_owned())?;
            let mut adapter = ProcessAdapter::spawn(adapter)?;
            let result = run_suite(lineage.baseline_suite(), &mut adapter, group)
                .map_err(|error| error.to_string())?;
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
        "seal" => {
            if adapter.is_some() || group.is_some() || output.is_some() {
                return Err("seal accepts only --root, --manifest, and --lineage".into());
            }
            lineage
                .check_sealable()
                .map_err(|error| error.to_string())?;
            println!(
                "{} revision {} satisfies the non-mutating seal preflight",
                lineage.manifest().lineage,
                lineage.manifest().revision
            );
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineageSelection {
    Draft,
}

fn parse_lineage(value: &str) -> Result<LineageSelection, String> {
    match value {
        "draft" => Ok(LineageSelection::Draft),
        _ => Err(format!("unknown lineage `{value}`")),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn suite_arguments(command: &str) -> Vec<String> {
        vec![
            command.into(),
            "--root".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .display()
                .to_string(),
            "--manifest".into(),
            "conformance/draft/manifest.json".into(),
            "--lineage".into(),
            "draft".into(),
        ]
    }

    #[test]
    fn every_group_name_is_closed_and_round_trips() {
        for (name, expected) in [
            ("lex-parse-format", CaseGroup::LexParseFormat),
            ("compile-pass", CaseGroup::CompilePass),
            ("compile-fail", CaseGroup::CompileFail),
            ("semantic-queries", CaseGroup::SemanticQueries),
            ("runtime", CaseGroup::Runtime),
            ("concurrency", CaseGroup::Concurrency),
            ("hosted", CaseGroup::Hosted),
            ("memory", CaseGroup::Memory),
            ("determinism", CaseGroup::Determinism),
            ("documentation", CaseGroup::Documentation),
        ] {
            assert_eq!(parse_group(name).unwrap(), expected);
        }
        assert_eq!(
            parse_group("unknown").unwrap_err(),
            "unknown group `unknown`"
        );
        assert_eq!(
            parse_lineage("unknown").unwrap_err(),
            "unknown lineage `unknown`"
        );
    }

    #[test]
    fn command_arguments_are_single_assignment_and_command_scoped() {
        assert!(run(suite_arguments("validate")).is_ok());
        assert!(
            run(Vec::new())
                .unwrap_err()
                .contains("a command is required")
        );
        assert_eq!(
            run(vec!["validate".into(), "--root".into()]).unwrap_err(),
            "`--root` requires a value"
        );
        assert!(
            run(vec!["validate".into(), "--unknown".into(), "x".into()])
                .unwrap_err()
                .contains("unknown option")
        );

        let mut duplicate = suite_arguments("validate");
        duplicate.extend(["--root".into(), ".".into()]);
        assert_eq!(run(duplicate).unwrap_err(), "`--root` may appear only once");

        let mut validate_with_adapter = suite_arguments("validate");
        validate_with_adapter.extend(["--adapter".into(), "adapter".into()]);
        assert_eq!(
            run(validate_with_adapter).unwrap_err(),
            "validate accepts only --root, --manifest, and --lineage"
        );

        assert_eq!(
            run(suite_arguments("run")).unwrap_err(),
            "`--adapter` is required for the run command"
        );
        assert!(
            run(suite_arguments("other"))
                .unwrap_err()
                .contains("unknown command `other`")
        );

        let seal = suite_arguments("seal");
        assert!(
            run(seal)
                .unwrap_err()
                .contains("still has pending tasks: M10.6",)
        );
    }
}
