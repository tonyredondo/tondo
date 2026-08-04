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
    Ok(())
}

fn validate_document(path: &str, document: &str) -> Result<(), String> {
    let mut numbered = BTreeMap::<String, usize>::new();
    let mut headings = BTreeMap::<String, usize>::new();
    let mut heading_path = Vec::<(usize, String)>::new();
    let mut fence: Option<(char, usize)> = None;

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
        if let Some(number) = numbered_prefix(heading) {
            insert_unique(path, "section number", number, line_number, &mut numbered)?;
        }
    }

    if fence.is_some() {
        return Err(format!("{path}: unterminated Markdown fence"));
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
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let width = trimmed.chars().take_while(|value| *value == marker).count();
    (width >= 3).then_some((marker, width))
}

fn closes_fence(line: &str, marker: char, width: usize) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|value| value == marker)
        && trimmed.chars().count() >= width
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(2..=6).contains(&hashes) || line.as_bytes().get(hashes) != Some(&b' ') {
        return None;
    }
    Some((hashes, line[hashes + 1..].trim()))
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

fn normalize_heading(heading: &str) -> String {
    heading
        .chars()
        .filter_map(|value| match value {
            '`' | '*' | '_' => None,
            value if value.is_whitespace() => Some(' '),
            value => Some(value.to_ascii_lowercase()),
        })
        .collect::<String>()
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
            "SPEC.md:5: duplicate heading path `appendix`; first declared at line 3"
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
}
