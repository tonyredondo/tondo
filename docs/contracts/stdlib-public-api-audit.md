# Auditoría pública de `std.0.1A`

`STD-PUBLIC-API-AUDIT-001` es una comprobación de trazabilidad, no una
afirmación de que los owners estén terminados. Su entrada declarativa es
[`testing/stdlib-public-api-config.json`](../../testing/stdlib-public-api-config.json)
y su salida reproducible es
[`testing/stdlib-public-api.json`](../../testing/stdlib-public-api.json). El
script [`scripts/stdlib-public-api-audit.sh`](../../scripts/stdlib-public-api-audit.sh)
extrae las firmas de los contratos y genera una fila por firma.
Solo se indexan declaraciones fuente canónicas `pub fn`: no existe una familia
`async fn`. El efecto postfix `suspends` pertenece a la firma publicada y al
hash de API; es obligatorio en contratos sin cuerpo e inferible en
implementaciones con cuerpo.

## Cadena exigida

Cada fila conserva la misma identidad desde el contrato hasta un caso público:

```text
contract signature → HIR symbol → lowering symbol → host/VM symbol → public case call
```

Una fila solo es `verified` cuando existen los paths declarados, el símbolo de
la operación aparece en las etapas HIR y lowering, el host/VM contiene el
símbolo cualificado (o una primitiva VM explícita para los intrinsics Core), y
el caso contiene una llamada al nombre canónico. La llamada no se satisface
con un path Rust aislado, un test que ejerce otra operación, una documentación,
un alias bootstrap ni un registro runtime paralelo.

Los owners build-only pueden declarar `host_vm.kind = not-applicable`, pero la
razón debe ser normativa y el caso debe apuntar a una raíz compiler-owned
`crates/...` con `case.kind = build-only`. Si el contrato no expone ninguna
firma indexable, el owner queda verificado solo cuando esa frontera build-only
es explícita; un owner runtime sin firmas sigue abierto con
`no-callable-signatures-indexed`. Esto evita confundir una implementación de
soporte con una API pública auditada.

Los intrinsics Core que se materializan como agregados y ramas MIR usan
`host_vm.kind = vm-inline`: la matriz conserva los símbolos exactos del
lowering y del runtime VM común, sin inventar un registro de operaciones
paralelo ni exigir una función host para una operación puramente portable.

## Modos

```text
scripts/stdlib-public-api-audit.sh --write   # regenera la matriz canónica
scripts/stdlib-public-api-audit.sh --check   # comprueba que no haya drift
scripts/stdlib-public-api-audit.sh --strict  # falla ante cualquier hueco
```

`--check` es el modo que puede formar parte del gate diario: informa de los
huecos sin ocultarlos y mantiene el workspace verificable mientras los leaves
de implementación siguen abiertos. `--strict` es el gate de promoción S1A y
debe pasar antes de una promoción global. El cierre coordinado de
`STD-IMPL-001` usa además
[`stdlib-implementation-coordination.md`](./stdlib-implementation-coordination.md):
solo promueve los owners Core/serialization que ya tienen evidencia completa;
no sustituye las celdas de promoción de la matriz. Así no se confunde una
coordinación parcial con un waiver del modo estricto.
`STD-IMPL-002` usa además
[`stdlib-hosted-implementation-coordination.md`](./stdlib-hosted-implementation-coordination.md):
solo promueve los cuatro owners Hosted cuando sus capabilities, bridges y
firmas públicas están verificadas; el resultado global de `--strict` queda
determinado por la matriz completa y sus razones normativas.

La auditoría actual registra `verified` con 214/214 firmas y cero gaps. Las
llamadas públicas de codecs incluyen rutas dynamic/typed y streaming; las
fronteras build-only se verifican por su caso compiler-owned y razón
`not-applicable`, sin fabricar una llamada runtime. La matriz normativa puede
seguir `open-gaps` por requisitos de fuzz, rendimiento, conformance o promoción;
eso es una señal fail-closed del tracker, no un waiver de esta auditoría.

## Invariantes de la matriz

- hay exactamente un owner por identidad de firma;
- el contrato y los stages se fijan por paths relativos al repositorio;
- el caso público declara su kind (`runtime`, `compile`, `runner-source` o
  `build-only`); un `build-only` debe ser compiler-owned y no aplicable en
  runtime;
- `hir.symbols`, `lowering.symbols` y `host_vm.symbols` conservan los tokens
  implementativos usados para verificar cada ruta. Cuando una fase usa un
  nombre interno distinto de la firma pública, el alias se declara de forma
  explícita en la configuración; nunca se acepta una coincidencia accidental
  por texto común;
- `bootstrap_alias` es siempre `false`;
- los estados se derivan de `missing`, nunca se editan a mano; y
- cualquier drift del contrato o de la configuración hace fallar `--check`.

La matriz no sustituye la ejecución del caso. La ejecución, coverage,
mutation, conformidad y performance siguen siendo gates separados y deben
apuntar a la misma identidad de fila cuando el owner se promueva.

## Coordinación normativa

La auditoría por firma alimenta la matriz normativa de
[`STD-MATRIX-ALL-001`](./stdlib-matrix.md). Esa matriz añade los requisitos
de los contratos de owner, `std.bytes` y las dimensiones públicas de PERF, y
expone las seis celdas `SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF →
DOC`. Por eso una fila de esta auditoría puede estar `verified` y seguir
`open-gaps` en la matriz: la implementación de la firma no demuestra todavía
su modelo, fuzz, presupuesto de coste, conformance o documentación de owner.
