# Contrato del dump lógico `DUMP-001`

**Estado:** `implemented` para la VM hosted de Tondo 0.1. La captura nativa de
señales, registros físicos y unwind de plataforma pertenece a
`DIAG-NATIVE-001`; no se sobreafirma aquí.

Este contrato fija el writer y el analizador offline del envelope
`tondo-dump/1`. El writer recibe una `DiagnosticTrace` ya acotada por el
runtime y proyecta únicamente identidad, stacks lógicos, eventos y metadatos
de recursos. No serializa `RuntimeValue`, bytes de usuario, mensajes de panic,
rutas físicas ni secretos.

## Writer

`DumpArtifact::from_trace` crea las nueve secciones obligatorias:

```text
header, termination, identity, stacks, heap_summary,
resource_ledger, scheduler_tail, redaction, limitations
```

Puede añadir `source_maps` y `retainers` cuando existen en la traza. La
sección `registers` queda ausente en la VM hosted y `limitations.unavailable`
la declara junto a `native-unwind` y `physical-paths`.

El archivo es UTF-8 JSON canónico con extensión `.tdump`. La serialización
ordena recursivamente las claves, no acepta whitespace alternativo y calcula
`content_sha256` sobre el mismo envelope con el hash vacío. El lector rechaza
formatos/versiones desconocidos, secciones ausentes, duplicadas o desconocidas,
shapes inválidos, bytes no canónicos, hashes incorrectos y archivos mayores de
256 MiB. El límite es fail-closed.

La identidad obligatoria es `run_id`, `attempt_id`, `shard`, `profile`,
`target`, `backend`, `toolchain` y `source_revision`. La terminación conserva
`reason`, `program_exit_status` y `command_exit_status`; `reason` es una
clasificación cerrada (`panic`, `fatal-signal`, `abort`, `returned`,
`cancelled`, `timeout` o `resource-limit`) y nunca conserva el texto del
panic.

## Analizador

`tondo dump analyze <file.tdump>` solo lee el archivo local y comprueba el
hash antes de producir un resumen. `--format human` es la vista por defecto;
`--format json` devuelve un objeto estable con la misma identidad, terminación,
secciones, conteos y limitaciones. El analizador no ejecuta código del dump,
no importa módulos y no accede a la red. Un input ausente, corrupto o con
formato inválido termina con status de uso `2`.

## Alcance y siguiente frontera

La ruta de señal async-signal-safe y el helper que materializa un dump tras un
SIGSEGV/abort son trabajo de `DIAG-NATIVE-001`. `DIAG-TEST-001` asocia estos
bytes con cada intento, retry y shard mediante el artifact store; no inventa
integración del backend nativo.

La evidencia machine-readable vive en
[`testing/diagnostic-dump.json`](../../testing/diagnostic-dump.json), y los
negativos en `scripts/diagnostic-dump-{check,test}.sh`.
