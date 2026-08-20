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

El cierre es una coordinación de implementación, no una conformidad global.
La auditoría pública que alimenta el registro ya está verificada (214/214):
las firmas de MessagePack/Protobuf atraviesan sus rutas públicas y los tres
owners build-only tienen una razón `not-applicable` explícita. No relaja
`--strict` ni convierte una frontera build-only en un provider runtime. La
siguiente coordinación de implementación es `NATIVE-PUBLISH-SPEC-001`, después
del cierre de `NATIVE-LINK-PLAN-001`, y queda condicionada
por las celdas de promoción que aún conserva S1A.

```text
scripts/stdlib-hosted-implementation-coordination-check.sh
scripts/stdlib-hosted-implementation-coordination-test.sh
```

El estado `closed-coordination` solo afirma que este conjunto de owners
comparte una prueba completa de implementación y capability; no afirma una
publicación de Tondo ni cierra las celdas pendientes de rendimiento o
conformance; `STD-A-FUZZ-001` coordina el fuzz owner-aware por separado.
