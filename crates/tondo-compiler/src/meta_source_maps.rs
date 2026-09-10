//! Compose provider byte ranges with token provenance from canonical formatting.

use crate::meta::{MetaContractError, MetaSourceMapEntry};
use crate::source::TextRange;
use crate::syntax::MappedFormattedSource;

pub(crate) fn compose(
    original: &[u8],
    formatted: &MappedFormattedSource,
    mappings: &[MetaSourceMapEntry],
) -> Result<Vec<MetaSourceMapEntry>, MetaContractError> {
    let text = std::str::from_utf8(original).map_err(|_| MetaContractError::InvalidSourceMap)?;
    let mut mappings = mappings.to_vec();
    mappings.sort();
    let mut previous_end = 0;
    let mut formatted_end = 0;
    let mut result = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let start = mapping.generated_start();
        let end = mapping.generated_end();
        if start > end
            || start < previous_end
            || !text.is_char_boundary(start as usize)
            || !text.is_char_boundary(end as usize)
        {
            return Err(MetaContractError::InvalidSourceMap);
        }
        previous_end = end;
        let original =
            TextRange::new(start, end).map_err(|_| MetaContractError::InvalidSourceMap)?;
        let mapped = formatted
            .map_range(original)
            .ok_or(MetaContractError::InvalidSourceMap)?;
        if mapped.start() < formatted_end {
            return Err(MetaContractError::InvalidSourceMap);
        }
        formatted_end = mapped.end();
        result.push(MetaSourceMapEntry::new(
            mapped.start(),
            mapped.end(),
            mapping.origin(),
        )?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::MetaSpan;
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{
        LexMode, ParseLimits, ParseMode, format_parsed, format_parsed_with_mappings, lex, parse,
    };

    fn formatted(text: &str) -> MappedFormattedSource {
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("generated:test").unwrap(),
                ModulePath::new("generated").unwrap(),
                LogicalPath::new("generated/test.to").unwrap(),
                text.as_bytes(),
            ))
            .unwrap();
        let parsed = parse(
            &sources,
            file,
            lex(&sources, file, LexMode::Module).unwrap(),
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        let mapped = format_parsed_with_mappings(&sources, file, &parsed).unwrap();
        assert_eq!(
            mapped.source(),
            &format_parsed(&sources, file, &parsed).unwrap()
        );
        mapped
    }

    #[test]
    fn meta_source_mapping_preserves_unicode_and_exact_ranges_across_formatting() {
        let original = "impl  Display for Item {\nfn display(self):String{\"λ🙂\"}\n}\n";
        let formatted = formatted(original);
        let start = original.find("λ🙂").unwrap();
        let origin = MetaSpan::new(3, 11, 17).unwrap();
        let mappings = compose(
            original.as_bytes(),
            &formatted,
            &[
                MetaSourceMapEntry::new(start as u32, (start + "λ🙂".len()) as u32, origin)
                    .unwrap(),
            ],
        )
        .unwrap();
        let mapped = mappings[0];
        assert_eq!(
            &formatted.source().bytes()
                [mapped.generated_start() as usize..mapped.generated_end() as usize],
            "λ🙂".as_bytes()
        );
        assert_eq!(mapped.origin(), origin);
        assert_ne!(mapped.generated_start(), start as u32);
        let whole = compose(
            original.as_bytes(),
            &formatted,
            &[MetaSourceMapEntry::new(0, original.len() as u32, origin).unwrap()],
        )
        .unwrap();
        assert_eq!(
            whole[0].generated_end() as usize,
            formatted.source().bytes().len()
        );
    }

    #[test]
    fn meta_source_mapping_rejects_removed_trivia_utf8_splits_overlap_and_rewritten_bytes() {
        let original = "fn   cafe\u{301}(): String { \"é\" }\n";
        let formatted = formatted(original);
        let origin = MetaSpan::new(0, 0, 1).unwrap();
        let accent = original.find('é').unwrap() as u32;
        let name = original.find("cafe").unwrap() as u32;
        for ranges in [
            vec![(3, 4)],
            vec![(accent, accent + 1)],
            vec![(0, 5), (4, 8)],
            vec![(0, original.len() as u32 + 1)],
            vec![(name + 1, name + 2)],
        ] {
            let mappings = ranges
                .into_iter()
                .map(|(start, end)| MetaSourceMapEntry::new(start, end, origin).unwrap())
                .collect::<Vec<_>>();
            assert!(matches!(
                compose(original.as_bytes(), &formatted, &mappings),
                Err(MetaContractError::InvalidSourceMap)
            ));
        }
    }
}
