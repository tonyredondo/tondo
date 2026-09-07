# Contrato de `std.yaml`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-YAML-001`. La ruta scalar y el bridge VM hosted de la implementación
quedan verificados por `STD-YAML-IMPL-001`; el modelo independiente, las
regresiones y el fuzz acotado quedan cerrados por `STD-YAML-TEST-001`. La
promoción de la API pública y el backend nativo permanecen explícitamente fuera
de este cierre.

`std.yaml` ofrece un lector y escritor YAML 1.2 deliberadamente seguro. No
intenta implementar todos los dialectos históricos de YAML: fija un subset
portable, determinista y acotado que sirve para configuración y documentos de
datos sin ejecutar código, consultar el entorno ni construir grafos sin límite.
El registro machine-readable es
[`testing/stdlib-yaml.json`](../../testing/stdlib-yaml.json), el contrato de
tests es [`testing/stdlib-yaml-test.json`](../../testing/stdlib-yaml-test.json)
y el contrato de rendimiento hosted es
[`testing/stdlib-yaml-performance.json`](../../testing/stdlib-yaml-performance.json)
y su documento [`docs/contracts/stdlib-yaml-performance.md`](./stdlib-yaml-performance.md).
La conformance VM/native target-qualified está cerrada por
[`testing/stdlib-yaml-conformance.json`](../../testing/stdlib-yaml-conformance.json) y
[`docs/contracts/stdlib-yaml-conformance.md`](./stdlib-yaml-conformance.md). La guía ejecutable de uso queda cerrada por
`STD-YAML-DOC-001`; su ficha, fixture y runners están en
[`testing/stdlib-yaml.json`](../../testing/stdlib-yaml.json),
`tests/runtime/m11-std-yaml-doc-001.to`,
[`scripts/stdlib-yaml-doc-check.sh`](../../scripts/stdlib-yaml-doc-check.sh) y
[`scripts/stdlib-yaml-doc-test.sh`](../../scripts/stdlib-yaml-doc-test.sh). El siguiente
bloque del owner es `STD-TOML-IMPL-001`.
Este documento se integra desde
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).

## Principios del owner

- El input es UTF-8; el parser no consulta locale, `TZ`, variables de entorno,
  filesystem, red, process ni ningún provider del host.
- El schema implícito es YAML 1.2 Core, no YAML 1.1. `yes`, `no`, `on`, `off`,
  fechas y timestamps permanecen strings salvo un tag explícito admitido.
- Las APIs tipadas usan `serialization.Encode[Yaml]` y
  `serialization.Decode[Yaml]`; las APIs dinámicas usan `YamlValue`, no
  `Any`, reflection de valores ni un árbol compartido con JSON/MessagePack.
- Un alias se resuelve como una copia lógica acotada. No se preserva identidad
  de anchors en `YamlValue`, no se admiten ciclos y no existe merge key `<<`.
- Los límites son finitos y forman parte de `YamlOptions`. No hay un modo
  ilimitado ni defaults dependientes del host.
- La máquina incremental y las operaciones materializadas comparten parser,
  validador, límites y errores. El parser usa frames explícitos y no la pila
  recursiva del host.
- No hay API async duplicada. Los readers/writers de `std.io` usan la
  suspensión implícita única de Tondo; ninguna operación de YAML es
  `selectable`.

## API fuente única

Estas declaraciones son la superficie pública canónica. `parse`/`decode` y
`encode` son collectors sobre la misma máquina que usan `YamlReader` y
`YamlWriter`; `parseAll`/`decodeAll` son las formas explícitas para streams con
múltiples documentos.

```tondo
pub type Yaml

pub enum YamlValue {
    Null
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float(Float64)
    Text(String)
    Bytes(Bytes)
    Array(Array[YamlValue])
    Object(Map[String, YamlValue])
}

pub type YamlValueView

pub enum YamlTag {
    Null
    Bool
    Int
    Float
    Str
    Binary
    Seq
    Map
}

pub enum YamlScalar {
    Null
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float(Float64)
    Text(String)
    Bytes(Bytes)
}

pub enum YamlEvent {
    StreamStart
    DocumentStart
    DocumentEnd
    Scalar(YamlScalar)
    SequenceStart(String?)
    SequenceEnd
    MappingStart(String?)
    MappingKey
    MappingEnd
    Anchor(String)
    Alias(String)
    Tag(YamlTag)
    StreamEnd
}

pub type YamlLimits = {
    maxInputBytes: Int
    maxDocuments: Int
    maxDepth: Int
    maxNodes: Int
    maxExpandedNodes: Int
    maxAliases: Int
    maxScalarBytes: Int
    maxCollectionEntries: Int
    maxAnchorNameBytes: Int
}

pub type YamlOptions = {
    limits: YamlLimits
}

pub enum YamlPathSegment {
    Key(String)
    Index(Int)
}

pub enum YamlErrorKind {
    InvalidLimit
    InvalidUtf8
    InvalidDirective
    InvalidDocument
    InvalidIndentation
    InvalidScalar
    InvalidEscape
    InvalidTag
    InvalidAnchor
    UndefinedAlias
    AliasCycle
    AliasLimit
    MergeKeyForbidden
    DuplicateKey
    NonStringKey
    NumberOutOfRange
    NonFiniteNumber
    InvalidBinary
    DepthLimit
    NodeLimit
    ExpandedNodeLimit
    ScalarLimit
    CollectionLimit
    DocumentLimit
    TypeMismatch
    MissingField
    UnknownField
    UnexpectedEvent
    TrailingDocument
    Io(std.io.IoError)
    Closed
    NoProgress
}

pub type YamlError = {
    kind: YamlErrorKind
    offset: Int
    line: Int
    column: Int
    path: Array[YamlPathSegment]
}

pub type YamlReader
pub type YamlWriter

pub fn YamlLimits.defaults(): YamlLimits
pub fn YamlLimits.create(
    maxInputBytes: Int,
    maxDocuments: Int,
    maxDepth: Int,
    maxNodes: Int,
    maxExpandedNodes: Int,
    maxAliases: Int,
    maxScalarBytes: Int,
    maxCollectionEntries: Int,
    maxAnchorNameBytes: Int,
): YamlLimits ! YamlError

pub fn YamlOptions.defaults(): YamlOptions
pub fn YamlOptions.create(limits: YamlLimits): YamlOptions

pub fn parse(input: Bytes, options: YamlOptions): YamlValue ! YamlError
pub fn parseAll(input: Bytes, options: YamlOptions): Array[YamlValue] ! YamlError
pub fn parseView(input: Bytes, options: YamlOptions): YamlValueView ! YamlError
pub fn validate(input: Bytes, options: YamlOptions): Unit ! YamlError

pub fn decode[T: Decode[Yaml]](input: Bytes, options: YamlOptions): T ! YamlError
pub fn decodeAll[T: Decode[Yaml]](input: Bytes, options: YamlOptions): Array[T] ! YamlError
pub fn encode(value: YamlValue, options: YamlOptions): Bytes ! YamlError
pub fn encode[T: Encode[Yaml]](value: T, options: YamlOptions): Bytes ! YamlError
pub fn encodeCanonical(value: YamlValue, limits: YamlLimits): Bytes ! YamlError

pub fn YamlReader.fromBytes(input: Bytes, options: YamlOptions): YamlReader ! YamlError
pub fn YamlReader.fromReader(var input: std.io.Reader, options: YamlOptions): YamlReader ! YamlError suspends
pub fn YamlReader.next(var self): YamlEvent? ! YamlError
pub fn YamlReader.own(var self, event: YamlEvent): YamlEvent ! YamlError
pub fn YamlReader.finish(var self): Unit ! YamlError
pub fn YamlWriter.toWriter(var output: std.io.Writer, options: YamlOptions): YamlWriter ! YamlError suspends
pub fn YamlWriter.write(var self, event: YamlEvent): Unit ! YamlError suspends
pub fn YamlWriter.finish(var self): Unit ! YamlError suspends
```

`YamlOptions` es inmutable, `Copy`, `Discard`, `Send` y `Share`. Los
constructores no contienen switches de compatibilidad: el subset seguro es una
decisión del owner, no una policy ambiental. `YamlLimits.create` rechaza
valores negativos, cero cuando impediría representar incluso un documento
vacío y cualquier combinación donde `maxExpandedNodes < maxNodes` no pueda
contener la expansión de aliases. `defaults` devuelve los límites finitos del
perfil del target, pero nunca lee una variable de entorno.

## Subset de sintaxis y schema

El parser acepta documentos YAML 1.2 con UTF-8 válido, comentarios, marcadores
`---`/`...`, mapas y secuencias en estilo block o flow, strings plain/single
quoted/double quoted y scalars block `|`/`>` con chomping explícito. La
indentación usa espacios; un tab en la indentación, una mezcla ambigua de
indentaciones o una dedent que no coincide con un frame produce
`InvalidIndentation`.

Un stream puede contener varios documentos. `parse`/`decode`/`validate` exigen
exactamente un documento; `parseAll`/`decodeAll` procesan el stream completo y
aplican `maxDocuments`. Un documento vacío se representa como `Null`. El
marcador `...` termina el documento actual y no permite contenido posterior
salvo comentarios, whitespace o el siguiente `---`.

La resolución implícita sigue el schema Core de YAML 1.2:

- `null` y `~` son null; una entrada de mapping sin valor también es null;
- solo `true` y `false` (en cualquier case ASCII) son booleanos;
- enteros decimales y los prefijos explícitos `0b`, `0o` y `0x` producen
  `Int64`/`UInt64` según signo y rango; no hay sexagesimal ni octal implícito;
- floats decimales y científicos deben ser finitos; `.nan`, `.inf` y sus
  variantes se rechazan con `NonFiniteNumber`;
- fechas, timestamps, `yes`, `no`, `on`, `off` y cualquier otro spelling son
  `Text` si no llevan un tag permitido; y
- un scalar que no encaja en una regla anterior es `Text` UTF-8.

Los tags admitidos son únicamente `!!null`, `!!bool`, `!!int`, `!!float`,
`!!str`, `!!binary`, `!!seq` y `!!map`, junto con su forma completa
`tag:yaml.org,2002:*`. El tag debe ser compatible con el nodo y no puede
convertir un valor fuera de los límites publicados. `!!binary` usa Base64
standard RFC 4648 con padding requerido y la misma validación de
`std.encoding`; los saltos de línea propios del scalar block se pliegan antes
de decodificar, pero no se aceptan alfabetos URL-safe ni bits de relleno no
cero.

Se rechazan directives `%TAG`, cualquier versión distinta de YAML 1.2,
custom/local tags (`!app`, `!<uri>`), `!!timestamp`, `!!set`, `!!omap`,
`!!pairs`, `!!python/*`, `!!js/*`, `!!binary` malformado, y cualquier forma de
ejecución o include. El mapping dinámico solo admite claves textuales. La key
`<<` no tiene significado especial y produce `MergeKeyForbidden`; para fusionar
records el programa debe hacerlo explícitamente después del parseo.

## Anchors y aliases

Un anchor `&name` se registra dentro del documento actual y un alias `*name`
solo puede referirse a un anchor ya registrado. Los nombres son ASCII,
empiezan por letra o `_`, continúan con letras, dígitos, `_` o `-` y respetan
`maxAnchorNameBytes`. No se permite redefinir un nombre.

`parse` y `parseView` expanden cada alias como copia lógica del nodo anclado. No
se conserva identidad de objetos, no se comparte memoria mutable y no se
permite una referencia recursiva; un ciclo produce `AliasCycle`. Cada alias
consume una unidad de `maxAliases` y cada nodo creado por su expansión consume
`YamlLimits.maxExpandedNodes`. Por tanto, un documento tipo billion-laughs falla con
`AliasLimit` o `ExpandedNodeLimit` antes de reservar una estructura
exponencial. Un alias no puede cruzar una frontera `DocumentEnd`.

La ruta de eventos conserva `Anchor` y `Alias` para herramientas streaming. El
reader valida su alcance y el writer exige que todo `Alias` tenga un anchor
previo compatible. El encoder de valores no emite anchors: repite los valores
de forma determinista y evita que la salida dependa del layout interno.

## Modelo dinámico y ruta tipada

`YamlValue.Object` conserva orden de inserción y exige keys `String` únicas.
`YamlValue.Bytes` solo procede de `!!binary`; el texto Base64 original no se
conserva en el modelo semántico. `YamlValueView` es prestado e inmutable: sus
scalars y colecciones dejan de ser válidos al avanzar el reader o finalizar la
operación que lo produjo. `own` y el collector materializado son las únicas
formas de obtener almacenamiento estable.

La ruta tipada usa `Encode[Yaml]`/`Decode[Yaml]` sin construir un
`YamlValue`. El derive visita fields en orden de declaración al codificar y
acepta cualquier orden al decodificar. Fields desconocidos, duplicados,
ausentes, tags incompatibles y tipos no representables devuelven el error con
path estructural; un `Option[T]` ausente se reconstruye como `none`. Los
attributes comunes `@name` y `@ignore` mantienen su semántica de
`std.serialization`; no existe un conjunto paralelo de annotations YAML en
0.1. Un field `Bytes` requiere `!!binary` y no se convierte implícitamente a
texto.

La ruta tipada no usa `std.reflect`, lookup por string ni dispatch runtime. Un
tipo puede implementar también `Encode[Json]`, `Encode[MessagePack]` o
`Encode[Protobuf]`, pero cada codec conserva su propio wire model.

## Canonicalidad y encoding

`encode` conserva el orden declarativo de records y el orden de inserción de
`YamlValue.Object`. Emite un único documento, dos espacios por nivel, UTF-8,
saltos LF, sin comentarios, directives, anchors ni aliases. Elige quoting
solo cuando el spelling plain cambiaría el tipo al volver a parsear; nunca
emite `yes`, `no`, timestamps implícitos ni números no finitos.

`encodeCanonical` es una política adicional de bytes reproducibles: ordena las
keys de cada object por sus bytes UTF-8, normaliza los scalars al spelling
canónico del schema Core y usa block scalars solo cuando su forma es inequívoca.
Esta es la canonicalización de Tondo, no una afirmación de que YAML tenga una
canonicalización universal interoperable con todos los parsers externos.

Antes de publicar bytes se comprueba `maxInputBytes`/`maxScalarBytes`, el
número de nodos y la salida acumulada frente a los límites de `YamlLimits` y
`ResourceLimits.max_vm_heap_bytes`. Un overflow o exceso devuelve
`NodeLimit`, `ScalarLimit`, `CollectionLimit` o `ResourceLimit` sin resultado
parcial.

## Streaming, estados y ownership

`YamlReader.fromReader` y `YamlWriter.toWriter` son las únicas entradas que
adaptan I/O suspendible; `fromBytes` permanece puro. `YamlReader.next` devuelve
`none` exactamente una vez después de `StreamEnd`; `finish` comprueba que el
stream no quedó truncado ni tiene un evento pendiente. `YamlReader` y
`YamlWriter` son handles afines, no `Copy`, `Share` ni `Clone`;
pueden transferirse a un task/thread solo cuando satisfacen `Send`. El reader
mantiene como máximo los frames necesarios para `maxDepth`, el anchor table
acotado y el scalar actual; nunca materializa el stream completo por efecto de
`next`. `YamlWriter` valida balance de streams, documents, sequences/maps,
keys, tags y anchors antes de escribir el siguiente evento.

El corte de chunks es irrelevante: dividir UTF-8, escapes, indentación,
anchors, aliases, scalars block o un número entre dos chunks produce los mismos
eventos, bytes y errores que un input contiguo. Un chunk vacío no cambia el
estado. `finish` es obligatorio y terminal; tras `finish` o cualquier error,
una operación posterior devuelve `Closed`. Un error de input no publica un
árbol parcial. Un límite durante `push`/`write` es atómico para el estado y no
consume el evento que no puede aceptar.

`YamlWriter.write` usa `std.io.writeAll`. Un writer sin progreso produce
`NoProgress`, un error del writer se envuelve en `Io` y el handle se vuelve
terminal antes de retornar. `YamlReader` devuelve EOF como `none` únicamente
después de `StreamEnd`; truncar un documento o cerrar un scalar block produce
un error estructural, no una terminación normal.

`YamlError.offset` es el byte UTF-8 de entrada observado antes del fallo;
`line`/`column` son 1-based y se refieren al scalar o delimiter que lo causó.
`path` contiene solo `Key`/`Index` semánticamente estables y se actualiza al
entrar en una colección. Errores de configuración o estado tienen offset cero
y path vacío.

## Seguridad, rendimiento y portabilidad

La ruta escalar es el oráculo normativo. Puede existir una ruta SIMD o
multiversionada para búsqueda de delimiters, validación UTF-8, indentación y
copia de scalars, pero solo tras demostrar igualdad de eventos, valores,
errores, offsets, paths, límites, terminalidad y ownership frente a la ruta
escalar. El dispatch depende únicamente del target declarado y del tamaño del
chunk.

La implementación debe usar worklists/frames explícitos para aliases,
indentación y collections. No puede resolver aliases mediante una expansión
recursiva sobre la pila del host ni mantener una tabla global entre documentos.
Los presupuestos de rendimiento medirán throughput, tail latency, allocations,
bytes copiados, profundidad, aliases y coste de rechazo adversarial.
`STD-YAML-PERF-001` cierra ahora el baseline scalar hosted con esas dimensiones;
esto no promociona una ruta nativa, SIMD ni lowering AOT.

## Estado de implementación

`STD-YAML-IMPL-001` cierra la ruta hosted verificable del draft/0.1. El módulo
[`crates/tondo-stdlib/src/yaml.rs`](../../crates/tondo-stdlib/src/yaml.rs)
implementa el parser YAML 1.2 Core, la validación de tags/aliases y límites, el
modelo dinámico, la conversión tipada sobre los eventos comunes de
`std.serialization` y la codificación normal/canónica. El compilador registra
los nominales, intrinsics, firmas y efectos de las 21 operaciones; el host y la
VM materializan esos handles y sus errores con ubicación y path.

La evidencia de este bloque cubre la ruta hosted buffered y del oráculo scalar:
`parse`/`parseAll`/`parseView`, `decode`/`decodeAll`, `encode` y
`encodeCanonical`, además del ciclo de vida de `YamlReader` y `YamlWriter`.
En esta implementación hosted los adapters de reader/writer son un bridge
buffered: `fromReader` colecciona el input antes de producir eventos y el
writer retiene eventos hasta `finish`; esto modela y prueba el contrato de
estados, pero no reclama todavía un runtime nativo de streaming ni lowering
AOT. El registro mantiene `native_aot_lowering: not-claimed` y
`public_api_promoted: false`.

La fixture ejecutable y el informe reproducible de implementación son
`tests/runtime/m11-std-yaml-impl-001.to` y
`target/reliability/evidence/stdlib-yaml-implementation.json`, generados por
los checkers y runner del bloque. El contrato de pruebas y su documento son
[`testing/stdlib-yaml-test.json`](../../testing/stdlib-yaml-test.json) y
[`docs/contracts/stdlib-yaml-test.md`](./stdlib-yaml-test.md); sus pruebas
independientes y regresiones hosted cierran el corpus acotado, los límites,
la fragmentación y el fuzz determinista. `STD-YAML-PERF-001` queda cerrado por
el baseline scalar hosted de 13 workloads y 27 muestras por workload, con
throughput, tail latency, allocations, bytes copiados, memoria lógica,
profundidad, aliases, expansión, rechazo adversarial y cleanup de handles.
La conformance queda cerrada por `STD-YAML-CONF-001`: el fixture hosted y el
probe nativo de proceso separado comparan el mismo corpus de seis casos, con
rutas dinámica/tipada, interoperabilidad Core, streaming de un byte, errores con
path/ubicación, límites y lifecycle. La prueba nativa reutiliza el scalar stdlib;
no promociona ABI YAML nativo, SIMD ni lowering AOT (`native_aot: not-claimed`).
La guía de uso queda cerrada por `STD-YAML-DOC-001`, con la fixture
`tests/runtime/m11-std-yaml-doc-001.to`, sus sidecars y los runners
`scripts/stdlib-yaml-doc-check.sh` / `scripts/stdlib-yaml-doc-test.sh`.
El cierre documental mantiene explícitas las fronteras de ejecución del
writer suspendible y no promociona runtime nativo público, SIMD ni lowering
AOT. El siguiente bloque del owner es `STD-TOML-IMPL-001`.

## Exclusiones deliberadas

Este contrato no incluye YAML 1.1, custom tags, directives de `%TAG`,
timestamps implícitos, merge keys, mappings con keys no textuales, `!!set`,
`!!omap`, `!!pairs`, código Python/JavaScript, includes, referencias externas,
anchors cíclicos, documentos ilimitados, comentarios preservados en el árbol,
locale, environment interpolation, schema discovery, RPC ni una API async o
`selectable` paralela.

La documentación de uso queda cerrada por `STD-YAML-DOC-001`; la siguiente
implementación del roadmap es `STD-TOML-IMPL-001`.

## Guía ejecutable de `std.yaml`

`STD-YAML-DOC-001` cierra la guía de uso de este owner sin introducir una
segunda API. La ficha documental vive en el campo `documentation` de
[`testing/stdlib-yaml.json`](../../testing/stdlib-yaml.json); la fixture
[`tests/runtime/m11-std-yaml-doc-001.to`](../../tests/runtime/m11-std-yaml-doc-001.to)
ejecuta las rutas materializadas y de reader verificadas por la VM hosted y
termina con `yaml-doc-ok`. Los seis ejemplos de la ficha son decisiones de
uso observables: `safe-subset-and-policies`, `materialized-and-typed`,
`aliases-and-limits`, `streaming-events`, `errors-and-security` y
`costs-and-ownership`.

La API pública conserva la firma suspendible de `YamlWriter.toWriter` y sus
reglas de lifecycle. Sin embargo, la ruta genérica `tondo-cli run` todavía
devuelve `unsupported VM host call` para esa llamada porque el dispatcher
async de `BootstrapHost` aún no registra el writer YAML. La cobertura directa
de `BootstrapHost::invoke` no convierte esa ruta en ejecución CLI; por eso
`writer-boundary: static-contract-only-until-async-dispatch` queda declarado
de forma explícita y esta fixture no lo reclama como ejemplo ejecutable.

### Subset seguro y policies

Usa el subset YAML 1.2 Core fijado por el owner: `true`/`false` son booleanos,
`yes`/`no` permanecen texto, los tags son solo los ocho tipos Core admitidos y
no hay includes, lookup ambiental ni ejecución. Selecciona `YamlOptions` una
sola vez; `defaults` usa límites finitos del target y `YamlOptions.create`
permite hacerlos visibles en el llamador. No hay autodetección ni una policy
paralela para YAML 1.1.

```tondo
let options = yaml.YamlOptions.defaults()
let source = bytes.Bytes("name: Tondo\nactive: true\nlegacy: yes\n")?
let value = yaml.parse(source, options)?
assert(String(yaml.encode(value, options)?)? == "name: Tondo\nactive: true\nlegacy: yes\n")
```

### Límites y costes

`YamlLimits` hace explícitos `maxInputBytes`, `maxDocuments`, `maxDepth`,
`maxNodes`, `YamlLimits.maxExpandedNodes`, `maxAliases`,
`maxScalarBytes`, `maxCollectionEntries` y `maxAnchorNameBytes`. Todos son
finitos y se validan antes de publicar un valor. El parser usa frames/worklists
explícitos: el coste normal es lineal en bytes y nodos observados, mientras que
cada alias consume presupuesto de alias y de expansión. Un límite produce un
error atómico (`NodeLimit`, `ExpandedNodeLimit`, `ScalarLimit` o
`CollectionLimit`) sin árbol parcial.

`encode` y `decode` son collectors de la misma máquina que el reader. Usa
`encodeCanonical` cuando necesites bytes reproducibles; ordena keys por UTF-8
y no preserva anchors. `parseView` presta una vista sin copiar y su lifetime
termina al avanzar el reader o al retornar la operación; `own` o el modelo
`YamlValue` son las formas de conservar datos.

### Errores y ownership

Cada fallo devuelve `YamlError` con `kind`, offset UTF-8, línea/columna
1-based y `path` de keys/índices. `AliasCycle`, `InvalidTag`, `InvalidBinary` y
los límites se rechazan antes de exponer un resultado parcial. Los handles
`YamlReader` y `YamlWriter` son afines: no son `Copy`, `Share` ni `Clone`,
pueden enviarse solo cuando satisfacen `Send` y quedan terminales después de
`finish` o de cualquier error; una operación posterior devuelve
`YamlErrorKind.Closed`. El input no se retiene como alias mutable.

### Ejemplos materializados

Para payloads pequeños o cuando el consumidor necesita un valor completo,
usa `parse`/`encode`; para tipos conocidos, `decode[T]`/`encode[T]` aplican el
protocolo `std.serialization` sin construir un árbol dinámico adicional.
`encodeCanonical` ofrece una salida estable para firmas o snapshots:

```tondo
let limits = yaml.YamlLimits.defaults()
let options = yaml.YamlOptions.create(limits)
let value = yaml.parse(bytes.Bytes("name: Tondo\ncount: 7\n")?, options)?
let typed = yaml.decode[Array[Int]](bytes.Bytes("- 7\n- 9\n")?, options)?
assert(typed == [7, 9])
assert(String(yaml.encodeCanonical(value, limits)?)? == "count: 7\nname: Tondo\n")
```

La fixture identifica esta familia como `materialized_typed` y comprueba que
la salida vuelve a parsear con el mismo schema Core.

### Ejemplos streaming

`parseAll` procesa varios documentos y aplica `maxDocuments`. Para control
fino, `YamlReader.fromBytes` y `next` exponen los eventos; `none` aparece una
sola vez tras `StreamEnd` y `finish` cierra el handle. El corte en fragmentos
no cambia eventos, valores ni errores. La fixture `streaming_events` verifica
dos documentos y los dieciséis eventos del stream.

Los adaptadores `YamlReader.fromReader` y `YamlWriter.toWriter` siguen siendo
las únicas entradas que suspenden. El reader tiene contrato ejecutable en esta
fixture; el writer conserva el contrato estático y la cobertura de host
directo descrita arriba hasta que exista su registro en el dispatcher async.

### Verificación ejecutable

La guía se comprueba con
[`scripts/stdlib-yaml-doc-check.sh`](../../scripts/stdlib-yaml-doc-check.sh),
y sus mutaciones negativas con
[`scripts/stdlib-yaml-doc-test.sh`](../../scripts/stdlib-yaml-doc-test.sh).
El checker ejecuta la fixture con
`cargo run -q -p tondo-cli --locked -- run`, compara exactamente el sidecar
`yaml-doc-ok` y exige exit `0`. La evidencia de rendimiento y conformance
permanece separada en
[`stdlib-yaml-performance.md`](./stdlib-yaml-performance.md) y
[`stdlib-yaml-conformance.md`](./stdlib-yaml-conformance.md); este cierre no
promueve runtime nativo, SIMD ni lowering AOT (`native_aot_lowering: not-claimed`,
`simd: not-measured-no-optimized-route`).
