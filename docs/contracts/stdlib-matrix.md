# Matriz normativa de `stdlib` 0.1A

`STD-MATRIX-ALL-001` coordina la trazabilidad completa del catálogo actual de
`STD-0.1A`. Su fuente machine-readable es
[`testing/stdlib-matrix.json`](../../testing/stdlib-matrix.json), generada de
forma reproducible por
[`scripts/stdlib-matrix-generate.sh`](../../scripts/stdlib-matrix-generate.sh)
y validada por
[`scripts/stdlib-matrix-check.sh`](../../scripts/stdlib-matrix-check.sh).

La matriz incluye exactamente los 21 owners del contrato de integración y el
owner intrínseco `std.bytes`, 207 firmas indexadas por la auditoría pública y
145 requisitos de owner. `std.meta` añade su contrato executable A0 y sus seis
requisitos de evidencia sin crear una segunda API pública. `std.0.1B` permanece
como catálogo futuro cerrado:
sus módulos aparecen solo en `catalogs.future_modules` y no se convierten en
requisitos implícitos de la fase actual.

## Celdas obligatorias

Cada fila contiene referencias explícitas a las seis celdas:

```text
SPEC → IMPL/HOST → MODEL/TEST/FUZZ → PERF → CONF → DOC
```

Las filas de firma apuntan a la fila correspondiente de
`testing/stdlib-public-api.json` para conservar los gaps de HIR, lowering y
caso público sin duplicar evidencia. Las filas de requisito apuntan al
contrato del owner y a su `test_matrix`. Las seis celdas se materializan como
`stage_refs`; el registro del owner contiene el estado y las razones de cada
celda. `dimensions_ref` enlaza cada fila con las dimensiones públicas de su
owner, evitando una cifra agregada entre módulos incompatibles.

`verified` solo significa que esa celda tiene evidencia suficiente. `partial`,
`pending`, `gap` y `not-applicable` exigen una razón no vacía. La matriz
permanece `open-gaps` mientras exista cualquier fila no completamente
verificada; no hay waivers silenciosos ni promoción implícita por tener
tests de kernel.

## Reproducibilidad y promoción

El checker regenera la matriz en un directorio temporal y compara el resultado
byte a byte con el archivo versionado. También exige que todas las firmas de la
auditoría pública estén presentes exactamente una vez, que cada requisito
tenga owner, que las dimensiones PERF tengan owner group y que todas las
referencias apunten a paths existentes. El test negativo elimina un owner y
una razón de estado para demostrar que el gate falla cerrado.

Esta matriz no cierra `STD-CONF-001`, `STD-TEST-001` ni `STD-DOC-001`: registra
sus celdas pendientes para que las siguientes coordinaciones puedan promover
owners sin perder la identidad de requisito.
