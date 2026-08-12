use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use crate::protocol::DocCategory;
use crate::sha256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentFence {
    pub fence_byte: u64,
    pub category: DocCategory,
    pub fixture: Option<String>,
    pub expected_codes: Vec<String>,
    pub source: Vec<u8>,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentError {
    byte: usize,
    message: String,
}

impl DocumentError {
    fn new(byte: usize, message: impl Into<String>) -> Self {
        Self {
            byte,
            message: message.into(),
        }
    }

    pub fn byte(&self) -> usize {
        self.byte
    }
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid documentation fence at byte {}: {}",
            self.byte, self.message
        )
    }
}

impl Error for DocumentError {}

pub fn extract_fences(
    markdown: &[u8],
    registered_errors: &BTreeSet<String>,
) -> Result<Vec<DocumentFence>, DocumentError> {
    std::str::from_utf8(markdown)
        .map_err(|error| DocumentError::new(error.valid_up_to(), "Markdown is not valid UTF-8"))?;
    validate_line_endings(markdown)?;
    let lines = physical_lines(markdown);
    let mut fences = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        if !line.contents.starts_with(b"~~~tondo") {
            index += 1;
            continue;
        }
        let header = std::str::from_utf8(line.contents)
            .expect("the complete document was validated as UTF-8");
        let (category, fixture, expected_codes) =
            parse_header(header, line.start, registered_errors)?;
        let opening = line.start;
        index += 1;
        let mut source = Vec::new();
        let mut closed = false;
        while index < lines.len() {
            let content = &lines[index];
            if content.contents == b"~~~" {
                closed = true;
                index += 1;
                break;
            }
            source.extend_from_slice(content.contents);
            source.push(b'\n');
            index += 1;
        }
        if !closed {
            return Err(DocumentError::new(opening, "Tondo fence is not closed"));
        }
        if source.is_empty() {
            source.push(b'\n');
        }
        let source_sha256 = sha256(&source);
        fences.push(DocumentFence {
            fence_byte: u64::try_from(opening).unwrap_or(u64::MAX),
            category,
            fixture,
            expected_codes,
            source,
            source_sha256,
        });
    }
    Ok(fences)
}

fn parse_header(
    header: &str,
    byte: usize,
    registered_errors: &BTreeSet<String>,
) -> Result<(DocCategory, Option<String>, Vec<String>), DocumentError> {
    if header.ends_with(char::is_whitespace) {
        return Err(DocumentError::new(
            byte,
            "Tondo fence header has trailing whitespace",
        ));
    }
    let Some(rest) = header.strip_prefix("~~~tondo") else {
        unreachable!("the scanner selects only Tondo headers")
    };
    if !rest.is_empty() && !rest.starts_with(' ') {
        return Err(DocumentError::new(byte, "unknown Tondo fence header form"));
    }
    let words = rest
        .strip_prefix(' ')
        .unwrap_or_default()
        .split(' ')
        .collect::<Vec<_>>();
    if words.iter().any(|word| word.is_empty()) && !rest.is_empty() {
        return Err(DocumentError::new(
            byte,
            "Tondo fence header uses non-canonical whitespace",
        ));
    }
    match words.as_slice() {
        [""] if rest.is_empty() => Ok((DocCategory::Syntax, None, Vec::new())),
        ["fragment"] => Ok((DocCategory::Fragment, Some("spec.0_1".into()), Vec::new())),
        ["fragment", fixture] => {
            validate_fixture_name(fixture, byte)?;
            Ok((DocCategory::Fragment, Some((*fixture).into()), Vec::new()))
        }
        ["script"] => Ok((DocCategory::Script, Some("spec.0_1".into()), Vec::new())),
        ["script", fixture] => {
            validate_fixture_name(fixture, byte)?;
            Ok((DocCategory::Script, Some((*fixture).into()), Vec::new()))
        }
        ["pseudocode"] => Ok((DocCategory::Pseudocode, None, Vec::new())),
        ["compile-fail", codes @ ..] if !codes.is_empty() => {
            let mut expected = Vec::with_capacity(codes.len());
            for code in codes {
                validate_error_code(code, byte, registered_errors)?;
                if expected.iter().any(|existing| existing == code) {
                    return Err(DocumentError::new(
                        byte,
                        format!("compile-fail code `{code}` is repeated"),
                    ));
                }
                expected.push((*code).to_owned());
            }
            expected.sort();
            Ok((DocCategory::CompileFail, Some("spec.0_1".into()), expected))
        }
        _ => Err(DocumentError::new(byte, "unknown Tondo fence header form")),
    }
}

fn validate_fixture_name(value: &str, byte: usize) -> Result<(), DocumentError> {
    let mut bytes = value.bytes();
    if !bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        || !bytes.all(|item| {
            item.is_ascii_lowercase() || item.is_ascii_digit() || item == b'_' || item == b'.'
        })
    {
        return Err(DocumentError::new(
            byte,
            format!("invalid fixture name `{value}`"),
        ));
    }
    Ok(())
}

fn validate_error_code(
    value: &str,
    byte: usize,
    registered_errors: &BTreeSet<String>,
) -> Result<(), DocumentError> {
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[0] != b'E' || !bytes[1..].iter().all(u8::is_ascii_digit) {
        return Err(DocumentError::new(
            byte,
            format!("invalid compile-fail code `{value}`"),
        ));
    }
    if !registered_errors.contains(value) {
        return Err(DocumentError::new(
            byte,
            format!("unknown compile-fail code `{value}`"),
        ));
    }
    Ok(())
}

fn validate_line_endings(markdown: &[u8]) -> Result<(), DocumentError> {
    for (index, byte) in markdown.iter().copied().enumerate() {
        if byte == b'\r' && markdown.get(index + 1) != Some(&b'\n') {
            return Err(DocumentError::new(index, "isolated CR line ending"));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct PhysicalLine<'a> {
    start: usize,
    contents: &'a [u8],
}

fn physical_lines(markdown: &[u8]) -> Vec<PhysicalLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in markdown.iter().copied().enumerate() {
        if byte != b'\n' {
            continue;
        }
        let mut end = index;
        if end > start && markdown[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(PhysicalLine {
            start,
            contents: &markdown[start..end],
        });
        start = index + 1;
    }
    if start < markdown.len() {
        lines.push(PhysicalLine {
            start,
            contents: &markdown[start..],
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors() -> BTreeSet<String> {
        ["E0004", "E1102"].into_iter().map(str::to_owned).collect()
    }

    #[test]
    fn extraction_normalizes_container_line_endings_and_preserves_byte_offset() {
        let lf = b"before\n~~~tondo fragment spec.core\nlet value = 1\n~~~\n";
        let crlf = b"before\r\n~~~tondo fragment spec.core\r\nlet value = 1\r\n~~~\r\n";
        let left = extract_fences(lf, &errors()).unwrap();
        let right = extract_fences(crlf, &errors()).unwrap();
        assert_eq!(left[0].source, b"let value = 1\n");
        assert_eq!(right[0].source, left[0].source);
        assert_eq!(left[0].fence_byte, 7);
        assert_eq!(right[0].fence_byte, 8);
    }

    #[test]
    fn categories_and_defaults_are_closed() {
        let document = b"\
~~~tondo\nfn f()\n~~~\n\
~~~tondo fragment\nlet x = 1\n~~~\n\
~~~tondo script spec.process\n1\n~~~\n\
~~~tondo compile-fail E1102 E0004\nbad\n~~~\n\
~~~tondo pseudocode\nwords\n~~~\n";
        let fences = extract_fences(document, &errors()).unwrap();
        assert_eq!(fences.len(), 5);
        assert_eq!(fences[0].category, DocCategory::Syntax);
        assert_eq!(fences[1].fixture.as_deref(), Some("spec.0_1"));
        assert_eq!(fences[2].fixture.as_deref(), Some("spec.process"));
        assert_eq!(fences[3].expected_codes, ["E0004", "E1102"]);
        assert_eq!(fences[4].category, DocCategory::Pseudocode);
    }

    #[test]
    fn malformed_headers_and_documents_are_rejected() {
        let isolated_cr = extract_fences(b"plain\rtext\n", &errors()).unwrap_err();
        assert_eq!(isolated_cr.byte(), 5);
        assert!(isolated_cr.to_string().contains("isolated CR line ending"));

        for document in [
            &b"~~~tondo \n~~~\n"[..],
            &b"~~~tondo  fragment\n~~~\n"[..],
            &b"~~~tondo compile-fail E9999\n~~~\n"[..],
            &b"~~~tondo compile-fail E0004 E0004\n~~~\n"[..],
            &b"~~~tondo unknown\n~~~\n"[..],
            &b"~~~tondo fragment Bad\n~~~\n"[..],
            &b"~~~tondo fragment\n"[..],
            &b"~~~tondo\rbody\n~~~\n"[..],
        ] {
            assert!(extract_fences(document, &errors()).is_err());
        }
    }

    #[test]
    fn hostile_headers_report_exact_bytes_and_reasons() {
        let cases = [
            (
                &b"prefix\n~~~tondo \n~~~\n"[..],
                7,
                "invalid documentation fence at byte 7: Tondo fence header has trailing whitespace",
            ),
            (
                &b"~~~tondo\tfragment\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: unknown Tondo fence header form",
            ),
            (
                &b"~~~tondo  fragment\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: Tondo fence header uses non-canonical whitespace",
            ),
            (
                &b"~~~tondo fragment Bad\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: invalid fixture name `Bad`",
            ),
            (
                &b"~~~tondo compile-fail E04\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: invalid compile-fail code `E04`",
            ),
            (
                &b"~~~tondo compile-fail E9999\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: unknown compile-fail code `E9999`",
            ),
            (
                &b"~~~tondo compile-fail E0004 E0004\n~~~\n"[..],
                0,
                "invalid documentation fence at byte 0: compile-fail code `E0004` is repeated",
            ),
            (
                &b"~~~tondo\nlet value = 1\n~~~ \n"[..],
                0,
                "invalid documentation fence at byte 0: Tondo fence is not closed",
            ),
        ];

        for (document, byte, expected) in cases {
            let error = extract_fences(document, &errors()).unwrap_err();
            assert_eq!(error.byte(), byte);
            assert_eq!(error.to_string(), expected);
        }

        let invalid_utf8 = extract_fences(b"ok\n\xff\n", &errors()).unwrap_err();
        assert_eq!(invalid_utf8.byte(), 3);
        assert_eq!(
            invalid_utf8.to_string(),
            "invalid documentation fence at byte 3: Markdown is not valid UTF-8"
        );
    }

    #[test]
    fn non_tondo_or_indented_fences_are_ignored_and_empty_sources_are_canonical() {
        let document = b"```tondo\nignored\n```\n ~~~tondo\nignored\n~~~\n~~~tondo pseudocode\n~~~\n~~~tondo\nlet value=1\n~~~";
        let fences = extract_fences(document, &errors()).unwrap();

        assert_eq!(fences.len(), 2);
        assert_eq!(fences[0].category, DocCategory::Pseudocode);
        assert_eq!(fences[0].source, b"\n");
        assert_eq!(fences[0].source_sha256, sha256(b"\n"));
        assert_eq!(fences[1].source, b"let value=1\n");
        assert!(fences[0].fence_byte < fences[1].fence_byte);
    }

    #[test]
    fn unicode_prefixes_and_crlf_use_original_byte_offsets() {
        let lf = "á\n~~~tondo\nvalue\n~~~\n".as_bytes();
        let crlf = "á\r\n~~~tondo\r\nvalue\r\n~~~\r\n".as_bytes();
        let lf_fence = extract_fences(lf, &errors()).unwrap();
        let crlf_fence = extract_fences(crlf, &errors()).unwrap();

        assert_eq!(lf_fence[0].fence_byte, 3);
        assert_eq!(crlf_fence[0].fence_byte, 4);
        assert_eq!(lf_fence[0].source, crlf_fence[0].source);
        assert_eq!(lf_fence[0].source_sha256, crlf_fence[0].source_sha256);
    }
}
