# ADR-003: Use a lossless CST

**Status:** accepted

## Context

Formatting, comments, fixes, documentation, and semantic tooling must agree on
one representation of the source.

## Decision

The parser produces an immutable, lossless concrete syntax tree (CST). Its
contract is deliberately small:

- A green tree owns only node kinds, token kinds, byte ranges, child order, and
  synthetic-token metadata. It never owns a second copy of source text.
- Every physical source byte belongs to exactly one non-synthetic token. Reading
  those token ranges in tree order reconstructs the input byte for byte, even
  for malformed UTF-8 and malformed literals.
- Whitespace, physical newlines, line comments, block comments, documentation
  comments, and a script shebang are tokens in the tree. They are marked as
  trivia but are not discarded by the parser.
- A logical `NL` is a zero-width synthetic token. It records the lexer decision
  from specification section 5.2 without claiming ownership of the physical
  newline bytes. Consecutive physical newlines may therefore correspond to one
  logical `NL` while their original trivia remains intact.
- `EOF` and missing recovery tokens are also zero-width and synthetic. A missing
  token carries the kind expected by the parser; it never fabricates source
  text.
- An unexpected physical token remains under an `Error` node. Recovery may add
  missing tokens but may not delete, reorder, or reinterpret input tokens.
- Context-dependent forms have explicit preliminary node kinds. In particular,
  bracket postfixes, path-plus-record bodies, parenthesized closure candidates,
  constructor patterns, and `for` headers remain unresolved in the CST.
- Node and token ranges are half-open byte ranges. A node range covers its first
  through last physical child; a node with only synthetic children is empty at
  the parser insertion point.

The root kind records the selected source form (`Module`, `Script`, or
`Fragment`). A source-form error is represented by diagnostics plus ordinary
error nodes, not by silently retrying a different grammar.

The concrete Rust representation uses a compact flat arena. Children refer to
node or token indices, making ownership acyclic and iteration deterministic.
Typed AST accessors are checked views over arena nodes; they never copy text,
trivia, or spans and do not constitute a second parser.

These invariants are public compiler contracts and have direct unit tests:
lossless reconstruction, total byte ownership, deterministic child order,
zero-width synthetic tokens, and preservation of unexpected input.

## Consequences

The semantic pipeline does not use a second parser. HIR may discard trivia only
after all source relationships have stable ranges.

Logical newlines do not compromise losslessness, and parser recovery remains
visible to diagnostics and tooling. Consumers that want semantic tokens skip
trivia and synthetic recovery tokens; formatters and editors retain both.
