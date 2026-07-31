# CODEOWNERS resolution contract

**Status:** implemented for `UTEST-OWNERS-001`

`tondo_compiler::test_owners` is the pure ownership boundary. The host supplies
the candidate paths and bytes; the module never opens files, follows symlinks,
consults a provider, checks permissions or makes a network request.

## File selection

`auto` considers exactly `.github/CODEOWNERS`, `CODEOWNERS` and
`docs/CODEOWNERS`, in that order. The first present candidate is authoritative;
an invalid or unreadable first candidate is an error and never falls back.
`none` produces no source, hash or rules. An explicit logical path requires a
present, regular, readable, non-escaping candidate and reports that path as the
source. Candidate paths are canonical repository-relative paths and duplicate
or unknown automatic candidates are rejected.

The selected bytes are UTF-8 without a BOM. The resolution records the original
logical source path and the lowercase 64-digit SHA-256 of the exact bytes.
Absence in `auto` is valid and yields an empty ownership set; absence for an
explicit path is an error.

## Portable rule parser

Blank lines and lines whose first non-ASCII-blank character is `#` are ignored.
All other lines contain one pattern and at least one owner separated by ASCII
spaces or tabs. There is no inline comment syntax. Owners are opaque non-empty
UTF-8 tokens, preserving textual order and duplicates.

Patterns are case-sensitive and use `/` as the only separator. The portable
subset supports `*` (zero or more scalars except `/`), `?` (one scalar except
`/`) and `**` (zero or more scalars including `/`). A leading `/` anchors at the
first segment; a pattern without `/` matches any complete segment and therefore
can own a whole sub-tree when that segment is non-final. Other patterns match
the full path from the root. A trailing `/` is normalized to `/**`.
Negation, bracket ranges, backslash escapes and `.`/`..` or empty segments are
rejected. The last matching rule wins without changing its owner order.

Generated sources call `owners_for(None)` when no logical origin exists and
receive `[]`; declared origins use their canonical logical source path.

Nine compiler tests cover selection precedence, explicit/disabled modes,
hashing, comments/CRLF, segment/full-path matching, wildcard and case rules,
absence, malformed files/rules, file-state guards, duplicate candidates and
generated sources.
