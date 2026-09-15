//! Unicode scalar values in private nonnegative integer carriers.
//! Literal validation is independent of the hosted VM's decoder.

pub(super) fn parse_literal(spelling: &str) -> Result<i64, String> {
    decode_literal(spelling)
        .map(|value| i64::from(u32::from(value)))
        .ok_or_else(|| format!("invalid native Char literal: {spelling:?}"))
}

fn decode_literal(spelling: &str) -> Option<char> {
    let body = spelling.strip_prefix('\'')?.strip_suffix('\'')?;
    match body {
        "\\n" => Some('\n'),
        "\\r" => Some('\r'),
        "\\t" => Some('\t'),
        "\\\\" => Some('\\'),
        "\\'" => Some('\''),
        "\\0" => Some('\0'),
        _ => {
            if let Some(digits) = body
                .strip_prefix("\\u{")
                .and_then(|body| body.strip_suffix('}'))
            {
                if !(1..=6).contains(&digits.len())
                    || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                return char::from_u32(u32::from_str_radix(digits, 16).ok()?);
            }
            let mut characters = body.chars();
            let value = characters.next()?;
            (characters.next().is_none()
                && !value.is_ascii_control()
                && !matches!(value, '\\' | '\''))
            .then_some(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_preserve_unicode_scalar_values_and_exact_escapes() {
        for (spelling, expected) in [
            ("'a'", 0x61),
            ("'ñ'", 0xf1),
            ("'λ'", 0x3bb),
            ("'🙂'", 0x1f642),
            ("'\\u{10FFFF}'", 0x10ffff),
            ("'\\u{D7FF}'", 0xd7ff),
            ("'\\u{E000}'", 0xe000),
            ("'\\u{000061}'", 0x61),
            ("'\\u{0}'", 0),
            ("'\\u{7F}'", 0x7f),
            ("'\\n'", 10),
            ("'\\r'", 13),
            ("'\\t'", 9),
            ("'\\0'", 0),
            ("'\\\\'", 92),
            ("'\\''", 39),
            ("'\"'", 34),
            ("'\u{301}'", 0x301),
        ] {
            assert_eq!(parse_literal(spelling), Ok(expected), "{spelling}");
        }
    }

    #[test]
    fn literals_reject_invalid_scalars_escapes_and_multiple_characters() {
        for spelling in [
            "",
            "a",
            "''",
            "'ab'",
            "'e\u{301}'",
            "'🙂a'",
            "'a'junk",
            "'''",
            "'\\'",
            "'\n'",
            "'\r'",
            "'\t'",
            "'\0'",
            "'\u{7f}'",
            "'\\x61'",
            "'\\b'",
            "'\\\"'",
            "'\\u{}'",
            "'\\u{+61}'",
            "'\\u{6_1}'",
            "'\\u{0000061}'",
            "'\\u{D800}'",
            "'\\u{DFFF}'",
            "'\\u{110000}'",
            "'\\u{FFFFFFFF}'",
            "'\\u{61}x'",
        ] {
            assert!(parse_literal(spelling).is_err(), "{spelling:?}");
        }
    }
}
