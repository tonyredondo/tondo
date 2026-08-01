use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use tondo_compiler::driver::DiagnosticFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestFormat {
    Human,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestSelector {
    All,
    Filter(String),
    Glob(String),
    Exact(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeownersSelection {
    Auto,
    None,
    Explicit(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestShard {
    pub index: u32,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOrder {
    Canonical,
    Random { seed: Option<u64> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestReportFormat {
    Json,
    Junit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReportOutput {
    pub format: TestReportFormat,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCliPlan {
    pub project: Option<PathBuf>,
    pub test_plan: Option<PathBuf>,
    pub selector: TestSelector,
    pub selector_explicit: bool,
    pub codeowners: CodeownersSelection,
    pub codeowners_explicit: bool,
    pub shard: Option<TestShard>,
    pub shard_explicit: bool,
    pub order: TestOrder,
    pub order_explicit: bool,
    pub list: bool,
    pub jobs: u32,
    pub jobs_explicit: bool,
    pub timeout_ms: Option<u64>,
    /// Keeps the distinction between the default and an explicit `none`.
    /// The parser accepts the spelling for compatibility, while the closed
    /// project plan decides whether execution may disable its wall-clock cap.
    pub timeout_explicit: bool,
    pub retry: u32,
    pub retry_explicit: bool,
    pub repeat: u32,
    pub repeat_explicit: bool,
    pub artifacts: Option<PathBuf>,
    pub update_snapshots: bool,
    pub diagnostic_format: DiagnosticFormat,
    pub test_format: TestFormat,
    pub reports: Vec<TestReportOutput>,
    pub show_output: bool,
    pub deny_skips: bool,
    pub allow_flaky: bool,
    pub allow_empty: bool,
}

pub fn parse(arguments: &[OsString]) -> Result<TestCliPlan, String> {
    if arguments.first().and_then(|value| value.to_str()) != Some("test") {
        return Err("the `test` command is required".into());
    }
    let mut plan = TestCliPlan {
        project: None,
        test_plan: None,
        selector: TestSelector::All,
        selector_explicit: false,
        codeowners: CodeownersSelection::Auto,
        codeowners_explicit: false,
        shard: None,
        shard_explicit: false,
        order: TestOrder::Canonical,
        order_explicit: false,
        list: false,
        jobs: 1,
        jobs_explicit: false,
        timeout_ms: None,
        timeout_explicit: false,
        retry: 0,
        retry_explicit: false,
        repeat: 1,
        repeat_explicit: false,
        artifacts: None,
        update_snapshots: false,
        diagnostic_format: DiagnosticFormat::Human,
        test_format: TestFormat::Human,
        reports: Vec::new(),
        show_output: false,
        deny_skips: false,
        allow_flaky: false,
        allow_empty: false,
    };
    let mut seen = BTreeSet::new();
    let mut seed = None;
    let mut index = 1;
    while index < arguments.len() {
        let argument = arguments[index]
            .to_str()
            .ok_or_else(|| "test options must be valid UTF-8".to_owned())?;
        if argument == "--" {
            return Err("`tondo test` does not accept program arguments".into());
        }
        if argument == "--list" {
            flag_once(&mut seen, "--list")?;
            plan.list = true;
        } else if argument == "--show-output" {
            flag_once(&mut seen, "--show-output")?;
            plan.show_output = true;
        } else if argument == "--deny-skips" {
            flag_once(&mut seen, "--deny-skips")?;
            plan.deny_skips = true;
        } else if argument == "--allow-flaky" {
            flag_once(&mut seen, "--allow-flaky")?;
            plan.allow_flaky = true;
        } else if argument == "--allow-empty" {
            flag_once(&mut seen, "--allow-empty")?;
            plan.allow_empty = true;
        } else if argument == "--update-snapshots" {
            flag_once(&mut seen, "--update-snapshots")?;
            plan.update_snapshots = true;
        } else if let Some((name, inline)) = argument.split_once('=') {
            if !matches!(
                name,
                "--filter"
                    | "--glob"
                    | "--exact"
                    | "--project"
                    | "--test-plan"
                    | "--codeowners"
                    | "--shard"
                    | "--order"
                    | "--seed"
                    | "--jobs"
                    | "--timeout"
                    | "--retry"
                    | "--repeat"
                    | "--artifacts"
                    | "--diagnostic-format"
                    | "--test-format"
                    | "--report"
            ) {
                return Err(format!("unknown option `{argument}`"));
            }
            parse_value(&mut plan, &mut seen, &mut seed, name, inline)?;
        } else if argument.starts_with('-') {
            if !matches!(
                argument,
                "--filter"
                    | "--glob"
                    | "--exact"
                    | "--project"
                    | "--test-plan"
                    | "--codeowners"
                    | "--shard"
                    | "--order"
                    | "--seed"
                    | "--jobs"
                    | "--timeout"
                    | "--retry"
                    | "--repeat"
                    | "--artifacts"
                    | "--diagnostic-format"
                    | "--test-format"
                    | "--report"
            ) {
                return Err(format!("unknown option `{argument}`"));
            }
            index += 1;
            let value = arguments
                .get(index)
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("`{argument}` requires a value"))?;
            parse_value(&mut plan, &mut seen, &mut seed, argument, value)?;
        } else {
            return Err(format!(
                "unexpected positional argument `{argument}`; use `--project` or `--test-plan`"
            ));
        }
        index += 1;
    }

    if let Some(seed) = seed {
        plan.order = match plan.order {
            TestOrder::Random { .. } => TestOrder::Random { seed: Some(seed) },
            TestOrder::Canonical => {
                return Err("`--seed` requires `--order random`".into());
            }
        };
    }
    validate_combinations(&plan)
}

fn parse_value(
    plan: &mut TestCliPlan,
    seen: &mut BTreeSet<&'static str>,
    seed: &mut Option<u64>,
    name: &str,
    value: &str,
) -> Result<(), String> {
    match name {
        "--project" => {
            once_value(seen, "--project")?;
            plan.project = Some(validate_text_path(value, "`--project`")?);
            Ok(())
        }
        "--test-plan" => {
            once_value(seen, "--test-plan")?;
            let path = validate_text_path(value, "`--test-plan`")?;
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
            {
                return Err("`--test-plan` accepts TOML only; JSON plans are unsupported".into());
            }
            plan.test_plan = Some(path);
            Ok(())
        }
        "--filter" => {
            once_value(seen, "--filter")?;
            validate_text(value, "`--filter`")?;
            set_selector(plan, TestSelector::Filter(value.into()))
        }
        "--glob" => {
            once_value(seen, "--glob")?;
            validate_glob(value)?;
            set_selector(plan, TestSelector::Glob(value.into()))
        }
        "--exact" => {
            once_value(seen, "--exact")?;
            validate_text(value, "`--exact`")?;
            set_selector(plan, TestSelector::Exact(value.into()))
        }
        "--codeowners" => {
            once_value(seen, "--codeowners")?;
            plan.codeowners_explicit = true;
            plan.codeowners = match value {
                "auto" => CodeownersSelection::Auto,
                "none" => CodeownersSelection::None,
                path => CodeownersSelection::Explicit(validate_relative_path(path, "CODEOWNERS")?),
            };
            Ok(())
        }
        "--shard" => {
            once_value(seen, "--shard")?;
            plan.shard_explicit = true;
            plan.shard = Some(parse_shard(value)?);
            Ok(())
        }
        "--order" => {
            once_value(seen, "--order")?;
            plan.order_explicit = true;
            plan.order = match value {
                "canonical" => TestOrder::Canonical,
                "random" => TestOrder::Random { seed: None },
                _ => return Err("`--order` expects `canonical` or `random`".into()),
            };
            Ok(())
        }
        "--seed" => {
            once_value(seen, "--seed")?;
            *seed = Some(parse_hex_seed(value)?);
            Ok(())
        }
        "--jobs" => {
            once_value(seen, "--jobs")?;
            plan.jobs_explicit = true;
            plan.jobs = parse_positive(value, "`--jobs`")?;
            Ok(())
        }
        "--timeout" => {
            once_value(seen, "--timeout")?;
            plan.timeout_explicit = true;
            plan.timeout_ms = if value == "none" {
                None
            } else {
                Some(parse_duration_ms(value)?)
            };
            Ok(())
        }
        "--retry" => {
            once_value(seen, "--retry")?;
            plan.retry = parse_non_negative(value, "`--retry`")?;
            plan.retry_explicit = true;
            Ok(())
        }
        "--repeat" => {
            once_value(seen, "--repeat")?;
            plan.repeat = parse_positive(value, "`--repeat`")?;
            plan.repeat_explicit = true;
            Ok(())
        }
        "--artifacts" => {
            once_value(seen, "--artifacts")?;
            plan.artifacts = Some(validate_relative_path(value, "artifacts")?);
            Ok(())
        }
        "--diagnostic-format" => {
            once_value(seen, "--diagnostic-format")?;
            plan.diagnostic_format = match value {
                "human" => DiagnosticFormat::Human,
                "json" => DiagnosticFormat::Json,
                _ => return Err("`--diagnostic-format` expects `human` or `json`".into()),
            };
            Ok(())
        }
        "--test-format" => {
            once_value(seen, "--test-format")?;
            plan.test_format = match value {
                "human" => TestFormat::Human,
                "json" => TestFormat::Json,
                _ => return Err("`--test-format` expects `human` or `json`".into()),
            };
            Ok(())
        }
        "--report" => {
            let (format, path) = value
                .split_once('=')
                .ok_or_else(|| "`--report` requires `json=<path>` or `junit=<path>`".to_owned())?;
            let format = match format {
                "json" => TestReportFormat::Json,
                "junit" => TestReportFormat::Junit,
                _ => return Err("`--report` expects `json=<path>` or `junit=<path>`".into()),
            };
            if path.is_empty() {
                return Err("`--report` requires a non-empty path".into());
            }
            let path = PathBuf::from(path);
            if plan
                .reports
                .iter()
                .any(|report| same_path(&report.path, &path))
            {
                return Err("report outputs must be distinct".into());
            }
            plan.reports.push(TestReportOutput { format, path });
            Ok(())
        }
        _ => Err(format!("unknown option `{name}`")),
    }
}

pub(crate) fn validate_combinations(plan: &TestCliPlan) -> Result<TestCliPlan, String> {
    if (plan.repeat_explicit && plan.retry_explicit) || (plan.repeat > 1 && plan.retry > 0) {
        return Err("`--retry` and `--repeat` are mutually exclusive".into());
    }
    if (plan.repeat_explicit || plan.repeat > 1) && plan.allow_flaky {
        return Err("`--repeat` and `--allow-flaky` are mutually exclusive".into());
    }
    if plan.list {
        if plan.show_output
            || plan.deny_skips
            || plan.allow_flaky
            || plan.retry > 0
            || plan.repeat > 1
            || plan.retry_explicit
            || plan.repeat_explicit
            || plan.update_snapshots
            || plan.artifacts.is_some()
        {
            return Err("`--list` cannot be combined with execution-only options".into());
        }
        if plan
            .reports
            .iter()
            .any(|report| report.format == TestReportFormat::Junit)
        {
            return Err("`--list` cannot emit a junit report".into());
        }
    }
    if plan.update_snapshots
        && (plan.jobs != 1
            || !matches!(plan.order, TestOrder::Canonical)
            || plan.shard.is_some()
            || plan.retry > 0
            || plan.repeat > 1
            || plan.retry_explicit
            || plan.repeat_explicit
            || plan.allow_flaky)
    {
        return Err("`--update-snapshots` requires canonical order, one job, and no shard/retry/repeat/flaky policy".into());
    }
    Ok(plan.clone())
}

fn set_selector(plan: &mut TestCliPlan, selector: TestSelector) -> Result<(), String> {
    if !matches!(plan.selector, TestSelector::All) {
        return Err("`--filter`, `--glob` and `--exact` are mutually exclusive".into());
    }
    plan.selector_explicit = true;
    plan.selector = selector;
    Ok(())
}

fn flag_once<'a>(seen: &mut BTreeSet<&'a str>, flag: &'a str) -> Result<(), String> {
    if !seen.insert(flag) {
        return Err(format!("`{flag}` may appear only once"));
    }
    Ok(())
}

fn once_value(seen: &mut BTreeSet<&'static str>, flag: &'static str) -> Result<(), String> {
    flag_once(seen, flag)
}

fn parse_positive(value: &str, flag: &str) -> Result<u32, String> {
    let number = parse_decimal(value, flag)?;
    if number == 0 || number > u32::MAX as u64 {
        return Err(format!("{flag} must be a positive 32-bit integer"));
    }
    Ok(number as u32)
}

fn parse_non_negative(value: &str, flag: &str) -> Result<u32, String> {
    let number = parse_decimal(value, flag)?;
    if number > u32::MAX as u64 {
        return Err(format!("{flag} exceeds the 32-bit limit"));
    }
    Ok(number as u32)
}

fn parse_decimal(value: &str, flag: &str) -> Result<u64, String> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("{flag} expects canonical decimal digits"));
    }
    value.parse().map_err(|_| format!("{flag} is out of range"))
}

fn parse_hex_seed(value: &str) -> Result<u64, String> {
    if value.is_empty() || value.len() > 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("`--seed` expects one to sixteen hexadecimal digits".into());
    }
    u64::from_str_radix(value, 16).map_err(|_| "`--seed` is out of range".into())
}

fn parse_shard(value: &str) -> Result<TestShard, String> {
    let (index, count) = value
        .split_once('/')
        .ok_or_else(|| "`--shard` expects `<index>/<count>`".to_owned())?;
    if count.contains('/') {
        return Err("`--shard` expects exactly one slash".into());
    }
    let index = parse_positive(index, "`--shard` index")?;
    let count = parse_positive(count, "`--shard` count")?;
    if index > count {
        return Err("`--shard` index must not exceed count".into());
    }
    Ok(TestShard { index, count })
}

fn parse_duration_ms(value: &str) -> Result<u64, String> {
    let (digits, unit) = if let Some(value) = value.strip_suffix("ms") {
        (value, 1_u64)
    } else if let Some(value) = value.strip_suffix('s') {
        (value, 1_000_u64)
    } else if let Some(value) = value.strip_suffix('m') {
        (value, 60_000_u64)
    } else if let Some(value) = value.strip_suffix('h') {
        (value, 3_600_000_u64)
    } else {
        return Err("`--timeout` expects `none` or a duration ending in ms, s, m or h".into());
    };
    let number = parse_positive(digits, "`--timeout`")? as u64;
    number
        .checked_mul(unit)
        .ok_or_else(|| "`--timeout` is out of range".into())
}

fn validate_glob(value: &str) -> Result<(), String> {
    validate_text(value, "`--glob`")?;
    if value.contains(['\\', '[', ']'])
        || value
            .split('/')
            .any(|segment| segment == "." || segment == ".." || segment.is_empty())
    {
        return Err("`--glob` contains an invalid path segment or unsupported escape".into());
    }
    Ok(())
}

fn validate_text(value: &str, option: &str) -> Result<(), String> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(format!("{option} requires a non-empty line-free value"));
    }
    Ok(())
}

fn validate_relative_path(value: &str, label: &str) -> Result<PathBuf, String> {
    validate_text(value, label)?;
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "{label} path must be relative and contain no `.` or `..`"
        ));
    }
    Ok(path.to_owned())
}

fn validate_text_path(value: &str, label: &str) -> Result<PathBuf, String> {
    validate_text(value, label)?;
    Ok(PathBuf::from(value))
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let left = std::path::absolute(left)
        .ok()
        .map(|path| normalize_path(&path));
    let right = std::path::absolute(right)
        .ok()
        .map(|path| normalize_path(&path));
    left == right
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_defaults_and_all_value_forms_without_execution() {
        let plan = parse(&args(&[
            "test",
            "--project",
            ".",
            "--test-plan",
            "tondo.test.toml",
            "--filter",
            "math",
            "--codeowners",
            "owners/CODEOWNERS",
            "--shard",
            "2/8",
            "--order",
            "random",
            "--seed",
            "5eed",
            "--jobs",
            "4",
            "--timeout",
            "2s",
            "--retry",
            "2",
            "--test-format=json",
            "--diagnostic-format=json",
            "--report",
            "json=target/tests.json",
            "--report=junit=target/tests.xml",
            "--show-output",
            "--allow-empty",
        ]))
        .unwrap();
        assert_eq!(plan.project, Some(PathBuf::from(".")));
        assert_eq!(plan.test_plan, Some(PathBuf::from("tondo.test.toml")));
        assert_eq!(plan.selector, TestSelector::Filter("math".into()));
        assert!(plan.selector_explicit);
        assert!(plan.codeowners_explicit);
        assert!(plan.shard_explicit);
        assert!(plan.order_explicit);
        assert!(plan.jobs_explicit);
        assert_eq!(plan.shard, Some(TestShard { index: 2, count: 8 }));
        assert_eq!(plan.order, TestOrder::Random { seed: Some(0x5eed) });
        assert_eq!(plan.timeout_ms, Some(2_000));
        assert!(plan.timeout_explicit);
        assert_eq!(plan.retry, 2);
        assert_eq!(plan.reports.len(), 2);
        assert_eq!(plan.diagnostic_format, DiagnosticFormat::Json);
        assert_eq!(plan.test_format, TestFormat::Json);
    }

    #[test]
    fn normalizes_random_seed_and_accepts_timeout_none() {
        let plan = parse(&args(&[
            "test",
            "--order",
            "random",
            "--seed=ABC",
            "--timeout",
            "none",
        ]))
        .unwrap();
        assert_eq!(plan.order, TestOrder::Random { seed: Some(0xabc) });
        assert_eq!(plan.timeout_ms, None);
        assert!(plan.timeout_explicit);
    }

    #[test]
    fn accepts_a_conventional_project_without_json_configuration() {
        let plan = parse(&args(&[
            "test",
            "--project",
            "example",
            "--test-plan",
            "tondo.test.toml",
        ]))
        .unwrap();
        assert_eq!(plan.project, Some(PathBuf::from("example")));
        assert_eq!(plan.test_plan, Some(PathBuf::from("tondo.test.toml")));
    }

    #[test]
    fn rejects_removed_json_project_options() {
        let error = parse(&args(&["test", "--manifest", "tondo.json"])).unwrap_err();
        assert!(error.contains("unknown option"));

        let error = parse(&args(&["test", "--test-plan", "tondo.test.json"])).unwrap_err();
        assert!(error.contains("TOML only"));
    }

    #[test]
    fn rejects_selector_numbers_paths_globs_and_report_collisions() {
        let invalid = [
            (
                &["test", "--filter", "a", "--glob", "a*"][..],
                "mutually exclusive",
            ),
            (&["test", "--shard", "0/2"][..], "positive"),
            (&["test", "--shard", "3/2"][..], "must not exceed"),
            (&["test", "--glob", "a/[b]"][..], "invalid path"),
            (&["test", "--codeowners", "../OWNERS"][..], "relative"),
            (
                &["test", "--report", "json=out", "--report", "junit=out"][..],
                "distinct",
            ),
            (&["test", "--jobs", "01"][..], "canonical decimal"),
        ];
        for (values, expected) in invalid {
            let error = parse(&args(values)).unwrap_err();
            assert!(error.contains(expected), "{values:?}: {error}");
        }
    }

    #[test]
    fn rejects_incompatible_execution_modes_before_compilation() {
        let invalid = [
            (&["test", "--list", "--retry", "1"][..], "--list"),
            (&["test", "--list", "--retry", "0"][..], "--list"),
            (
                &["test", "--repeat", "2", "--retry", "1"][..],
                "mutually exclusive",
            ),
            (
                &["test", "--repeat", "1", "--allow-flaky"][..],
                "mutually exclusive",
            ),
            (
                &["test", "--repeat", "1", "--update-snapshots"][..],
                "canonical order",
            ),
            (
                &["test", "--update-snapshots", "--order", "random"][..],
                "canonical order",
            ),
            (&["test", "--seed", "5eed"][..], "requires `--order random`"),
            (&["test", "--", "arg"][..], "does not accept"),
            (&["test", "source.to"][..], "positional argument"),
        ];
        for (values, expected) in invalid {
            let error = parse(&args(values)).unwrap_err();
            assert!(error.contains(expected), "{values:?}: {error}");
        }
    }

    #[test]
    fn effective_sidecar_policy_cannot_leak_into_list_or_snapshot_update() {
        let mut list = parse(&args(&["test", "--list"])).unwrap();
        list.retry = 1;
        assert!(validate_combinations(&list).unwrap_err().contains("--list"));

        let mut update = parse(&args(&["test", "--update-snapshots"])).unwrap();
        update.repeat = 2;
        assert!(
            validate_combinations(&update)
                .unwrap_err()
                .contains("canonical order")
        );
    }

    #[test]
    fn repeats_and_equals_forms_are_checked_at_the_option_boundary() {
        let invalid = [
            (&["test", "--jobs=0"][..], "positive"),
            (&["test", "--timeout", "0ms"][..], "positive"),
            (
                &["test", "--order", "canonical", "--seed=1"][..],
                "requires `--order random`",
            ),
            (&["test", "--report=xml=out"][..], "expects `json=<path>`"),
            (&["test", "--unknown"][..], "unknown option"),
            (&["test", "--jobs", "1", "--jobs", "2"][..], "only once"),
            (
                &[
                    "test",
                    "--report",
                    "json=out",
                    "--report",
                    "junit=out/../out",
                ][..],
                "distinct",
            ),
        ];
        for (values, expected) in invalid {
            let error = parse(&args(values)).unwrap_err();
            assert!(error.contains(expected), "{values:?}: {error}");
        }
    }
}
