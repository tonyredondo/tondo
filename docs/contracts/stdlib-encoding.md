# Contrato de `std.encoding`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-ENCODING-001`. El registro machine-readable está en
[`testing/stdlib-encoding.json`](../../testing/stdlib-encoding.json) y este
documento se integra desde [`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
La evidencia de tests y fuzz del mismo owner está en
[`testing/stdlib-encoding-test.json`](../../testing/stdlib-encoding-test.json)
y [`docs/contracts/stdlib-encoding-test.md`](./stdlib-encoding-test.md).
El baseline reproducible de performance hosted está en
[`testing/stdlib-encoding-performance.json`](../../testing/stdlib-encoding-performance.json)
y [`docs/contracts/stdlib-encoding-performance.md`](./stdlib-encoding-performance.md).
El cierre fija la semántica portable de Base64 y hexadecimal; no afirma que
los adaptadores runtime de VM o nativo ya estén publicados.

`std.encoding` es el único owner de estos encodings binario-texto. Reutiliza
`std.bytes.Bytes` como valor binario y `std.io.Reader`/`std.io.Writer` para
streaming. Importar el módulo no abre I/O, no consulta el entorno y no concede
capabilities.

## API fuente única

Las siguientes declaraciones son la superficie pública canónica. Los métodos
de `Base64Options` y `HexOptions` son la única forma de seleccionar una policy;
no hay funciones paralelas con nombres `encodeAsync`, `decodeAsync`, `tryEncode`
ni una segunda API basada en `String`.

```tondo
pub enum Base64Alphabet {
    Standard
    UrlSafe
}

pub enum Base64Padding {
    Required
    Omitted
}

pub enum HexCase {
    Lower
    Upper
    Any
}

pub enum EncodingErrorKind {
    InvalidLimit
    InvalidCharacter
    InvalidLength
    InvalidPadding
    NonCanonical
    ResourceLimit
    Io(std.io.IoError)
    Closed
    NoProgress
}

pub type EncodingError = {
    kind: EncodingErrorKind
    offset: Int
}

pub type EncodingLimits = {
    maxInputBytes: Int
    maxOutputBytes: Int
}

pub type Base64Options = {
    alphabet: Base64Alphabet
    padding: Base64Padding
    limits: EncodingLimits
}

pub type HexOptions = {
    case: HexCase
    limits: EncodingLimits
}

pub type Base64Encoder
pub type Base64Decoder
pub type HexEncoder
pub type HexDecoder

pub fn EncodingLimits.defaults(): EncodingLimits
pub fn EncodingLimits.create(maxInputBytes: Int, maxOutputBytes: Int): EncodingLimits ! EncodingError

pub fn Base64Options.create(alphabet: Base64Alphabet, padding: Base64Padding, limits: EncodingLimits): Base64Options
pub fn Base64Options.standard(limits: EncodingLimits): Base64Options
pub fn Base64Options.urlSafe(limits: EncodingLimits): Base64Options
pub fn Base64Options.urlSafeUnpadded(limits: EncodingLimits): Base64Options
pub fn Base64Options.encode(self, input: Bytes): Bytes ! EncodingError
pub fn Base64Options.decode(self, input: Bytes): Bytes ! EncodingError
pub fn Base64Options.encodeTo(self, input: Bytes, var output: std.io.Writer): Unit ! EncodingError suspends
pub fn Base64Options.decodeFrom(self, var input: std.io.Reader): Bytes ! EncodingError suspends
pub fn Base64Options.encoder(self): Base64Encoder ! EncodingError
pub fn Base64Options.decoder(self): Base64Decoder ! EncodingError

pub fn HexOptions.create(case: HexCase, limits: EncodingLimits): HexOptions
pub fn HexOptions.lower(limits: EncodingLimits): HexOptions
pub fn HexOptions.upper(limits: EncodingLimits): HexOptions
pub fn HexOptions.anyCase(limits: EncodingLimits): HexOptions
pub fn HexOptions.encode(self, input: Bytes): Bytes ! EncodingError
pub fn HexOptions.decode(self, input: Bytes): Bytes ! EncodingError
pub fn HexOptions.encodeTo(self, input: Bytes, var output: std.io.Writer): Unit ! EncodingError suspends
pub fn HexOptions.decodeFrom(self, var input: std.io.Reader): Bytes ! EncodingError suspends
pub fn HexOptions.encoder(self): HexEncoder ! EncodingError
pub fn HexOptions.decoder(self): HexDecoder ! EncodingError

pub fn Base64Encoder.push(var self, chunk: Bytes): Bytes ! EncodingError
pub fn Base64Encoder.finish(var self): Bytes ! EncodingError
pub fn Base64Decoder.push(var self, chunk: Bytes): Bytes ! EncodingError
pub fn Base64Decoder.finish(var self): Bytes ! EncodingError

pub fn HexEncoder.push(var self, chunk: Bytes): Bytes ! EncodingError
pub fn HexEncoder.finish(var self): Bytes ! EncodingError
pub fn HexDecoder.push(var self, chunk: Bytes): Bytes ! EncodingError
pub fn HexDecoder.finish(var self): Bytes ! EncodingError
```

`EncodingLimits.defaults` devuelve los límites finitos del perfil del target;
no lee variables de entorno ni propiedades del host. `create` acepta límites
no negativos y representables. Un límite cero solo permite una transformación
vacía. Aunque una policy solicite un límite mayor, toda reserva continúa
sometida a `ResourceLimits.max_vm_heap_bytes`.

Las policies son valores inmutables, `Copy`, `Discard`, `Send` y `Share`. Sus
constructores nombrados no tienen combinaciones inválidas: `create` permite las
cuatro combinaciones de alfabeto y padding de Base64. `standard` selecciona el
alfabeto RFC 4648 y padding requerido; `urlSafe` selecciona el alfabeto URL-safe
RFC 4648 y padding requerido; `urlSafeUnpadded` selecciona el mismo alfabeto y
padding omitido. Para Base64 estándar sin padding se usa `create` de forma
explícita, no una policy ambiental.

`HexOptions.lower` y `upper` producen y aceptan exclusivamente la case
seleccionada. `anyCase` produce siempre hexadecimal lowercase canónico y acepta
cualquier combinación de dígitos ASCII en mayúsculas o minúsculas; esta
relajación es visible en el nombre de la policy. Ninguna policy acepta `0x`,
espacios, separadores, Unicode o caracteres fuera del alfabeto.

## Semántica de Base64

La policy `Standard` usa exactamente el alfabeto
`A-Z a-z 0-9 + /`; `UrlSafe` sustituye `+` y `/` por `-` y `_`. No se emiten
saltos de línea, whitespace, prefijos ni separadores. Padding `Required` emite
`=` solo en el último quantum y exige la longitud múltiplo de cuatro al
decodificar. Padding `Omitted` no emite `=` y rechaza cualquier `=` de entrada;
los restos de longitud dos o tres son válidos y un resto de longitud uno es
`InvalidLength`.

El encoder emite siempre la representación canónica: no hay bits de relleno
distintos de cero, alfabetos mezclados ni variantes MIME. El decoder aplica la
misma policy de forma estricta. Rechaza whitespace, un alfabeto alternativo,
padding en una posición intermedia, padding excesivo, bits de relleno no cero,
longitudes imposibles y datos después de un quantum terminado. Los rechazos de
spelling válido pero no canónico son `NonCanonical`; una estructura de padding
imposible es `InvalidPadding`.

La transformación materializada calcula el tamaño prospectivo antes de
reservar o publicar el resultado. Para `n` bytes de entrada, el tamaño Base64
con padding es `4 * ceil(n / 3)`; sin padding se eliminan uno o dos `=` según
el resto. Un overflow del cálculo o un tamaño superior a
`maxOutputBytes` produce `ResourceLimit` sin resultado parcial.

## Semántica hexadecimal

Cada byte se representa con exactamente dos dígitos ASCII. `Lower` emite
`0123456789abcdef` y `Upper` emite `0123456789ABCDEF`. `decode` exige una
longitud par y no acepta un nibble incompleto. La case estricta se comprueba
antes de publicar cada byte; una letra de la case opuesta produce
`NonCanonical`, mientras un carácter no hexadecimal produce
`InvalidCharacter`. `Any` es la única policy que admite ambas cases y mantiene
la salida lowercase.

El tamaño de salida hexadecimal es `2 * input.length`. El cálculo, la reserva y
la comprobación de `maxOutputBytes` son previos a modificar el destino. La
entrada vacía produce un `Bytes` vacío y un stream terminado correctamente.

## Streaming, estados y ownership

`encode`/`decode` son collectors de la misma máquina que usan los tipos
incrementales; no existen dos parsers ni dos tablas de errores. `encoder` y
`decoder` son handles afines, no `Copy`, `Share` ni `Clone`, y solo se pueden
transferir a un contexto que satisfaga `Send`. `push` consume un chunk lógico,
devuelve únicamente bytes ya completos y conserva como máximo dos bytes de
entrada Base64, un quantum pendiente del decoder o un nibble hexadecimal.

El corte de chunks es irrelevante: dividir en un byte, en cada frontera de
quantum o en chunks grandes produce exactamente los mismos bytes y errores que
una llamada materializada. Un chunk vacío no cambia el estado. `finish` es
obligatorio para validar el resto pendiente y es la única operación terminal de
éxito. Un decoder Base64 con padding requerido no puede terminar con un quantum
incompleto; uno sin padding solo puede terminar con restos de dos o tres
caracteres. Un decoder hexadecimal no puede terminar con un nibble pendiente.

Tras `finish` o cualquier error, el handle queda terminal y una operación
posterior devuelve `encoding.EncodingErrorKind.Closed`. Un error de input puede haber observado un prefijo,
pero no publica un `Bytes` parcial y no permite reintentar el mismo estado. Si
una policy de límite falla durante `push`, la operación es atómica para el
estado: no consume el chunk ni cambia el carry. Las APIs `encodeTo` y
`decodeFrom` conducen la misma máquina mediante `std.io.Writer`/`Reader`; una
llamada ordinaria se espera automáticamente y no existe una pareja async.

El writer se usa mediante `std.io.writeAll`; un writer que no progresa produce
`NoProgress`, y un error de I/O produce `Io(...)`. En ambos casos el encoder se
vuelve terminal antes de retornar. Los bytes ya aceptados por el writer no se
presentan como un resultado Tondo parcial. `decodeFrom` consume hasta EOF,
llama a `finish` y nunca devuelve un buffer incompleto.

`EncodingError.offset` es el número de bytes de entrada ya observados antes del
fallo; para errores de configuración, límite o estado es cero. El offset es
portable, no contiene una dirección del host y no se calcula con una segunda
pasada sobre la entrada.

## Rendimiento y portabilidad

La ruta escalar es el oráculo normativo. Puede existir una ruta SIMD o
multiversionada por target para bloques grandes, pero solo después de demostrar
igualdad byte a byte de output, errores, offsets, límites, estado terminal y
ownership frente a la ruta escalar. El dispatch depende únicamente del target
declarado y del tamaño del bloque; nunca de variables ambientales.

La ruta `push` no reserva por byte y mantiene un carry constante. La ruta a
`Writer` no materializa el documento completo; solo puede reservar buffers
acotados por chunk y por `EncodingLimits`. La operación materializada es un
collector de esta misma máquina. Los benchmarks posteriores medirán throughput,
tail latency, bytes copiados, allocations, tamaño de código y crossover SIMD,
separando input vacío, pequeño, grande, restos de quantum y errores.

`STD-ENCODING-PERF-001` cierra ahora el baseline de la VM hosted para 16
workloads materializados e incrementales. El probe mide la ruta scalar del
bridge con tres procesos independientes, tres warmups y nueve repeticiones
medidas, y conserva todas las muestras para calcular mediana/P95/P99. Registra
copias lógicas del bridge, identidades de valores host, memoria lógica del
registro y handles vivos; la memoria lógica no es RSS y excluye la sobrecarga
del allocator. El dispatch reportado es siempre `scalar-fixed-target`, con
clases `empty`, `quantum`, `small` y `large`.

El reporte no mide tamaño de código, crossover SIMD, ABI runtime nativo ni
lowering AOT. Por eso `native_aot` permanece `not-claimed`,
`simd` permanece `not-measured-no-optimized-route` y
`multiversion_dispatch` solo describe la dimensión declarada del target, sin
afirmar una ruta optimizada. La interoperabilidad VM/nativo y la igualdad
scalar/SIMD tienen una frontera explícita en `STD-ENCODING-CONF-001`.

## Conformance VM/native

`STD-ENCODING-CONF-001` cierra la corpus observable compartida en
[`testing/stdlib-encoding-conformance.json`](../../testing/stdlib-encoding-conformance.json)
y [`stdlib-encoding-conformance.md`](./stdlib-encoding-conformance.md). La
fixture hosted y la sonda nativa ejecutan seis case IDs: interoperabilidad de
Base64/hex, fragmentación de streaming, errores estrictos con offsets,
límites/terminalidad y la frontera de ruta scalar/SIMD. La sonda nativa usa
handles opacos `u64` y el mismo kernel scalar de
`crates/tondo-stdlib/src/encoding.rs`; no expone layout, punteros ni una API
FFI pública.

La comparación verifica bytes, policies, chunk boundaries, kind/offset de
errores, publicación atómica ante límites y cero handles vivos por caso. No
existe aún una ruta SIMD optimizada, por lo que el reporte conserva
`simd: not-measured-no-optimized-route`; `native_aot_lowering: not-claimed`
continúa sin cambios. `STD-ENCODING-DOC-001` cierra ahora la guía ejecutable;
el siguiente bloque del slice es `STD-YAML-IMPL-001`.

## Guía ejecutable de `std.encoding`

`STD-ENCODING-DOC-001` cierra la guía de uso de este owner sin crear una API
paralela. La ficha documental vive en el campo `documentation` de
[`testing/stdlib-encoding.json`](../../testing/stdlib-encoding.json); la fixture
[`tests/runtime/m11-std-encoding-doc-001.to`](../../tests/runtime/m11-std-encoding-doc-001.to)
ejecuta los mismos contratos en la VM hosted y termina con
`encoding-doc-ok`. La guía describe la semántica observada, no una publicación
del runtime nativo ni una release.

### Una forma por policy

Selecciona la policy una sola vez y conserva ese valor para todas las
operaciones. `Base64Options.standard`, `urlSafe` y `urlSafeUnpadded` hacen
visible el alfabeto y el padding; para cualquier otra combinación se usa
`Base64Options.create` de forma explícita. `HexOptions.lower`, `upper` y
`anyCase` hacen visible la policy de case. No hay autodetección, defaults
ambientales, `encodeAsync`, `tryEncode` ni una segunda API basada en `String`.

```tondo
let limits = encoding.EncodingLimits.defaults()
let policy = encoding.Base64Options.standard(limits)
let input = bytes.Bytes("fo")?
let encoded = policy.encode(input)?
assert(String(encoded)? == "Zm8=")
```

### Errores observables

Todas las operaciones devuelven `EncodingError` con `kind` y `offset`. Un
decoder estricto usa `InvalidCharacter`, `InvalidLength`, `InvalidPadding` o
`NonCanonical` según la regla que se viola; un límite usa `ResourceLimit` sin
publicar un resultado parcial. Después de `finish` o de cualquier error el
handle es terminal y la siguiente operación produce `encoding.EncodingErrorKind.Closed`. El offset es el
número de bytes observados antes del fallo, no una posición del host.

```tondo
match policy.decode(bytes.Bytes("Zh==")?) {
    ok(_) => assert(false)
    err(error) => match error.kind {
        encoding.EncodingErrorKind.NonCanonical => assert(error.offset == 0)
        _ => assert(false)
    }
}
```

### Costes y límites

La operación materializada es un collector de la máquina incremental y tiene
coste lineal en los bytes de entrada. Base64 produce `4 * ceil(n / 3)` bytes
con padding y hexadecimal produce `2 * n`; el cálculo prospectivo y los
límites `maxInputBytes`/`maxOutputBytes` se comprueban antes de publicar. Un
stream conserva solo el `bounded-carry` de un quantum (como máximo dos bytes de entrada
Base64, tres caracteres pendientes del decoder o un nibble hexadecimal), no
reserva por byte y no retiene un alias de la entrada. `encodeTo` escribe por
chunks mediante `std.io.Writer`; no materializa el documento completo.

El baseline reproducible de esos costes está en
[`stdlib-encoding-performance.md`](./stdlib-encoding-performance.md). Mide la
ruta scalar del bridge hosted, no RSS, SIMD, tamaño de código ni una ruta AOT;
por eso `simd: not-measured-no-optimized-route` y
`native_aot_lowering: not-claimed` siguen siendo parte del contrato.

### Ejemplos materializados

La forma materializada es la elección por defecto para payloads pequeños o
cuando el consumidor necesita un `Bytes` completo. El encoder y el decoder
comparten las mismas policies y errores:

```tondo
let limits = encoding.EncodingLimits.defaults()
let source = bytes.Bytes("fo")?
let standard = encoding.Base64Options.standard(limits)
let encoded = standard.encode(source)?
let decoded = standard.decode(encoded)?
assert(String(decoded)? == "fo")
let hex = encoding.HexOptions.upper(limits).encode(source)?
assert(String(hex)? == "666F")
```

La fixture identifica este caso como `materialized_examples` y además prueba
que `HexOptions.anyCase` acepta la case explícita sin cambiar la salida
canónica.

### Ejemplos streaming

Usa un handle affine cuando el input llega por fragmentos. `push` publica solo
quanta completos; `finish` valida el carry restante y cierra el handle. El
corte entre chunks no cambia el resultado:

```tondo
let limits = encoding.EncodingLimits.defaults()
var encoder = encoding.Base64Options.urlSafeUnpadded(limits).encoder()?
let first = encoder.push(bytes.Bytes("f")?)?
let second = encoder.push(bytes.Bytes("o")?)?
assert(first.length() == 0)
assert(second.length() == 0)
let tail = encoder.finish()?
assert(String(tail)? == "Zm8")
```

Para un destino `Writer`, declara la capacidad suspendible del helper y deja
que la llamada ordinaria se espere automáticamente; no existe una pareja
`encodeAsync`:

```tondo
fn write_example(): !(bytes.BytesError | bytes.Utf8Error | console.ConsoleError | encoding.EncodingError) suspends {
    let limits = encoding.EncodingLimits.defaults()
    let policy = encoding.Base64Options.standard(limits)
    let input = bytes.Bytes("fo")?
    var output = console.stdout()?
    policy.encodeTo(input, var output)?
}
```

### Verificación ejecutable

La guía se comprueba con
[`scripts/stdlib-encoding-doc-check.sh`](../../scripts/stdlib-encoding-doc-check.sh),
y sus mutaciones negativas con
[`scripts/stdlib-encoding-doc-test.sh`](../../scripts/stdlib-encoding-doc-test.sh).
La fixture cubre `policy_selection`, `materialized_examples`,
`streaming_examples`, `errors_and_limits`, `costs_and_ownership` y
`writer_example`; sus sidecars exigen el marcador final
`Zm8=encoding-doc-ok` y exit `0`.
Los contratos de performance y conformance permanecen separados y enlazados;
este cierre documental no cambia sus targets ni sus fronteras.

## Exclusiones deliberadas

Este contrato no incluye MIME Base64, saltos de línea configurables, whitespace
permisivo, Base32/Base16 alternativos, Base85, percent-encoding, URL escaping,
transcodificación `String`, cifrado, compresión, autodetección de formato,
fallback de locale, buffers ilimitados ni una capability de host. Esas
funcionalidades requerirían owners y policies explícitos; no se agregan como
booleanos a esta API.

La ruta escalar de
`crates/tondo-stdlib/src/encoding.rs` y el bridge de la VM hosted ya están
verificados por `STD-ENCODING-IMPL-001`: cubren las operaciones materializadas,
los adapters `Reader`/`Writer`, los handles afines y la traducción de errores
tipados en la superficie del compilador. La fixture
`tests/runtime/m11-std-encoding-impl-001.to` produce `Zm8=encoding-ok` con
salida y estado hash-bound en la evidencia de implementación.

Esta implementación es un cierre de la ruta hosted y del oráculo scalar, no una
afirmación de runtime nativo ni de lowering AOT genérico:
`native_aot_lowering: not-claimed`. Los bloques
`STD-ENCODING-TEST-001` ya cierra el modelo independiente, los vectores,
las fronteras de chunk, los límites, los errores byte-exactos y el fuzz
acotado sin promover una API pública ni un backend adicional.
`STD-ENCODING-PERF-001` cierra el baseline scalar hosted descrito arriba y
`STD-ENCODING-CONF-001` cierra la interoperabilidad VM/native descrita en la
sección anterior y `STD-ENCODING-DOC-001` cierra esta guía ejecutable. La
semántica sigue siendo una sola en la VM hosted, el kernel scalar y la lane
nativa privada; `native_aot_lowering: not-claimed` permanece explícito.
