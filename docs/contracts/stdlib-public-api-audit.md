# Auditoría pública de `std.0.1A`

`STD-PUBLIC-API-AUDIT-001` es una comprobación de trazabilidad, no una
afirmación de que los owners estén terminados. Su entrada declarativa es
[`testing/stdlib-public-api-config.json`](../../testing/stdlib-public-api-config.json)
y su salida reproducible es
[`testing/stdlib-public-api.json`](../../testing/stdlib-public-api.json). El
script [`scripts/stdlib-public-api-audit.sh`](../../scripts/stdlib-public-api-audit.sh)
extrae las firmas de los contratos y genera una fila por firma.
Solo se indexan declaraciones fuente canónicas `pub fn`: la ausencia de una
keyword `async` es normativa, mientras que `suspends` pertenece a la interfaz
publicada y al hash de API, no a la declaración que escribe el autor.

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
razón debe ser normativa. Si el contrato no expone ninguna firma indexable, el
owner queda abierto con `no-callable-signatures-indexed`; esto evita confundir
una implementación de soporte con una API pública auditada.

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
solo promueve los owners Core/serialization que ya tienen evidencia completa y
mantiene los gaps globales como trabajo posterior. Así no se confunde una
coordinación parcial con un waiver del modo estricto.

La matriz actual registra deliberadamente `open-gaps`: los codecs typed y
streaming, varios métodos Hosted, la superficie completa de `std.testing` y
las APIs build-only todavía no atraviesan una llamada pública para cada firma.
Es una señal fail-closed para el tracker, no un waiver.

## Invariantes de la matriz

- hay exactamente un owner por identidad de firma;
- el contrato y los stages se fijan por paths relativos al repositorio;
- el caso público declara su kind (`runtime`, `compile` o `runner-source`);
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
