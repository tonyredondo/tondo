//! Pure CODEOWNERS parsing and matching for test metadata.
//!
//! The host supplies candidate paths and, when present, their bytes. This
//! module never opens a file, consults a provider or performs a permission
//! check. It validates the closed portable subset and resolves the last
//! matching rule for a logical source path.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::str;

use crate::artifact::sha256;
use crate::test_plan::{CodeownersMode, TestProjectPlan};

pub const AUTO_CODEOWNERS_PATHS: [&str; 3] =
    [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeownersCandidate {
    path: String,
    present: bool,
    bytes: Vec<u8>,
    regular_file: bool,
    readable: bool,
    symlink_escape: bool,
}

impl CodeownersCandidate {
    pub fn absent(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            present: false,
            bytes: Vec::new(),
            regular_file: true,
            readable: true,
            symlink_escape: false,
        }
    }

    pub fn present(path: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            present: true,
            bytes: bytes.into(),
            regular_file: true,
            readable: true,
            symlink_escape: false,
        }
    }

    pub fn with_file_state(
        mut self,
        regular_file: bool,
        readable: bool,
        symlink_escape: bool,
    ) -> Self {
        self.regular_file = regular_file;
        self.readable = readable;
        self.symlink_escape = symlink_escape;
        self
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn is_present(&self) -> bool {
        self.present
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipRule {
    pattern: String,
    owners: Vec<String>,
}

impl OwnershipRule {
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn owners(&self) -> &[String] {
        &self.owners
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipResolution {
    mode: &'static str,
    source: Option<String>,
    sha256: Option<String>,
    rules: Vec<OwnershipRule>,
}

impl OwnershipResolution {
    pub fn mode(&self) -> &'static str {
        self.mode
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    pub fn rules(&self) -> &[OwnershipRule] {
        &self.rules
    }

    /// Resolve the owners of one logical source path. A generated source with
    /// no repository origin should be represented by `None` at the call site
    /// and receives no owners.
    pub fn owners_for(&self, logical_path: Option<&str>) -> Result<Vec<String>, OwnershipError> {
        let Some(logical_path) = logical_path else {
            return Ok(Vec::new());
        };
        let logical_path = canonical_path("source.logical_path", logical_path)?;
        let mut owners = Vec::new();
        for rule in &self.rules {
            if matches_pattern(rule.pattern.as_str(), &logical_path) {
                owners.clone_from(&rule.owners);
            }
        }
        Ok(owners)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnershipError {
    InvalidField {
        field: &'static str,
        message: String,
    },
    Missing {
        path: String,
    },
    InvalidFile {
        path: String,
        message: String,
    },
    InvalidUtf8 {
        path: String,
    },
    InvalidRule {
        path: String,
        line: usize,
        message: String,
    },
    Duplicate {
        kind: &'static str,
        value: String,
    },
}

impl fmt::Display for OwnershipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField { field, message } => {
                write!(formatter, "invalid {field}: {message}")
            }
            Self::Missing { path } => write!(formatter, "CODEOWNERS file `{path}` is missing"),
            Self::InvalidFile { path, message } => {
                write!(formatter, "invalid CODEOWNERS file `{path}`: {message}")
            }
            Self::InvalidUtf8 { path } => {
                write!(formatter, "CODEOWNERS file `{path}` is not valid UTF-8")
            }
            Self::InvalidRule {
                path,
                line,
                message,
            } => {
                write!(
                    formatter,
                    "invalid CODEOWNERS rule `{path}:{line}`: {message}"
                )
            }
            Self::Duplicate { kind, value } => write!(formatter, "duplicate {kind} `{value}`"),
        }
    }
}

impl Error for OwnershipError {}

pub fn resolve_for_plan(
    plan: &TestProjectPlan,
    candidates: Vec<CodeownersCandidate>,
) -> Result<OwnershipResolution, OwnershipError> {
    resolve(plan.codeowners(), candidates)
}

pub fn resolve(
    mode: &CodeownersMode,
    candidates: Vec<CodeownersCandidate>,
) -> Result<OwnershipResolution, OwnershipError> {
    if matches!(mode, CodeownersMode::None) {
        return Ok(OwnershipResolution {
            mode: "none",
            source: None,
            sha256: None,
            rules: Vec::new(),
        });
    }
    let candidates = normalize_candidates(candidates, mode)?;
    match mode {
        CodeownersMode::None => unreachable!("none mode returned before candidate normalization"),
        CodeownersMode::Auto => {
            for path in AUTO_CODEOWNERS_PATHS {
                let Some(candidate) = candidates.get(path) else {
                    continue;
                };
                if !candidate.present {
                    continue;
                }
                return parse_present("auto", candidate);
            }
            Ok(OwnershipResolution {
                mode: "auto",
                source: None,
                sha256: None,
                rules: Vec::new(),
            })
        }
        CodeownersMode::Path(path) => {
            let path = canonical_path("codeowners.path", path)?;
            let Some(candidate) = candidates.get(&path) else {
                return Err(OwnershipError::Missing { path });
            };
            if !candidate.present {
                return Err(OwnershipError::Missing { path });
            }
            parse_present("explicit", candidate)
        }
    }
}

fn normalize_candidates(
    candidates: Vec<CodeownersCandidate>,
    mode: &CodeownersMode,
) -> Result<BTreeMap<String, CodeownersCandidate>, OwnershipError> {
    let mut normalized = BTreeMap::new();
    let allowed_auto = AUTO_CODEOWNERS_PATHS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    for mut candidate in candidates {
        candidate.path = canonical_path("codeowners.candidate.path", &candidate.path)?;
        if matches!(mode, CodeownersMode::Auto) && !allowed_auto.contains(candidate.path.as_str()) {
            return Err(OwnershipError::InvalidField {
                field: "codeowners.candidate.path",
                message: format!("auto mode does not admit `{}`", candidate.path),
            });
        }
        let path = candidate.path.clone();
        if normalized.contains_key(&path) {
            return Err(OwnershipError::Duplicate {
                kind: "CODEOWNERS candidate",
                value: path,
            });
        }
        normalized.insert(path, candidate);
    }
    Ok(normalized)
}

fn parse_present(
    mode: &'static str,
    candidate: &CodeownersCandidate,
) -> Result<OwnershipResolution, OwnershipError> {
    if !candidate.regular_file {
        return Err(OwnershipError::InvalidFile {
            path: candidate.path.clone(),
            message: "entry is not a regular file".into(),
        });
    }
    if candidate.symlink_escape {
        return Err(OwnershipError::InvalidFile {
            path: candidate.path.clone(),
            message: "entry escapes its declared root through a symlink".into(),
        });
    }
    if !candidate.readable {
        return Err(OwnershipError::InvalidFile {
            path: candidate.path.clone(),
            message: "entry is not readable".into(),
        });
    }
    if candidate.bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(OwnershipError::InvalidFile {
            path: candidate.path.clone(),
            message: "UTF-8 BOM is not permitted".into(),
        });
    }
    let text = str::from_utf8(&candidate.bytes).map_err(|_| OwnershipError::InvalidUtf8 {
        path: candidate.path.clone(),
    })?;
    let rules = parse_rules(&candidate.path, text)?;
    let hash = sha256(&candidate.bytes)
        .strip_prefix("sha256:")
        .expect("sha256 helper always returns a prefixed digest")
        .to_owned();
    Ok(OwnershipResolution {
        mode,
        source: Some(candidate.path.clone()),
        sha256: Some(hash),
        rules,
    })
}

fn parse_rules(path: &str, text: &str) -> Result<Vec<OwnershipRule>, OwnershipError> {
    let mut rules = Vec::new();
    for (index, raw_line) in text.split('\n').enumerate() {
        let line_number = index + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let first = line.trim_start_matches(|character| character == ' ' || character == '\t');
        if first.is_empty() || first.starts_with('#') {
            continue;
        }
        let fields = line
            .split(|character| character == ' ' || character == '\t')
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        if fields.len() < 2 {
            return Err(OwnershipError::InvalidRule {
                path: path.into(),
                line: line_number,
                message: "a rule requires one pattern and at least one owner".into(),
            });
        }
        let pattern = normalize_pattern(path, line_number, fields[0])?;
        let owners = fields[1..]
            .iter()
            .map(|owner| (*owner).to_owned())
            .collect::<Vec<_>>();
        rules.push(OwnershipRule { pattern, owners });
    }
    Ok(rules)
}

fn normalize_pattern(path: &str, line: usize, value: &str) -> Result<String, OwnershipError> {
    if value.is_empty() {
        return Err(OwnershipError::InvalidRule {
            path: path.into(),
            line,
            message: "pattern cannot be empty".into(),
        });
    }
    if value.contains(['!', '[', ']', '\\']) {
        return Err(OwnershipError::InvalidRule {
            path: path.into(),
            line,
            message: "negation, ranges and backslash are not supported".into(),
        });
    }
    let anchored = value.starts_with('/');
    let mut pattern = value.strip_prefix('/').unwrap_or(value).to_owned();
    if pattern.is_empty() {
        return Err(OwnershipError::InvalidRule {
            path: path.into(),
            line,
            message: "pattern cannot contain only an anchor".into(),
        });
    }
    if pattern.ends_with('/') {
        pattern.push_str("**");
    }
    for segment in pattern.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(OwnershipError::InvalidRule {
                path: path.into(),
                line,
                message: "pattern has an empty, `.` or `..` segment".into(),
            });
        }
    }
    if anchored {
        Ok(format!("/{pattern}"))
    } else {
        Ok(pattern)
    }
}

fn matches_pattern(pattern: &str, path: &str) -> bool {
    let anchored = pattern.starts_with('/');
    let body = pattern.strip_prefix('/').unwrap_or(pattern);
    if !body.contains('/') {
        let segments = path.split('/').collect::<Vec<_>>();
        let candidates = if anchored {
            &segments[..1]
        } else {
            segments.as_slice()
        };
        return candidates.iter().any(|segment| glob_match(body, segment));
    }
    glob_match(body, path)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let mut memo = vec![None; (pattern.len() + 1) * (text.len() + 1)];

    fn visit(
        pattern: &[char],
        text: &[char],
        pattern_index: usize,
        text_index: usize,
        memo: &mut [Option<bool>],
    ) -> bool {
        let slot = pattern_index * (text.len() + 1) + text_index;
        if let Some(value) = memo[slot] {
            return value;
        }
        let value = if pattern_index == pattern.len() {
            text_index == text.len()
        } else if pattern[pattern_index] == '*' {
            let double = pattern_index + 1 < pattern.len() && pattern[pattern_index + 1] == '*';
            let next = pattern_index + if double { 2 } else { 1 };
            visit(pattern, text, next, text_index, memo)
                || (text_index < text.len()
                    && (double || text[text_index] != '/')
                    && visit(pattern, text, pattern_index, text_index + 1, memo))
        } else if text_index < text.len()
            && ((pattern[pattern_index] == '?' && text[text_index] != '/')
                || (pattern[pattern_index] != '?' && pattern[pattern_index] == text[text_index]))
        {
            visit(pattern, text, pattern_index + 1, text_index + 1, memo)
        } else {
            false
        };
        memo[slot] = Some(value);
        value
    }

    visit(&pattern, &text, 0, 0, &mut memo)
}

fn canonical_path(field: &'static str, value: &str) -> Result<String, OwnershipError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains(['\\', '\n', '\r'])
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(OwnershipError::InvalidField {
            field,
            message: "path must be relative, slash-separated and canonical".into(),
        });
    }
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present(path: &str, text: &str) -> CodeownersCandidate {
        CodeownersCandidate::present(path, text.as_bytes().to_vec())
    }

    #[test]
    fn auto_uses_precedence_and_last_matching_rule() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![
                present("CODEOWNERS", "* @fallback\n"),
                present(".github/CODEOWNERS", "* @root\nsrc/** @src @src\n"),
                present("docs/CODEOWNERS", "* @docs\n"),
            ],
        )
        .unwrap();
        assert_eq!(resolution.mode(), "auto");
        assert_eq!(resolution.source(), Some(".github/CODEOWNERS"));
        assert_eq!(
            resolution.owners_for(Some("src/math.to")).unwrap(),
            ["@src", "@src"]
        );
        assert_eq!(
            resolution.owners_for(Some("docs/readme.md")).unwrap(),
            ["@root"]
        );
    }

    #[test]
    fn explicit_and_none_modes_close_source_and_digest() {
        let bytes = b"/src/* @team\n".to_vec();
        let resolution = resolve(
            &CodeownersMode::Path("owners/CODEOWNERS".into()),
            vec![CodeownersCandidate::present(
                "owners/CODEOWNERS",
                bytes.clone(),
            )],
        )
        .unwrap();
        assert_eq!(resolution.mode(), "explicit");
        assert_eq!(resolution.source(), Some("owners/CODEOWNERS"));
        assert_eq!(
            resolution.sha256(),
            Some(sha256(&bytes).strip_prefix("sha256:").unwrap())
        );
        assert_eq!(
            resolution.owners_for(Some("src/main.to")).unwrap(),
            ["@team"]
        );

        let disabled = resolve(
            &CodeownersMode::None,
            vec![CodeownersCandidate::present(
                "../invalid",
                b"not parsed".to_vec(),
            )],
        )
        .unwrap();
        assert_eq!(disabled.source(), None);
        assert!(disabled.owners_for(Some("src/main.to")).unwrap().is_empty());
    }

    #[test]
    fn parses_comments_crlf_and_opaque_owner_tokens() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![present(
                ".github/CODEOWNERS",
                "  # ignored\r\n\t\r\nfoo#bar @user/email @owner\r\n",
            )],
        )
        .unwrap();
        assert_eq!(resolution.rules().len(), 1);
        assert_eq!(
            resolution.owners_for(Some("foo#bar")).unwrap(),
            ["@user/email", "@owner"]
        );
    }

    #[test]
    fn implements_segment_and_full_path_globs() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![present(
                ".github/CODEOWNERS",
                "foo @segment\n/src/* @anchored\nsrc/** @tree\ndocs/ @docs\n",
            )],
        )
        .unwrap();
        assert_eq!(
            resolution.owners_for(Some("a/foo/b")).unwrap(),
            ["@segment"]
        );
        assert_eq!(
            resolution.owners_for(Some("foo/bar")).unwrap(),
            ["@segment"]
        );
        assert_eq!(
            resolution.owners_for(Some("src/main.to")).unwrap(),
            ["@tree"]
        );
        assert_eq!(
            resolution.owners_for(Some("src/nested/main.to")).unwrap(),
            ["@tree"]
        );
        assert_eq!(
            resolution.owners_for(Some("docs/readme.md")).unwrap(),
            ["@docs"]
        );
        assert!(
            resolution
                .owners_for(Some("lib/main.to"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn glob_wildcards_are_case_sensitive_and_respect_separators() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![present(
                ".github/CODEOWNERS",
                "src/** @tree\nsrc/*/test?.to @team\n",
            )],
        )
        .unwrap();
        assert_eq!(
            resolution.owners_for(Some("src/unit/test1.to")).unwrap(),
            ["@team"]
        );
        assert_eq!(
            resolution
                .owners_for(Some("src/unit/nested/test1.to"))
                .unwrap(),
            ["@tree"]
        );
        assert!(
            resolution
                .owners_for(Some("SRC/unit/test1.to"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn absent_auto_is_empty_but_explicit_missing_fails() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![
                CodeownersCandidate::absent(".github/CODEOWNERS"),
                CodeownersCandidate::absent("CODEOWNERS"),
                CodeownersCandidate::absent("docs/CODEOWNERS"),
            ],
        )
        .unwrap();
        assert_eq!(resolution.source(), None);
        let error = resolve(
            &CodeownersMode::Path("owners/CODEOWNERS".into()),
            vec![CodeownersCandidate::absent("owners/CODEOWNERS")],
        )
        .unwrap_err();
        assert!(matches!(error, OwnershipError::Missing { .. }));
    }

    #[test]
    fn rejects_invalid_files_rules_and_candidate_states() {
        for candidate in [
            present(".github/CODEOWNERS", "\u{feff}* @team\n"),
            present(".github/CODEOWNERS", "[ab] @team\n"),
            present(".github/CODEOWNERS", "*\n"),
            present(".github/CODEOWNERS", "foo/../bar @team\n"),
        ] {
            assert!(resolve(&CodeownersMode::Auto, vec![candidate]).is_err());
        }
        let error = resolve(
            &CodeownersMode::Auto,
            vec![present(".github/CODEOWNERS", "* @team\n").with_file_state(false, true, false)],
        )
        .unwrap_err();
        assert!(matches!(error, OwnershipError::InvalidFile { .. }));
        let error = resolve(
            &CodeownersMode::Auto,
            vec![present(".github/CODEOWNERS", "* @team\n").with_file_state(true, false, false)],
        )
        .unwrap_err();
        assert!(matches!(error, OwnershipError::InvalidFile { .. }));
    }

    #[test]
    fn rejects_duplicate_or_unknown_candidates_and_invalid_paths() {
        let duplicate = vec![
            present(".github/CODEOWNERS", "* @team\n"),
            present(".github/CODEOWNERS", "* @team\n"),
        ];
        assert!(matches!(
            resolve(&CodeownersMode::Auto, duplicate),
            Err(OwnershipError::Duplicate { .. })
        ));
        assert!(matches!(
            resolve(
                &CodeownersMode::Auto,
                vec![present("owners/CODEOWNERS", "* @team\n")]
            ),
            Err(OwnershipError::InvalidField { .. })
        ));
        assert!(matches!(
            resolve(&CodeownersMode::Path("../CODEOWNERS".into()), Vec::new()),
            Err(OwnershipError::InvalidField { .. })
        ));
    }

    #[test]
    fn generated_sources_without_an_origin_have_no_owners() {
        let resolution = resolve(
            &CodeownersMode::Auto,
            vec![present("CODEOWNERS", "* @team\n")],
        )
        .unwrap();
        assert!(resolution.owners_for(None).unwrap().is_empty());
        assert!(resolution.owners_for(Some("src/../main.to")).is_err());
    }
}
