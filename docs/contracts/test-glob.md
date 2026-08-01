# Portable glob selection

**Status:** implemented for `UTEST-GLOB-001`

`tondo_compiler::test_glob::GlobPattern` implements the closed testing glob
grammar. `::` separates components; `*` matches zero or more Unicode scalars
inside one component; `?` matches exactly one scalar; and `**` matches zero or
more complete components only when it is itself a component. Other scalars are
literal, matching is case-sensitive, and no Unicode normalization, locale,
shell expansion, filesystem lookup, character classes, braces, escapes,
alternatives, or regex operators exist.

Patterns are parsed before execution and reject empty components, isolated
colons, embedded/adjacent globstars, and non-canonical consecutive stars. Both
component matching and component-sequence matching use dynamic programming,
so the work is bounded and cannot backtrack exponentially.

`select_tree` validates the static suite tree, treats a matched suite as the
union of its descendant leaves, deduplicates overlapping matches, and returns
leaves in UTF-8 byte order. The result is intended to be consumed by the shard
stage before execution ordering.

