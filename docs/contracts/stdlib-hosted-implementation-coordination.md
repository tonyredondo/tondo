# Coordinación de implementación Hosted de `std.0.1A`

`STD-IMPL-002` es el gate vertical de los cuatro owners Hosted. Su registro
reproducible es
[`testing/stdlib-hosted-implementation-coordination.json`](../../testing/stdlib-hosted-implementation-coordination.json)
y se genera con
[`stdlib-hosted-implementation-coordination-generate.sh`](../../scripts/stdlib-hosted-implementation-coordination-generate.sh).

El coordinador exige, para `std.console`, `std.path`, `std.fs` y
`std.process`, una ruta de implementación y tests existente, la etapa
`IMPL/HOST` verificada en la matriz, la evidencia de owner correspondiente y
la verificación de todas las firmas públicas. También comprueba que la
capability declarada por cada owner coincide exactamente con
`testing/stdlib-hosted.json`:

- `std.console` requiere `console` y conserva los bridges de streams de
  `std.io`.
- `std.path` no requiere capability: es puramente léxico, preserva bytes
  nativos y no consulta el filesystem.
- `std.fs` requiere `filesystem` y conserva handles, cleanup y límites en el
  bridge Hosted.
- `std.process` requiere `process` y conserva argv exacto, pipes, streams
  separados/combined, cancelación y reap de hijos.

El cierre es una coordinación de implementación, no una conformidad global:
la auditoría pública mantiene visibles sus 32 gaps de firma de MessagePack /
Protobuf y sus tres owners build-only sin superficie indexable. No relaja
`--strict` ni convierte esos gaps en un waiver. El siguiente bloque explícito
es `STD-CODEC-PUBLIC-001`.

```text
scripts/stdlib-hosted-implementation-coordination-check.sh
scripts/stdlib-hosted-implementation-coordination-test.sh
```

El estado `closed-coordination` solo afirma que este conjunto de owners
comparte una prueba completa de implementación y capability; no afirma una
publicación de Tondo ni cierra las celdas pendientes de fuzz, rendimiento o
conformance.
