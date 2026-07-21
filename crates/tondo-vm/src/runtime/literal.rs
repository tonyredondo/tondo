pub(super) fn integer(spelling: &str) -> Option<i128> {
    let suffix = ["i16", "i32", "i64", "u16", "u32", "u64", "i8", "u8"]
        .into_iter()
        .find(|suffix| spelling.ends_with(suffix));
    let body = suffix.map_or(spelling, |suffix| {
        &spelling[..spelling.len() - suffix.len()]
    });
    let (radix, digits) = if let Some(digits) = body.strip_prefix("0b") {
        (2, digits)
    } else if let Some(digits) = body.strip_prefix("0o") {
        (8, digits)
    } else if let Some(digits) = body.strip_prefix("0x") {
        (16, digits)
    } else {
        (10, body)
    };
    let magnitude = u128::from_str_radix(&digits.replace('_', ""), radix).ok()?;
    i128::try_from(magnitude).ok()
}

pub(super) fn float(spelling: &str, single_precision: bool) -> Option<f64> {
    let suffix = ["f32", "f64"]
        .into_iter()
        .find(|suffix| spelling.ends_with(suffix));
    let body = suffix.map_or(spelling, |suffix| {
        &spelling[..spelling.len() - suffix.len()]
    });
    let body = body.replace('_', "");
    if single_precision {
        let value = body.parse::<f32>().ok()?;
        value.is_finite().then_some(f64::from(value))
    } else {
        let value = body.parse::<f64>().ok()?;
        value.is_finite().then_some(value)
    }
}

pub(super) fn character(spelling: &str) -> Option<char> {
    let body = spelling.strip_prefix('\'')?.strip_suffix('\'')?;
    let decoded = escaped(body, false)?;
    let mut characters = decoded.chars();
    let value = characters.next()?;
    characters.next().is_none().then_some(value)
}

pub(super) fn string(spelling: &str) -> Option<String> {
    let (raw, multiline, opening, closing) = if spelling.starts_with("r\"\"\"") {
        (true, true, "r\"\"\"", "\"\"\"")
    } else if spelling.starts_with("r\"") {
        (true, false, "r\"", "\"")
    } else if spelling.starts_with("\"\"\"") {
        (false, true, "\"\"\"", "\"\"\"")
    } else if spelling.starts_with('\"') {
        (false, false, "\"", "\"")
    } else {
        return None;
    };
    let body = spelling.strip_prefix(opening)?.strip_suffix(closing)?;
    let body = if multiline {
        normalize_multiline(body)
    } else {
        body.to_owned()
    };
    if raw {
        Some(body)
    } else {
        escaped(&body, true)
    }
}

fn normalize_multiline(body: &str) -> String {
    let mut normalized = body.replace("\r\n", "\n");
    if normalized.starts_with('\n') {
        normalized.remove(0);
    }
    let line_start = normalized.rfind('\n').map_or(0, |index| index + 1);
    if !normalized[line_start..]
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return normalized;
    }
    let prefix = normalized[line_start..].to_owned();
    normalized.truncate(if line_start == 0 { 0 } else { line_start - 1 });
    normalized
        .split('\n')
        .map(|line| {
            if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
                let common = line
                    .bytes()
                    .zip(prefix.bytes())
                    .take_while(|(left, right)| left == right)
                    .count();
                &line[common..]
            } else {
                line.strip_prefix(&prefix).unwrap_or(line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn escaped(body: &str, decode_braces: bool) -> Option<String> {
    let mut output = String::with_capacity(body.len());
    let mut characters = body.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next()? {
                'n' => output.push('\n'),
                'r' => output.push('\r'),
                't' => output.push('\t'),
                '\\' => output.push('\\'),
                '\'' => output.push('\''),
                '"' => output.push('"'),
                '0' => output.push('\0'),
                'u' => {
                    if characters.next()? != '{' {
                        return None;
                    }
                    let mut digits = String::new();
                    loop {
                        let digit = characters.next()?;
                        if digit == '}' {
                            break;
                        }
                        digits.push(digit);
                    }
                    if !(1..=6).contains(&digits.len()) {
                        return None;
                    }
                    output.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
                }
                _ => return None,
            },
            '{' | '}' if decode_braces => {
                characters.next_if_eq(&character)?;
                output.push(character);
            }
            _ => output.push(character),
        }
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_decode_to_runtime_values_without_host_locale() {
        assert_eq!(integer("0xffu16"), Some(255));
        assert_eq!(integer("9_223"), Some(9_223));
        assert_eq!(float("1.5f32", true), Some(1.5));
        assert_eq!(character("'\\u{1f642}'"), Some('🙂'));
        assert_eq!(string("\"left{{right}}\\n\""), Some("left{right}\n".into()));
        assert_eq!(
            string("\"\"\"\n    first\n    second\n    \"\"\""),
            Some("first\nsecond".into())
        );
    }
}
