# Contrato de `std.serialization`

**Estado:** contrato común e implementación portable cerrados para STD-0.1A.
Este documento define la frontera pública que deben implementar JSON,
MessagePack y Protobuf; no es una API dinámica ni una sustitución de los
contratos específicos de cada formato.

`std.serialization` es el único owner de los protocolos tipados compartidos.
Los codecs concretos eligen cómo representan sus valores dinámicos, pero una
implementación typed siempre usa estos protocolos, dispatch estático y una
construcción atómica del resultado.

## Principios

- Un tipo implementa `Serialize`, `Deserialize` o ambos. El compilador
  monomorfiza la llamada; no hay trait objects, vtables, registro global ni
  lookup por nombre.
- La ruta typed escribe directamente al `Serializer[E]` o lee eventos desde un
  `Deserializer[E]`. No materializa `JsonValue`, `MessagePackValue` ni otro
  DOM intermedio.
- `Serializer[E]` y `Deserializer[E]` son protocolos de estado. `var` expresa
  que el cursor o writer avanza; cada operación mantiene la identidad del
  error `E` del formato.
- Un decoder no publica un `T` hasta haber consumido y validado todos sus
  componentes. Un fallo deja el destino sin cambios observables.
- Los nombres de records, fields y variants son metadata de la expansión o del
  schema declarado. No se obtienen mediante reflection runtime.

## Protocolos públicos

```tondo
pub trait Serializer[E] {
    fn null(var self): Unit ! E
    fn bool(var self, value: Bool): Unit ! E
    fn int(var self, value: Int64): Unit ! E
    fn uint(var self, value: UInt64): Unit ! E
    fn float32(var self, value: Float32): Unit ! E
    fn float64(var self, value: Float64): Unit ! E
    fn string(var self, value: String): Unit ! E
    fn bytes(var self, value: Bytes): Unit ! E

    fn startArray(var self, length: Int?): Unit ! E
    fn endArray(var self): Unit ! E

    fn startMap(var self, length: Int?): Unit ! E
    fn mapKey(var self): Unit ! E
    fn endMap(var self): Unit ! E

    fn startRecord(var self, name: String, fields: Int?): Unit ! E
    fn field(var self, name: String): Unit ! E
    fn endRecord(var self): Unit ! E

    fn startEnum(var self, name: String, variant: String): Unit ! E
    fn endEnum(var self): Unit ! E
}

pub trait Deserializer[E] {
    fn next(var self): SerializationEvent ! E
    fn own(var self, event: SerializationEvent): SerializationEvent ! E
}

pub trait Serialize {
    fn serialize[E, S: Serializer[E]](self, var serializer: S): Unit ! E
}

pub trait Deserialize {
    fn deserialize[E, D: Deserializer[E]](var deserializer: D): Self ! E
}
```

`Int8`–`Int64` se emiten como `Int64` después de comprobar que la conversión
es exacta; `UInt8`–`UInt64` se emiten como `UInt64`. `Byte` se emite mediante
`bytes` o una conversión explícita a `UInt8`; nunca se confunde con un número
sin una decisión del impl generado. `Float32` conserva sus bits y `Float64`
conserva su valor IEEE completo, incluidos signed zero y NaN cuando el formato
lo admite.

`Serializer` es terminal después del primer error. `Deserializer` es terminal
después de un error de input, límite, I/O o secuencia; volver a llamar a
`next` no puede producir un valor parcial nuevo. El codec decide la forma
concreta del error `E`, pero debe conservar la clase, offset y path estructural
que exige su contrato.

## Eventos

`next` produce exactamente uno de estos eventos:

```tondo
pub enum SerializationEvent {
    Null
    Bool(Bool)
    Int(Int64)
    UInt(UInt64)
    Float32(Float32)
    Float64(Float64)
    String(String)
    Bytes(Bytes)

    StartArray(Int?)
    EndArray
    StartMap(Int?)
    MapKey
    EndMap

    StartRecord(String, Int?)
    Field(String)
    EndRecord
    StartEnum(String, String)
    EndEnum
}
```

Los valores `String` y `Bytes` entregados por un reader pueden ser vistas
temporales y solo son válidas hasta la siguiente llamada a `next`. `own` crea
una copia estable bajo los límites del reader. No existe una conversión
implícita que retenga una vista después de avanzar el cursor.

### Máquina de estados

1. El documento tiene exactamente un valor raíz.
2. `StartArray(length)` abre una secuencia; cada evento de valor cuenta como un
   elemento y `EndArray` exige la longitud declarada cuando existe.
3. `StartMap(length)` abre pares. Cada par empieza con `MapKey`, seguido de un
   valor de clave y un valor de entrada. La clave puede ser cualquier valor de
   `SerializationEvent`; no se restringe a `String`.
4. `StartRecord(name, fields)` abre un record. Cada miembro exige `Field(name)`
   seguido de exactamente un valor. El nombre debe ser único y la longitud
   declarada debe coincidir.
5. `StartEnum(name, variant)` abre una variante. Una variante puede ser unit o
   contener un único valor payload; ese valor puede ser un scalar, array, map o
   record. `EndEnum` cierra la variante.
6. Un `End*` solo puede cerrar el contenedor correspondiente y no puede dejar
   un field, una clave o un payload pendiente.

El validador usa frames explícitos y nunca recurre al stack del host. Un
`length` negativo, una longitud mayor que el límite o un cierre fuera de orden
es un error antes de reservar o publicar datos.

## Derive y personalización

El provider de `derive serialization.Serialize + serialization.Deserialize`
genera output Tondo ordinario y determinista:

- records visitan fields en orden de declaración;
- enums conservan nombre de tipo, variant y payload;
- parámetros genéricos reciben únicamente los bounds mínimos usados por la
  expansión;
- el decode construye un record temporal y lo publica solo tras validar todos
  sus fields;
- los fields privados solo se incluyen cuando el provider tiene autorización
  explícita para el tipo objetivo; nunca se hacen públicos por accidente; y
- renombrar, omitir, aplanar, transformar o asignar IDs de wire requiere un
  `impl` manual o un DTO declarado.

No se permiten attributes ejecutables, callbacks al compilador, reflection de
valores ni providers que consulten filesystem, environment, process, reloj,
red, entropy o threads. Un provider recibe un modelo semántico sellado y
produce un impl con source map y diagnostics reproducibles.

Protobuf es una excepción deliberada al derive genérico: sus field numbers,
presence y evolución vienen únicamente del `.proto` schema-first y su
generator publica impls ordinarios compatibles con estos traits.

## Implementación portable del protocolo

`crates/tondo-stdlib/src/serialization.rs` contiene la implementación de
referencia del protocolo común. `EventSerializer` aplica el límite de eventos y
publica el vector únicamente después de `validate_events`; `EventDeserializer`
mantiene un cursor acotado, soporta `peek_event` para composiciones genéricas y
solo termina con consumo completo. Los impls estáticos de `Serialize` y
`Deserialize` cubren scalars, `String`, `Option[T]` y `Array[T]` sin trait
objects, reflection ni DOM. `serialize_value`/`deserialize_value` son los
adaptadores de prueba y de los codecs; el lowering de estas operaciones a
símbolos Tondo públicos y el provider derive siguen siendo gates separados.

## Límites y errores

Cada reader, writer y codec expone límites finitos para input/output bytes,
profundidad, elementos, fields, eventos y materialización de payloads. Los
defaults se fijan por el owner y los límites explícitos no pueden ampliarse
durante una operación ya iniciada.

El protocolo común reconoce estas clases:

```tondo
pub enum SerializationError {
    UnexpectedEvent
    TypeMismatch
    MissingField
    DuplicateField
    InvalidContainerLength
    LimitExceeded
    InvalidPath
    Io(IoError)
}
```

Los codecs pueden envolver esta clase en su error nominal, pero no sustituirla
por el mensaje crudo del sistema operativo. El error conserva `offset` cuando
existe, `path` desde `$`, la fase (`read`, `write`, `derive` o `schema`) y una
razón estable. Nunca copia automáticamente payloads grandes, snippets de
entrada o secretos.

## Evidencia exigida

El owner no se considera implementado por tener solo el validador Rust. La
promoción requiere, por separado:

- tests de la máquina de eventos, cierres, duplicados, longitudes y payloads;
- properties de equivalencia frente a chunking y orden de consumo;
- límites probados sin crecimiento parcial;
- al menos un impl typed público y un decoder que publique de forma atómica;
- source maps y diagnostics del provider derive;
- prueba de ausencia de reflection/DOM en la ruta typed; y
- conexión de JSON, MessagePack y Protobuf a la misma frontera antes de cerrar
  `STD-IMPL-001`, `STD-CODEC-CONF-001` o S1A.

El registro machine-readable de este contrato es
[`testing/stdlib-serialization.json`](../../testing/stdlib-serialization.json)
y el check ejecutable es
[`scripts/stdlib-serialization-check.sh`](../../scripts/stdlib-serialization-check.sh).
El siguiente cierre de la cadena es `STD-DERIVE-SER-001`, seguido por las
implementaciones typed/streaming de JSON, MessagePack y Protobuf.
