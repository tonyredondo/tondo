# Contrato de `std.log`

**Estado:** `contract-locked` para STD-0.1B, cerrado por
`STD-LOG-001`. El registro machine-readable está en
[`testing/stdlib-log.json`](../../testing/stdlib-log.json) y la integración
normativa se enlaza desde
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija el shape y las garantías observables; no afirma que los
sinks, el bridge de host, la conformance o los benchmarks ya estén
implementados.

`std.log` separa un evento puro de su entrega. Construir un evento no consulta
el host ni cambia el control del programa. Un `Logger` solo existe cuando el
caller le entrega un sink explícito; el sink declara su capability, formato,
capacidad y política de backpressure. No hay logger global, configuración
ambiental, nivel leído de `environment`, ni una tarea detached escondida.

## 1. Alcance y objetivos

El contrato proporciona:

1. niveles ordenables y un filtro mínimo por logger;
2. eventos inmutables con target, mensaje, timestamp civil opcional y fields
   estructurados;
3. valores de field acotados, incluidos arrays/objects y una variante
   `Redacted` explícita;
4. formatos `Text` y `JsonLines` con canonicalidad machine-readable;
5. sinks explícitos para console y filesystem, y el protocolo `LogSink` para
   un sink de red o uno definido por el target;
6. backpressure visible (`Block`, `Reject` o `Drop`) y receipts que muestran
   si un evento fue aceptado o descartado;
7. concurrencia linealizable, cierre terminal, flush y cancelación cooperativa;
8. límites finitos, errores nominales, providers sellados para tests y una
   frontera scalar que un futuro fast path debe igualar.

El contrato no convierte logging en tracing, metrics, profiling, audit trail,
telemetry exporter o sistema de configuración. Correlación, sampling,
redaction heurística y rotation pertenecen a contratos posteriores o al
caller.

## 2. Eventos y niveles

Los niveles tienen orden total de severidad:

```tondo
pub enum LogLevel {
    Trace
    Debug
    Info
    Warn
    Error
}
```

`minimumLevel` acepta un evento cuando `event.level >= minimumLevel`; un evento
filtrado no entra en el sink, no consume capacidad y no cuenta como drop. No
existe `Fatal`: terminar el programa desde una API de logging ocultaría un
efecto de control. Un caller puede emitir `Error` y decidir explícitamente qué
hacer después.

`LogEvent` contiene exactamente un `LogLevel`, un `target` no vacío, un
`message` UTF-8, un conjunto ordenado de fields únicos y un
`UtcDateTime?`. `none` en `timestamp` es legítimo: `std.log` nunca llama a
`CivilClock.now()` por su cuenta. Si el caller necesita tiempo, lo obtiene de
`std.time` bajo la capability `civil-clock` y lo pasa al constructor. El valor
no contiene PID, task ID, dirección, path físico ni secreto del host.

`LogEvent` es inmutable, `Copy`, `Discard`, `Send` y `Share`. Sus copias son
lógicas y están sujetas a los mismos presupuestos de fields, profundidad y
bytes; no contienen aliases mutables.

## 3. Fields y valores

`Fields` es un mapa ordenado de claves UTF-8 a `LogValue`. Una clave es no
vacía, no contiene `NUL` ni caracteres de control y mide como máximo
`maxFieldKeyBytes`; comparar claves para detectar duplicados y para el formato
JSON usa sus bytes UTF-8, no locale ni normalización Unicode. Insertar una
clave existente produce `DuplicateField` y no cambia el builder.

```tondo
pub enum LogValue {
    Null
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float(Float64)
    Text(String)
    Bytes(Bytes)
    Array(Array[LogValue])
    Object(Fields)
    Redacted
}
```

Los floats deben ser finitos; NaN e infinitos producen `NonFiniteValue` antes
de publicar el valor. `Bytes` conserva todos sus bytes y no se interpreta como
texto. `Redacted` es una decisión del caller y no conserva el secreto: los
sinks escriben `[REDACTED]` en `Text` y el token JSON `{"$redacted":true}` en
`JsonLines`. No existe escaneo heurístico para adivinar contraseñas, tokens o
PII; si el programa copia un secreto a `Text`, `Bytes`, un field o un mensaje,
la responsabilidad es explícita.

`Fields.empty`, `Fields.put`, `Fields.get` y `Fields.count` no hacen I/O.
`Fields.put` comprueba el delta completo antes de mutar; un límite excedido no
deja un field parcial. El orden de inserción permanece observable en el evento
y en el sink `Text`. `JsonLines` ordena las claves por bytes UTF-8 para que el
record sea reproducible aunque el builder haya recibido fields en otro orden.

## 4. Formatos

`LogFormat` es cerrado:

```tondo
pub enum LogFormat {
    Text
    JsonLines
}
```

`Text` produce una línea UTF-8 por evento con el orden
`level target message fields`. Escapa `LF`, `CR`, tabulador, backslash,
comillas y caracteres de control; un mensaje nunca puede inyectar una segunda
línea. Cada field se imprime como `key=value`; arrays/objects usan la misma
codificación delimitada y `Bytes` usa `b64:<RFC4648-canonical>`.

`JsonLines` produce un objeto por línea del formato
`tondo-log-event-0.1/1`:

```json
{"schema":"tondo-log-event-0.1/1","level":"info","target":"http.server","message":"started","time":null,"fields":{}}
```

El orden de las propiedades raíz es `schema`, `level`, `target`, `message`,
`time`, `fields`. `time` es `null` o RFC 3339 UTC con hasta nueve dígitos de
fracción. `Bytes` se representa como `{"$bytes":"..."}` con Base64 RFC 4648
canónico; `Redacted` usa `{"$redacted":true}`. No hay floats no finitos,
claves duplicadas, whitespace adicional ni newline final opcional: el sink
escribe exactamente un `LF` después de cada record.

Los formatos no cambian la semántica de aceptación. Si un valor no cabe en el
límite o el sink no puede codificarlo, el evento no se publica parcialmente y
el caller recibe un `LogError`.

## 5. Sinks y backpressure

El protocolo estático de sinks es:

```tondo
pub trait LogSink: Send + Share {
    fn write(var self, event: LogEvent): LogReceipt ! LogError suspends
    fn flush(var self): Unit ! LogError suspends
    fn close(self): Unit ! LogError suspends
}
```

`ConsoleSink` escribe en `stdout` o `stderr` mediante la capability `console`.
`FileSink` escribe a un `Path` explícito mediante `filesystem`; no crea padres,
no consulta `HOME`, no cambia permisos ambientales y no rota archivos en este
contrato. Un provider de red implementa `LogSink` sobre un `std.io.Writer` o
un transporte de `std.net` bajo `network`; `std.log` no hace DNS, TLS ni
reconexión por debajo del protocolo.

```tondo
pub enum ConsoleStream { Stdout, Stderr }
pub enum FileMode { Append, Truncate }
pub enum Backpressure { Block, Reject, Drop }

pub type SinkOptions
pub type LogLimits
pub type LoggerOptions
pub type ConsoleSink
pub type FileSink
pub type Logger[S: LogSink]
```

`Block` espera hasta que el sink acepta el evento; la espera es el único punto
de suspensión y responde a cancelación. `Reject` devuelve
`LogError.Backpressure` sin consumir el evento. `Drop` devuelve
`LogReceipt.Dropped`; el descarte es observable y nunca se presenta como
aceptado. No existe una política `DropOldest`, porque destruir un evento ya
aceptado sin un ack explícito rompe el orden y la causalidad. Todo buffer tiene
capacidad finita; un sink no puede convertirse en una cola ilimitada por
configuración.

La superficie de construcción es:

```tondo
pub fn LogLimits.defaults(): LogLimits
pub fn LogLimits.create(maxEventBytes: Int, maxFields: Int, maxDepth: Int,
    maxFieldKeyBytes: Int, maxStringBytes: Int, maxQueueEntries: Int): LogLimits ! LogError
pub fn SinkOptions.create(format: LogFormat, backpressure: Backpressure,
    capacity: Int, limits: LogLimits): SinkOptions ! LogError
pub fn LoggerOptions.create(minimumLevel: LogLevel): LoggerOptions
pub fn ConsoleSink.create(stream: ConsoleStream, options: SinkOptions): ConsoleSink ! LogError
pub fn FileSink.create(path: Path, mode: FileMode, options: SinkOptions): FileSink ! LogError
pub fn Logger.create[S: LogSink](sink: S, options: LoggerOptions): Logger[S] ! LogError
```

`Logger` es el único owner del sink. `Logger.enabled` es una consulta pura;
`Logger.emit` entrega un `LogEvent` al sink solo después de aplicar el filtro.
`flush` espera hasta que todos los eventos aceptados antes de la llamada hayan
alcanzado el writer del sink. `close` es terminal: deja de admitir eventos,
drena lo aceptado, ejecuta `flush` y consume el logger. Un error de flush o
close sigue siendo visible; nunca se ignora para hacer que el programa parezca
haber registrado el evento.

```tondo
pub fn Logger.enabled(ref self, level: LogLevel): Bool
pub fn Logger.emit(ref self, event: LogEvent): LogReceipt ! LogError suspends
pub fn Logger.flush(ref self): Unit ! LogError suspends
pub fn Logger.close(self): Unit ! LogError suspends
```

`LogReceipt` tiene solo `Accepted` y `Dropped`. Un logger y un sink son
shareable y sendable cuando el sink lo es; las llamadas concurrentes se
linearizan por sink. El orden observable es el orden de commit, no el orden de
creación de tasks ni el orden de un scheduler. `close` espera operaciones en
vuelo, y las llamadas que llegan después reciben `Closed`. No hay locks,
workers, queues ni handles host globales fuera de la identidad del logger.

## 6. Errores, límites y capabilities

`LogError` es nominal y portable:

```text
InvalidTarget
InvalidFieldKey
DuplicateField
InvalidLimit
ResourceLimit
NonFiniteValue
Backpressure
Closed
Cancelled
CapabilityMissing
UnsupportedFormat
Io
Host
```

Los defaults normativos son `maxEventBytes = 1 MiB`, `maxFields = 64`,
`maxDepth = 16`, `maxFieldKeyBytes = 128`, `maxStringBytes = 64 KiB` y
`maxQueueEntries = 1024`. `capacity` debe ser positiva y no superar el
presupuesto del target; `0` no significa una cola ilimitada. Los límites se
comprueban antes de reservar, formatear o llamar al host. Un error nunca
publica un prefijo, cambia los fields parcialmente o deja un descriptor de
sink abierto.

El core de eventos, values, fields, formats, filters y el protocolo de sink no
requiere capability. `ConsoleSink` requiere `console`, `FileSink` requiere
`filesystem` y un provider de red requiere `network`. Importar `std.log` no
selecciona ninguna de ellas; si falta una capability, el constructor no forma
parte de la interfaz y el compilador produce `E1008`. No hay fallback a
`stderr`, archivo temporal, environment ni red.

El sink de conformance es sellado y determinista. Puede inyectar errores,
cancelación, full queue y short writes sin cambiar las firmas de producción.
No existe un provider de test visible para código ordinario.

## 7. Seguridad, ownership y rendimiento

Events, fields y values no contienen referencias mutables. `Logger` no es
`Copy` ni `Clone`; debe cerrarse explícitamente y puede transferirse a una
task o thread que cumpla `Send`. Un `LogSink` es el único owner de su writer y
no puede ser usado después de `close`. Un error antes de la linearización
conserva el event lógico del caller; no se descarta un payload por un fallo de
backpressure, cancelación o límite.

El formatter scalar es el oracle: recorrido iterativo de fields, escapes
tabulados y Base64 canónico sin usar la pila recursiva del host. Arrays y
objects se recorren mediante frames explícitos y consumen `maxDepth`; ningún
input puede convertir la profundidad del log en stack overflow. SIMD, tablas
de escape y buffers especializados están permitidos solo tras equivalencia
byte a byte, errores exactos y mismo presupuesto de allocations.

El coste de un evento filtrado se limita a la comparación de nivel. El coste de
uno aceptado incluye validación, copia lógica y formato; los reports deben
separar `disabled`, `accepted`, `dropped`, `flush-tail`, allocations y bytes
de queue. No se prometen latencias de sink host antes de `STD-LOG-PERF-001`.

## 8. Matriz de evidencia y exclusiones

El owner debe observar al menos:

| Grupo | Observables obligatorios |
|---|---|
| `event-model` | niveles, target/mensaje, timestamp opcional, Copy/Send/Share |
| `fields` | inserción ordenada, duplicate, claves/UTF-8, arrays/objects, redacted |
| `formats` | escape Text, schema JsonLines, bytes, finite floats, newline único |
| `filter` | threshold, `enabled`, evento filtrado sin sink ni coste de queue |
| `backpressure` | Block, Reject, Drop, receipts, límites y cancelación |
| `concurrency` | linearización, orden commit, close con operaciones en vuelo |
| `sinks` | console/filesystem/network capabilities, short write, flush y close |
| `failures` | host, capability, format, resource, no partial publication |
| `privacy` | redacted explícito, ausencia de heurística, no metadata ambiental |
| `vm-native-parity` | bytes, errores, ordering, limits y policies idénticas |

Los corpus mínimos cubren niveles/formats, keys y values inválidos, UTF-8,
depth/bytes/queue limits, concurrent commits, cancellation, sink failures,
short writes, capability denial, close races y equivalencia VM/native. La
conformance compara receipts, records y errores, no timings ni texto del host.

Se excluyen `Fatal`, logger global, environment-configured levels, sampling,
implicit batching, unbounded queues, `DropOldest`, hidden threads, retries,
rotation, DNS/TLS/reconnect, metrics/tracing exporters, callback-based APIs,
heuristic secret redaction, `logAsync` y `std.log.select`.

El contrato machine-readable y sus negativos ejecutables son
[`testing/stdlib-log.json`](../../testing/stdlib-log.json),
[`scripts/stdlib-log-check.sh`](../../scripts/stdlib-log-check.sh) y
[`scripts/stdlib-log-test.sh`](../../scripts/stdlib-log-test.sh). El diseño B0
queda cerrado por `STD-LOG-001`; la implementación, bridges de host, tests,
fuzzing, rendimiento, conformance y documentación de uso quedan pendientes de
`STD-LOG-IMPL-001`, `STD-LOG-HOST-001`, `STD-LOG-TEST-001`,
`STD-LOG-PERF-001`, `STD-LOG-CONF-001` y `STD-LOG-DOC-001`.
