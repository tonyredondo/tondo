use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tondo_reliability::inventory;
use tondo_reliability::matrix;
use tondo_reliability::quality::{QualityBaseline, capture, parse_llvm_cov, parse_mutation_report};
use tondo_reliability::regression::RegressionLedger;
use tondo_reliability::{
    INVENTORY_PATH, MATRIX_PATH, QUALITY_BASELINE_PATH, REGRESSION_LEDGER_PATH, canonical_json,
    check_bytes, workspace_root, write_if_changed,
};

const USAGE: &str = "\
Tondo reliability tooling

Usage:
  tondo-reliability generate [--root <directory>]
  tondo-reliability check [--root <directory>]
  tondo-reliability inventory <generate|check> [--root <directory>]
  tondo-reliability matrix <generate|check> [--root <directory>]
  tondo-reliability quality check [--root <directory>]
  tondo-reliability quality capture --coverage <json> --mutants <json> --revision <id> [--root <directory>]
  tondo-reliability quality verify --coverage <json> [--mutants <json>] [--root <directory>]";

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("tondo-reliability: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(arguments: Vec<String>) -> Result<String, String> {
    let arguments = parse_arguments(arguments)?;
    let root = workspace_root(&arguments.root)?;
    if arguments.positionals.first().map(String::as_str) != Some("quality") {
        reject_quality_options(&arguments)?;
    }
    match arguments.positionals.as_slice() {
        [command] if command == "generate" => generate_all(&root),
        [command] if command == "check" => check_all(&root),
        [area, command] if area == "inventory" && command == "generate" => {
            generate_inventory(&root)
        }
        [area, command] if area == "inventory" && command == "check" => check_inventory(&root),
        [area, command] if area == "matrix" && command == "generate" => generate_matrix(&root),
        [area, command] if area == "matrix" && command == "check" => check_matrix(&root),
        [area, command] if area == "quality" && command == "check" => {
            reject_quality_options(&arguments)?;
            let path = root.join(QUALITY_BASELINE_PATH);
            QualityBaseline::load(&path)?;
            Ok(format!("quality baseline is valid: {}", path.display()))
        }
        [area, command] if area == "quality" && command == "capture" => {
            let coverage = required_path(&arguments.coverage, "--coverage")?;
            let mutants = required_path(&arguments.mutants, "--mutants")?;
            let revision = arguments
                .revision
                .as_deref()
                .ok_or_else(|| "`--revision` is required".to_owned())?;
            let baseline = capture(
                revision,
                &std::fs::read(coverage)
                    .map_err(|error| format!("cannot read `{}`: {error}", coverage.display()))?,
                &std::fs::read(mutants)
                    .map_err(|error| format!("cannot read `{}`: {error}", mutants.display()))?,
            )?;
            let changed = write_if_changed(
                &root.join(QUALITY_BASELINE_PATH),
                &canonical_json(&baseline)?,
            )?;
            Ok(format!(
                "quality baseline {}: line coverage {} bp, mutation score {} bp",
                change(changed),
                baseline.coverage.global.lines.basis_points,
                baseline.mutation.score_basis_points
            ))
        }
        [area, command] if area == "quality" && command == "verify" => {
            if arguments.revision.is_some() {
                return Err("quality verify does not accept `--revision`".into());
            }
            let coverage_path = required_path(&arguments.coverage, "--coverage")?;
            let coverage =
                parse_llvm_cov(&std::fs::read(coverage_path).map_err(|error| {
                    format!("cannot read `{}`: {error}", coverage_path.display())
                })?)?;
            let baseline = QualityBaseline::load(&root.join(QUALITY_BASELINE_PATH))?;
            baseline.verify_coverage_report(&coverage)?;
            if let Some(mutants_path) = &arguments.mutants {
                let mutation =
                    parse_mutation_report(&std::fs::read(mutants_path).map_err(|error| {
                        format!("cannot read `{}`: {error}", mutants_path.display())
                    })?)?;
                baseline.verify_mutation_report(&mutation)?;
            }
            Ok(format!(
                "quality gate passed: line coverage {} bp{}",
                coverage.global.lines.basis_points,
                arguments
                    .mutants
                    .as_ref()
                    .map(|_| " and mutation score")
                    .unwrap_or("")
            ))
        }
        _ => Err(format!("invalid command\n\n{USAGE}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    positionals: Vec<String>,
    root: PathBuf,
    coverage: Option<PathBuf>,
    mutants: Option<PathBuf>,
    revision: Option<String>,
}

fn parse_arguments(arguments: Vec<String>) -> Result<Arguments, String> {
    let mut positionals = Vec::new();
    let mut root = None;
    let mut coverage = None;
    let mut mutants = None;
    let mut revision = None;
    let mut index = 0;
    while index < arguments.len() {
        if matches!(
            arguments[index].as_str(),
            "--root" | "--coverage" | "--mutants" | "--revision"
        ) {
            let option = arguments[index].as_str();
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("`{option}` requires a value"))?;
            let duplicate = match option {
                "--root" => root.replace(PathBuf::from(value)).is_some(),
                "--coverage" => coverage.replace(PathBuf::from(value)).is_some(),
                "--mutants" => mutants.replace(PathBuf::from(value)).is_some(),
                "--revision" => revision.replace(value.clone()).is_some(),
                _ => unreachable!(),
            };
            if duplicate {
                return Err(format!("`{option}` may appear only once"));
            }
            index += 2;
        } else if arguments[index].starts_with('-') {
            return Err(format!("unknown option `{}`\n\n{USAGE}", arguments[index]));
        } else {
            positionals.push(arguments[index].clone());
            index += 1;
        }
    }
    Ok(Arguments {
        positionals,
        root: root.unwrap_or(env::current_dir().map_err(|error| error.to_string())?),
        coverage,
        mutants,
        revision,
    })
}

fn reject_quality_options(arguments: &Arguments) -> Result<(), String> {
    if arguments.coverage.is_some() || arguments.mutants.is_some() || arguments.revision.is_some() {
        Err("this command does not accept quality report options".into())
    } else {
        Ok(())
    }
}

fn required_path<'a>(value: &'a Option<PathBuf>, name: &str) -> Result<&'a Path, String> {
    value
        .as_deref()
        .ok_or_else(|| format!("`{name}` is required"))
}

fn generate_all(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    validate_regressions(root, &inventory)?;
    let matrix = matrix::build(root, &inventory)?;
    let inventory_changed =
        write_if_changed(&root.join(INVENTORY_PATH), &canonical_json(&inventory)?)?;
    let matrix_changed = write_if_changed(&root.join(MATRIX_PATH), &canonical_json(&matrix)?)?;
    Ok(format!(
        "inventory {} ({} tests), matrix {} ({} requirements)",
        change(inventory_changed),
        inventory.summary.logical_tests,
        change(matrix_changed),
        matrix.summary.total
    ))
}

fn check_all(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    validate_regressions(root, &inventory)?;
    check_bytes(&root.join(INVENTORY_PATH), &canonical_json(&inventory)?)?;
    let matrix = matrix::build(root, &inventory)?;
    check_bytes(&root.join(MATRIX_PATH), &canonical_json(&matrix)?)?;
    QualityBaseline::load(&root.join(QUALITY_BASELINE_PATH))?;
    Ok(format!(
        "reliability evidence is current: {} tests, {} requirements",
        inventory.summary.logical_tests, matrix.summary.total
    ))
}

fn validate_regressions(root: &Path, inventory: &inventory::Inventory) -> Result<(), String> {
    RegressionLedger::load(&root.join(REGRESSION_LEDGER_PATH))?.validate(root, inventory)
}

fn generate_inventory(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    let changed = write_if_changed(&root.join(INVENTORY_PATH), &canonical_json(&inventory)?)?;
    Ok(format!(
        "inventory {}: {} logical tests, {} repetitions",
        change(changed),
        inventory.summary.logical_tests,
        inventory.summary.repetitions
    ))
}

fn check_inventory(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    inventory::validate(&inventory)?;
    check_bytes(&root.join(INVENTORY_PATH), &canonical_json(&inventory)?)?;
    Ok(format!(
        "inventory is current: {} logical tests",
        inventory.summary.logical_tests
    ))
}

fn generate_matrix(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    let matrix = matrix::build(root, &inventory)?;
    let changed = write_if_changed(&root.join(MATRIX_PATH), &canonical_json(&matrix)?)?;
    Ok(format!(
        "coverage matrix {}: {} requirements",
        change(changed),
        matrix.summary.total
    ))
}

fn check_matrix(root: &Path) -> Result<String, String> {
    let inventory = inventory::build(root)?;
    let matrix = matrix::build(root, &inventory)?;
    check_bytes(&root.join(MATRIX_PATH), &canonical_json(&matrix)?)?;
    Ok(format!(
        "coverage matrix is current: {} requirements",
        matrix.summary.total
    ))
}

fn change(changed: bool) -> &'static str {
    if changed { "updated" } else { "unchanged" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_order_independent_but_closed() {
        let arguments = parse_arguments(vec![
            "inventory".into(),
            "--root".into(),
            ".".into(),
            "check".into(),
        ])
        .unwrap();
        assert_eq!(arguments.positionals, ["inventory", "check"]);
        assert_eq!(arguments.root, PathBuf::from("."));
        assert_eq!(arguments.coverage, None);
        assert!(parse_arguments(vec!["--unknown".into()]).is_err());
        assert!(parse_arguments(vec!["--root".into()]).is_err());
    }

    #[test]
    fn quality_options_are_single_assignment() {
        let arguments = parse_arguments(vec![
            "quality".into(),
            "verify".into(),
            "--coverage".into(),
            "coverage.json".into(),
            "--mutants".into(),
            "outcomes.json".into(),
        ])
        .unwrap();
        assert_eq!(arguments.coverage, Some(PathBuf::from("coverage.json")));
        assert_eq!(arguments.mutants, Some(PathBuf::from("outcomes.json")));
        assert!(
            parse_arguments(vec![
                "--coverage".into(),
                "a".into(),
                "--coverage".into(),
                "b".into(),
            ])
            .is_err()
        );
    }
}
