# Bootstrap syntax contract

**Status:** implemented for Tondo 0.1-draft.8

## Public phases

The syntax frontend has one lossless path:

~~~text
SourceDatabase + FileId + LexMode
  -> Lexed
  -> ParseMode + ParseLimits
  -> Parsed { Cst, diagnostics }
  -> format_parsed (accepted trees only)
  -> FormattedSource
~~~

`Module`, `Script`, and `Fragment` are the public source forms. Imported files
are always lexed and parsed as modules. `SyntaxSequence` and `StandaloneBlock`
are internal document-test surfaces used to validate isolated normative grammar
examples; the compilation driver never selects them for user source files.

Lexical diagnostics stop the public driver before parsing. A lexically valid
source is always parsed before formatting, semantic acceptance, or the
bootstrap `T0001` marker, so
`E0004`, `E0005`, and `E0006` are observable through `fmt`, `check`, and `run`.
Only a diagnostic-free `Parsed` value reaches the formatter.

## CST inventory and ownership

`SyntaxKind` is the closed node inventory. It covers roots, declarations,
types, statements, expressions, postfix forms, patterns, preliminary contextual
forms, and explicit `Error` nodes. ADR-003 defines byte ownership and synthetic
tokens.

The representation is a flat immutable arena:

- `NodeId` and `TokenId` are opaque request-local indices.
- `SyntaxElement` preserves exact child order.
- `SyntaxNodeRef` and `SyntaxTokenRef` are borrowed handles into one `Cst`.
- `DescendantTokens` walks tree order, which may differ from token-arena order
  after recovery tokens have been appended.
- Every physical byte remains in exactly one non-synthetic token even when the
  parser rejects the source.

## Parsing strategy

Declarations, types, statements, and patterns use handwritten recursive
descent. Expressions use Pratt parsing with the binding powers fixed by spec
section 23.19. Equality, comparison, and range families are non-associative and
produce `E0005` when chained without grouping.

Logical-newline insertion follows the innermost open delimiter. Parentheses and
brackets suppress `NL`; a brace nested inside either restores significant
newlines for its body until a still more deeply nested parenthesis or bracket is
entered. This keeps multiline records and blocks parseable in argument and list
positions while retaining ordinary continuation layout.

The CST deliberately retains contextual forms:

- `BracketPostfix` contains `BracketItem` nodes until resolution chooses generic
  arguments, index, or slice.
- `RecordLikeExpr` retains path-plus-brace forms.
- `ClosureExpr` is recognized from the complete parenthesized header plus body,
  without consulting types.
- `ConstructorPattern`, `QualifiedValuePattern`, and `ForHeader` retain the
  information required for semantic classification.

## Recovery

Missing required tokens become zero-width synthetic tokens. Unexpected physical
tokens are attached beneath `Error` nodes. Delimiters and logical newlines are
recovery boundaries and are not consumed merely to make progress.

After an `E0004`, further generic syntax errors on the same logical line are
suppressed. A physical newline can also become a recovery boundary when newline
suppression followed an incomplete construct and the next token clearly starts
an independent declaration or statement. This keeps later declarations intact
without silently retrying another source form. Specific `E0005` and `E0006`
diagnostics take precedence over generic cascades.

## Typed AST facade

`syntax::ast` provides a checked wrapper for every `SyntaxKind`, sum views for
sources, declarations, statements, expressions, patterns, and primary types,
plus filtered typed-child iterators. Each wrapper contains exactly one
`SyntaxNodeRef`; it neither owns nor copies source text, trivia, tokens, or
ranges. Semantic phases must use this facade or the underlying CST and must not
construct a second syntax tree.

## Defensive limits

The parser enforces request-wide node and diagnostic budgets. Recursive CST and
expression nesting is capped at 256 in the bootstrap even if an embedding host
requests a larger value, because exceeding the safe host-stack bound must yield
`ParseResource::NestingDepth`/`T0002`, never abort the process. This is an
implementation safety limit, not Tondo language semantics.

## Validation

The maintained tests cover:

- all 295 Tondo fences in the pinned specification;
- every public source form and all three syntax diagnostic codes;
- byte-for-byte CST reconstruction and synthetic-token invariants;
- precedence, contextual brackets, records, closures, `for`, patterns, and
  multiple assignment;
- local recovery that preserves later methods and declarations;
- every one-byte input, 2,048 deterministic arbitrary byte sequences, and a
  nesting input immediately beyond the safe recursion ceiling;
- 512 deterministic grammar-generated valid programs whose formatted output is
  reparsed and formatted to the same fixed point;
- propagation of syntax diagnostics and resource rejection through the public
  driver and fixture harness.

The formatter's layout and test contract is documented separately in
`docs/contracts/formatter.md`.
