# Source model contract

**Status:** accepted for M0

## Stable and local identities

- `SourceId` is an opaque, non-empty UTF-8 string without LF.
- `ModulePath` is dot-separated and normalized component-wise to NFC.
- `LogicalPath` is relative, uses `/`, contains no empty, `.` or `..`
  components, and is normalized component-wise to NFC.
- `(SourceId, ModulePath, LogicalPath)` is unique inside a source database.
- `FileId` is a compact request-local index and never appears in public output.

From M2 onward, the closed package graph maps each `SourceId` one-to-one to a
`PackageId` and declares every available module. `CompilationRequest` validates
that ownership before lexing; see `package-graph.md`.

## Bytes and positions

Source snapshots are immutable bytes. Invalid UTF-8 remains representable so the
lexer can issue `E0001` at the exact byte.

- Offsets are zero-based bytes.
- Ranges are semi-open `[start, end)`.
- Line and column are zero-based internally.
- Columns count Unicode scalar values.
- A lazy line index is created on first position lookup.
- At and before the first invalid UTF-8 byte, positions retain line and column
  when the prefix is unambiguous. Later positions retain the byte and may omit
  line and column.

The M0 representation uses `u32` offsets, so one source snapshot cannot exceed
4 GiB. The lower default resource limit is an implementation budget, not a
language rule.

## Physical and virtual sources

Both origins use the same `SourceInput` and compilation path. Physical paths are
used only to read bytes at the host boundary. Tests and generated sources can
provide virtual bytes directly. Origin does not change source semantics.

## Deterministic ordering

Serialized locations use source ID, module, logical file path, and byte range.
They never use `FileId`, allocation address, insertion order, or physical path.
