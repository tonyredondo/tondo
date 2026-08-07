# Auditoría pública de `std.0.1A`

`STD-PUBLIC-API-AUDIT-001` es una comprobación de trazabilidad, no una
afirmación de que los owners estén terminados. Su entrada declarativa es
[`testing/stdlib-public-api-config.json`](../../testing/stdlib-public-api-config.json)
y su salida reproducible es
[`testing/stdlib-public-api.json`](../../testing/stdlib-public-api.json). El
script [`scripts/stdlib-public-api-audit.sh`](../../scripts/stdlib-public-api-audit.sh)
extrae las firmas de los contratos y genera una fila por firma.

## Cadena exigida

Cada fila conserva la misma identidad desde el contrato hasta un caso público:

```text
contract signature → HIR symbol → lowering symbol → host/VM symbol → public case call
```

Una fila solo es `verified` cuando existen los paths declarados, el símbolo de
la operación aparece en las etapas HIR y lowering, el host/VM contiene el
símbolo cualificado (o la primitiva VM explícita para los intrinsics Core), y
el caso contiene una llamada al nombre canónico. La llamada no se satisface
con un path Rust aislado, un test que ejerce otra operación, una documentación,
un alias bootstrap ni un registro runtime paralelo.

Los owners build-only pueden declarar `host_vm.kind = not-applicable`, pero la
razón debe ser normativa. Si el contrato no expone ninguna firma indexable, el
owner queda abierto con `no-callable-signatures-indexed`; esto evita confundir
una implementación de soporte con una API pública auditada.

## Modos

```text
scripts/stdlib-public-api-audit.sh --write   # regenera la matriz canónica
scripts/stdlib-public-api-audit.sh --check   # comprueba que no haya drift
scripts/stdlib-public-api-audit.sh --strict  # falla ante cualquier hueco
```

`--check` es el modo que puede formar parte del gate diario: informa de los
huecos sin ocultarlos y mantiene el workspace verificable mientras los leaves
de implementación siguen abiertos. `--strict` es el gate de promoción S1A y
debe pasar antes de cerrar `STD-IMPL-001`, `STD-IMPL-002` o cualquier leaf de
evidencia.

La matriz actual registra deliberadamente `open-gaps`: los codecs typed y
streaming, varios métodos Hosted, la superficie completa de `std.testing` y
las APIs build-only todavía no atraviesan una llamada pública para cada firma.
Es una señal fail-closed para el tracker, no un waiver.

## Invariantes de la matriz

- hay exactamente un owner por identidad de firma;
- el contrato y los stages se fijan por paths relativos al repositorio;
- el caso público declara su kind (`runtime`, `compile` o `runner-source`);
- `bootstrap_alias` es siempre `false`;
- los estados se derivan de `missing`, nunca se editan a mano; y
- cualquier drift del contrato o de la configuración hace fallar `--check`.

La matriz no sustituye la ejecución del caso. La ejecución, coverage,
mutation, conformidad y performance siguen siendo gates separados y deben
apuntar a la misma identidad de fila cuando el owner se promueva.
