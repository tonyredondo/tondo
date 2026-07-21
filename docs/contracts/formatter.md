# Canonical formatter contract

**Status:** implemented for Tondo 0.1-draft.8

## Public boundary

The formatter is a terminal branch of the ordinary frontend:

~~~text
SourceDatabase + FileId
  -> lex
  -> parse in the declared source form
  -> diagnostic-free Parsed CST
  -> format_parsed
  -> FormattedSource
~~~

`format_parsed` consumes the lossless CST rather than reparsing text or building
a second syntax tree. It returns owned UTF-8 bytes and never mutates the source
database. A source with lexical or syntax diagnostics is rejected before this
boundary; calling the API with a `Parsed` value that contains diagnostics
returns `FormatError::InvalidSyntax`.

The compilation driver retains every parsed source until frontend validation is
complete and formats only the request root. Module, script, and fragment roots
use their corresponding lexer and parser modes. Imported sources are modules.

## Deterministic layout

The internal document algebra contains text, hard lines, breakable lines with
or without a flat space, concatenation, indentation, groups, and conditional
broken/flat documents. Rendering is deterministic:

- Maximum width is 100 Unicode scalar values, not UTF-8 bytes.
- Indentation is four ASCII spaces.
- A group stays flat when its flat form fits the remaining width; nested groups
  are considered in document order.
- Indentation is emitted lazily, so empty lines contain no trailing spaces.
- Output uses `LF`, has no trailing horizontal whitespace, and ends in exactly
  one `LF`.
- Identifiers are emitted in their normalized NFC spelling; literal contents
  retain their source spelling and value.

Lists share one layout for parameters, arguments, generic items, tuples,
arrays, maps, sets, and patterns. Flat lists use comma-space separators. Broken
lists place one item per line and have a trailing comma, including when a break
is forced by a comment. Empty permitted forms remain compact.

Records use comma-space separators when flat and significant newlines without
commas when broken. The lexer restores logical newlines inside a brace nested in
parentheses or brackets, so this representation remains valid in every
expression position. Declarations, blocks, `match`, trait bodies, and
implementation bodies use their mandatory multiline forms.

Binary and type-operator chains break after the operator with one continuation
indent. Assignment and match-arm bodies use the same safe break direction.
Postfix and path chains break before a dot. Every generated break therefore
coincides with the language's logical-newline suppression rules.

## Comments, imports, and roots

Line, block, and documentation comments are attached deterministically as
leading, trailing, or section runs. The formatter preserves their text and
meaningful section boundaries, removes indentation-only trivia, and never drops
comments attached to separators. Documentation comments remain adjacent to the
item they document. A comment that cannot be represented safely inline forces a
multiline layout.

Imports are sorted only within a contiguous import group. Blank sections and
section comments delimit groups, and comments attached to an import move with
that import. Declarations retain their source order.

A script shebang is preserved as the first atom and separated from following
content by one blank line. Module and fragment roots cannot acquire a shebang
through formatting.

## CLI behavior

`tondo fmt source.to` validates the module source and writes canonical bytes to
stdout; it never edits `source.to`. `tondo fmt --check source.to` is silent and
returns exit code 0 exactly when the input bytes are already the formatter fixed
point. A difference returns exit code 1. Syntax and resource diagnostics also
return exit code 1 on stderr and no formatter bytes on stdout.

The bootstrap CLI currently exposes a loose module root only. Embedding callers
can select script or fragment roots through `CompilationRequest`.

## Validation obligations

The maintained suite proves:

- byte-exact output for the normative minimum corpus;
- the 99/100/101-column boundaries for every shared list family;
- comments, doc comments, import sections, shebang, CRLF, NFC, multiline
  literals, records, control flow, match, operator chains, and postfix chains;
- parseability and idempotence for every syntactically valid Tondo fence in the
  pinned specification;
- a fixed point for 512 deterministic grammar-generated valid programs;
- rejection of invalid recoverable syntax and resource exhaustion without
  fabricated or partial output.

For every valid covered input `source`, the public invariant is:

~~~text
parse(F(source)) succeeds
F(F(source)) == F(source)
~~~
