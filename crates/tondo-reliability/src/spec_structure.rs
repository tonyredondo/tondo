//! Structural validation for the normative Markdown specifications.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const SPECIFICATIONS: [&str; 5] = [
    "TONDO_LANGUAGE_SPEC.md",
    "TONDO_STANDARD_LIBRARY_SPEC.md",
    "TONDO_TESTING_SPEC.md",
    "TONDO_TOOLCHAIN_SPEC.md",
    "TONDO_LLM_FORM_SPEC.md",
];

/// Validates the stable heading identities used by conformance and links.
pub fn validate_repository(root: &Path) -> Result<(), String> {
    for relative in SPECIFICATIONS {
        let path = root.join(relative);
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
        let document = std::str::from_utf8(&bytes)
            .map_err(|error| format!("`{relative}` is not valid UTF-8: {error}"))?;
        validate_document(relative, document)?;
    }
    let testing = fs::read_to_string(root.join("TONDO_TESTING_SPEC.md"))
        .map_err(|error| format!("cannot read `TONDO_TESTING_SPEC.md`: {error}"))?;
    validate_testing_status(&testing)?;
    Ok(())
}

fn validate_testing_status(testing: &str) -> Result<(), String> {
    if testing
        .lines()
        .find(|line| line.starts_with("- **Estado:**"))
        .is_some_and(|line| line.contains("todavía no implementado"))
    {
        return Err(
            "TONDO_TESTING_SPEC.md: implementation status contradicts the functional runner".into(),
        );
    }
    Ok(())
}

fn validate_document(path: &str, document: &str) -> Result<(), String> {
    let mut numbered = BTreeMap::<String, usize>::new();
    let mut headings = BTreeMap::<String, usize>::new();
    let mut heading_path = Vec::<(usize, String)>::new();
    let mut numbered_path = Vec::<(usize, Vec<u64>)>::new();
    let mut fence: Option<(char, usize)> = None;
    let mut root_heading = None;

    for (index, line) in document.lines().enumerate() {
        let line_number = index + 1;
        if let Some((marker, width)) = fence {
            if closes_fence(line, marker, width) {
                fence = None;
            }
            continue;
        }
        if let Some(opened) = opens_fence(line) {
            fence = Some(opened);
            continue;
        }
        let Some((level, heading)) = markdown_heading(line) else {
            continue;
        };
        let normalized = normalize_heading(heading);
        if normalized.is_empty() {
            return Err(format!("{path}:{line_number}: heading has no stable text"));
        }
        if level == 1 {
            if let Some(previous) = root_heading.replace(line_number) {
                return Err(format!(
                    "{path}:{line_number}: document has a second level-one title; first declared at line {previous}"
                ));
            }
        } else if root_heading.is_none() {
            return Err(format!(
                "{path}:{line_number}: the first heading must be the level-one document title"
            ));
        }
        while heading_path
            .last()
            .is_some_and(|(ancestor_level, _)| *ancestor_level >= level)
        {
            heading_path.pop();
        }
        let identity = heading_path
            .iter()
            .map(|(_, ancestor)| ancestor.as_str())
            .chain(std::iter::once(normalized.as_str()))
            .collect::<Vec<_>>()
            .join(" / ");
        insert_unique(path, "heading path", &identity, line_number, &mut headings)?;
        heading_path.push((level, normalized));
        while numbered_path
            .last()
            .is_some_and(|(ancestor_level, _)| *ancestor_level >= level)
        {
            numbered_path.pop();
        }
        if let Some(number) = numbered_prefix(heading) {
            let segments = parse_number(number);
            if segments.len() != level.saturating_sub(1) {
                return Err(format!(
                    "{path}:{line_number}: section number `{number}` has depth {}, expected {} for heading level {level}",
                    segments.len(),
                    level.saturating_sub(1)
                ));
            }
            if segments.len() > 1 && numbered_path.is_empty() {
                return Err(format!(
                    "{path}:{line_number}: section number `{number}` has no numbered parent"
                ));
            }
            if let Some((_, parent)) = numbered_path.last()
                && !segments.starts_with(parent)
            {
                return Err(format!(
                    "{path}:{line_number}: section number `{number}` is not a child of `{}`",
                    format_number(parent)
                ));
            }
            insert_unique(path, "section number", number, line_number, &mut numbered)?;
            numbered_path.push((level, segments));
        }
    }

    if fence.is_some() {
        return Err(format!("{path}: unterminated Markdown fence"));
    }
    if root_heading.is_none() {
        return Err(format!("{path}: missing level-one document title"));
    }
    Ok(())
}

fn insert_unique(
    path: &str,
    kind: &str,
    identity: &str,
    line: usize,
    seen: &mut BTreeMap<String, usize>,
) -> Result<(), String> {
    if let Some(previous) = seen.insert(identity.to_owned(), line) {
        return Err(format!(
            "{path}:{line}: duplicate {kind} `{identity}`; first declared at line {previous}"
        ));
    }
    Ok(())
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = strip_markdown_indent(line)?;
    let marker = trimmed.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let width = trimmed.chars().take_while(|value| *value == marker).count();
    (width >= 3).then_some((marker, width))
}

fn closes_fence(line: &str, marker: char, width: usize) -> bool {
    let Some(trimmed) = strip_markdown_indent(line) else {
        return false;
    };
    let trimmed = trimmed.trim_end();
    let marker_width = trimmed.chars().take_while(|value| *value == marker).count();
    marker_width >= width && trimmed[marker_width..].is_empty()
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let line = strip_markdown_indent(line)?;
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes)
        || line
            .as_bytes()
            .get(hashes)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
    {
        return None;
    }
    let heading = line[hashes..].trim();
    Some((hashes, strip_closing_atx(heading)))
}

fn strip_markdown_indent(line: &str) -> Option<&str> {
    let spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    (spaces <= 3 && line.as_bytes().get(spaces) != Some(&b'\t')).then_some(&line[spaces..])
}

fn strip_closing_atx(heading: &str) -> &str {
    let trimmed = heading.trim_end();
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() != trimmed.len()
        && without_hashes
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        without_hashes.trim_end()
    } else {
        trimmed
    }
}

fn numbered_prefix(heading: &str) -> Option<&str> {
    let prefix = heading.split_whitespace().next()?;
    let candidate = prefix.strip_suffix('.').unwrap_or(prefix);
    let mut segments = candidate.split('.');
    let first = segments.next()?;
    if first.is_empty() || !first.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if segments
        .all(|segment| !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit()))
    {
        Some(candidate)
    } else {
        None
    }
}

fn parse_number(number: &str) -> Vec<u64> {
    number
        .split('.')
        .map(|segment| segment.parse().expect("numbered_prefix checked digits"))
        .collect()
}

fn format_number(segments: &[u64]) -> String {
    segments
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn normalize_heading(heading: &str) -> String {
    heading
        .chars()
        .filter_map(|value| match value {
            '`' | '*' | '_' => None,
            value if value.is_whitespace() => Some(' '),
            value => Some(value),
        })
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_unique_sections_and_ignores_fenced_headings() {
        validate_document(
            "SPEC.md",
            "# Title\n\n## 1. First\n\n~~~text\n## 1. First\n~~~\n\n### 1.1 Child\n",
        )
        .unwrap();
    }

    #[test]
    fn rejects_duplicate_section_numbers_with_different_titles() {
        let error = validate_document(
            "SPEC.md",
            "# Title\n\n## 1. First\n\n## 1. Different title\n",
        )
        .unwrap_err();
        assert_eq!(
            error,
            "SPEC.md:5: duplicate section number `1`; first declared at line 3"
        );
    }

    #[test]
    fn rejects_duplicate_normalized_headings() {
        let error =
            validate_document("SPEC.md", "# Title\n\n## Appendix\n\n## `Appendix`\n").unwrap_err();
        assert_eq!(
            error,
            "SPEC.md:5: duplicate heading path `title / appendix`; first declared at line 3"
        );
    }

    #[test]
    fn accepts_the_same_heading_below_different_parents() {
        validate_document(
            "SPEC.md",
            "# Title\n\n## 1. First\n\n### Testing\n\n## 2. Second\n\n### Testing\n",
        )
        .unwrap();
    }

    #[test]
    fn rejects_unterminated_fences() {
        assert_eq!(
            validate_document("SPEC.md", "# Title\n\n~~~text\n").unwrap_err(),
            "SPEC.md: unterminated Markdown fence"
        );
    }

    #[test]
    fn validates_commonmark_atx_headings_and_numbered_parents() {
        validate_document(
            "SPEC.md",
            "# Title\n\n  ## 1. Parent ##\n\n   ### 1.1 Child ###\n",
        )
        .unwrap();

        assert!(
            validate_document("SPEC.md", "# Title\n\n## 1. Parent\n\n### 2.1 Child\n")
                .unwrap_err()
                .contains("is not a child")
        );
        assert!(
            validate_document("SPEC.md", "# Title\n\n### 1. Wrong depth\n")
                .unwrap_err()
                .contains("has depth")
        );
        assert!(
            validate_document(
                "SPEC.md",
                "# Title\n\n## 1. Numbered\n\n## Appendix\n\n### 1.1 Not below section one\n",
            )
            .unwrap_err()
            .contains("parent")
        );
    }

    #[test]
    fn rejects_duplicate_titles_at_level_one_and_after_closing_atx_markers() {
        assert!(validate_document("SPEC.md", "# Title\n\n# `TITLE`\n").is_err());
        assert!(validate_document("SPEC.md", "# Title\n\n# Other\n").is_err());
        assert!(validate_document("SPEC.md", "## 1. Section\n").is_err());
        assert!(validate_document("SPEC.md", "plain text\n").is_err());
        assert!(
            validate_document("SPEC.md", "# Title\n\n## Appendix\n\n## Appendix ##\n").is_err()
        );
    }

    #[test]
    fn commonmark_indentation_applies_to_fences_and_headings() {
        validate_document(
            "SPEC.md",
            "# Title\n\n   ~~~text\n## ignored\n   ~~~\n\n   ## Appendix\n",
        )
        .unwrap();
        assert!(
            validate_document("SPEC.md", "# Title\n\n    ~~~text\n").is_ok(),
            "four-space indented fences are code blocks, not open fences"
        );
    }

    #[test]
    fn testing_status_must_describe_the_functional_runner() {
        validate_testing_status("- **Estado:** borrador con implementación funcional.\n").unwrap();
        assert!(validate_testing_status("- **Estado:** todavía no implementado.\n").is_err());
    }
}
