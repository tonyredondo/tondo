# Contrato de `std.time` civil

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-CIVIL-TIME-001`. El registro machine-readable está en
[`testing/stdlib-civil-time.json`](../../testing/stdlib-civil-time.json) y la
integración normativa se enlaza desde
[`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre define el calendario civil, pero no afirma que los adaptadores VM o
nativos, ni la base de zonas, ya estén implementados.

`std.time` conserva un único time-base (`single-duration`, `single-instant`).
`Duration`, `Instant`, `Timer`,
`deadline`, `sleep` y la sustitución virtual pertenecen al contrato monotónico
de [`stdlib-time.md`](./stdlib-time.md); este documento no duplica ninguno de
ellos. El calendario civil es una representación de fecha y hora humana, no una
forma alternativa de medir intervalos.

La superficie pura no consulta el host. El reloj de pared actual requiere la
capability explícita `civil-clock`; una captura que relacione ese reloj con un
`Instant` requiere además `clock`. La base de zonas es un input inmutable del
target, identificado por versión y hash, y nunca se busca en `TZ`, locale,
filesystem, red o environment.

## Superficie pública

~~~tondo
pub type Date
pub type Time
pub type DateTime
pub type UtcDateTime
pub type UtcOffset
pub type ZoneId
pub type ZoneDataVersion
pub type ZoneDatabase
pub type TimeZone
pub type ZonedDateTime
pub type CivilAnchor

pub enum CivilError {
    InvalidDate
    InvalidTime
    InvalidOffset
    InvalidZoneId
    ZoneUnavailable
    ZoneDataUnavailable
    NonexistentLocalTime
    AmbiguousLocalTime
    OutOfRange
    DomainMismatch
    Unavailable
    ResourceLimit
}

pub enum MonthPolicy { Reject, Clamp }
pub enum ResolvePolicy { Reject, Earlier, Later, ShiftForward }

pub fn Date.create(year: Int, month: Int, day: Int): Date ! CivilError
pub fn Date.parse(text: String): Date ! CivilError
pub fn Date.year(self): Int
pub fn Date.month(self): Int
pub fn Date.day(self): Int
pub fn Date.dayOfWeek(self): Int
pub fn Date.dayOfYear(self): Int
pub fn Date.addDays(self, days: Int): Date ! CivilError
pub fn Date.addMonths(self, months: Int, policy: MonthPolicy): Date ! CivilError
pub fn Date.addYears(self, years: Int, policy: MonthPolicy): Date ! CivilError
pub fn Date.format(self): String

pub fn Time.create(hour: Int, minute: Int, second: Int, nanosecond: Int): Time ! CivilError
pub fn Time.parse(text: String): Time ! CivilError
pub fn Time.hour(self): Int
pub fn Time.minute(self): Int
pub fn Time.second(self): Int
pub fn Time.nanosecond(self): Int
pub fn Time.format(self): String

pub fn DateTime.create(date: Date, time: Time): DateTime
pub fn DateTime.parse(text: String): DateTime ! CivilError
pub fn DateTime.date(self): Date
pub fn DateTime.time(self): Time
pub fn DateTime.add(self, amount: Duration): DateTime ! CivilError
pub fn DateTime.format(self): String

pub fn UtcDateTime.create(date: Date, time: Time): UtcDateTime
pub fn UtcDateTime.parse(text: String): UtcDateTime ! CivilError
pub fn UtcDateTime.date(self): Date
pub fn UtcDateTime.time(self): Time
pub fn UtcDateTime.add(self, amount: Duration): UtcDateTime ! CivilError
pub fn UtcDateTime.format(self): String
pub fn UtcDateTime.inZone(self, zone: TimeZone): ZonedDateTime ! CivilError

pub fn UtcOffset.fromSeconds(seconds: Int): UtcOffset ! CivilError
pub fn UtcOffset.seconds(self): Int
pub fn UtcOffset.format(self): String

pub fn ZoneId.fromText(text: String): ZoneId ! CivilError
pub fn ZoneId.text(self): String

pub fn zoneDatabase(): ZoneDatabase ! CivilError
pub fn ZoneDatabase.version(self): ZoneDataVersion
pub fn ZoneDatabase.hash(self): String
pub fn ZoneDatabase.zone(ref self, id: ZoneId): TimeZone ! CivilError
pub fn ZoneDataVersion.text(self): String

pub fn TimeZone.id(self): ZoneId
pub fn TimeZone.version(self): ZoneDataVersion
pub fn TimeZone.offsetAt(self, utc: UtcDateTime): UtcOffset ! CivilError
pub fn TimeZone.resolve(self, local: DateTime, policy: ResolvePolicy): ZonedDateTime ! CivilError

pub fn ZonedDateTime.local(self): DateTime
pub fn ZonedDateTime.zone(self): ZoneId
pub fn ZonedDateTime.offset(self): UtcOffset
pub fn ZonedDateTime.isDaylight(self): Bool
pub fn ZonedDateTime.toUtc(self): UtcDateTime ! CivilError
pub fn ZonedDateTime.add(self, amount: Duration): ZonedDateTime ! CivilError

pub fn CivilClock.now(): UtcDateTime ! CivilError
pub fn CivilClock.sample(): CivilAnchor ! CivilError
pub fn CivilAnchor.instant(self): Instant
pub fn CivilAnchor.utc(self): UtcDateTime
pub fn CivilAnchor.toUtc(self, instant: Instant): UtcDateTime ! CivilError
pub fn CivilAnchor.toInstant(self, utc: UtcDateTime): Instant ! CivilError
~~~

`Date`, `Time`, `DateTime`, `UtcDateTime`, `UtcOffset`, `ZoneId`, `ZoneDataVersion` y
`ZonedDateTime` son valores inmutables `Copy + Discard + Send + Share +
Equatable`. `TimeZone` y `ZoneDatabase` son snapshots inmutables copiables y
compartibles del mismo bundle; no contienen un descriptor abierto ni un path
físico. `CivilAnchor` es también un valor copiables y compartible, pero conserva
la identidad del dominio monotónico de su `Instant`. Ningún tipo civil es un
puntero, un handle de host o un alias mutable.

`Date.dayOfWeek` devuelve `1..7` (lunes..domingo) y `dayOfYear` devuelve
`1..365/366`. Los valores usan el calendario gregoriano proléptico y el rango
normativo de año `1..9999`. El año cero, fechas fuera del rango y los valores
que desbordan ese rango producen `CivilError.InvalidDate` u
`CivilError.OutOfRange` según corresponda.

## Parsing, formato y aritmética

El formato es deliberadamente pequeño y sin locale:

- `Date.parse` acepta exactamente `YYYY-MM-DD`, con cuatro dígitos de año y
  dos de mes y día. `Date.format` produce siempre esa forma.
- `Time.parse` acepta `HH:MM:SS` con una fracción opcional de uno a nueve
  dígitos después de `.`. La fracción se normaliza a nanosegundos y
  `Time.format` omite la fracción cuando es cero y, si existe, elimina ceros a
  la derecha.
- `DateTime.parse` acepta `YYYY-MM-DDTHH:MM:SS[.fffffffff]` sin offset ni
  sufijo. Es una fecha/hora local ingenua y no representa un instante.
- `UtcDateTime.parse` exige exactamente la misma forma seguida por `Z` y
  `format` siempre conserva ese sufijo. No se aceptan offsets textuales en esta
  forma; se construye una zona mediante `TimeZone.resolve`.
- `UtcOffset` es el valor nominal para un desplazamiento firmado en segundos;
  no se usa un `Int` desnudo en la API de zonas. `fromSeconds` comprueba
  `[-86399, 86399]` y `format` produce `Z` para cero o `±HH:MM:SS` para los
  demás valores.
- `ZoneId.fromText` acepta un ID IANA canónico ASCII, con `/` entre
  componentes, sin `..`, NUL, espacios, alias ni un path físico. `UTC` es el
  único nombre especial permitido. La normalización de locale, IDNA y aliases
  no es implícita.

No se admiten segundos intercalares: `second` está en `0..59`, `24:00:00` es
inválido y el calendario no inventa una fecha para un segundo 60. Si una
aplicación necesita representar un dato externo con leap seconds debe conservar
el texto/bytes original mediante su propio tipo.

`Date.addDays` usa aritmética gregoriana comprobada. `addMonths` y `addYears`
requieren una política explícita: `Reject` devuelve `InvalidDate` cuando el día
no existe en el destino; `Clamp` lo lleva al último día válido del mes. No hay
una sobrecarga que elija una política por defecto. `DateTime.add` y
`UtcDateTime.add` reutilizan el único `Duration` y comprueban el rango antes de
publicar el valor; no consultan ningún reloj.

Todas las operaciones de parsing, formato y aritmética son síncronas, puras y
de coste acotado por el tamaño finito de sus inputs. La implementación puede
usar tablas, división especializada o SIMD para lotes, pero el resultado es
byte-exacto y no puede depender de la plataforma, locale o frecuencia del CPU.

## Bundle de zonas versionado

`zoneDatabase()` devuelve el único bundle de zonas seleccionado por el target
(`target-declared-immutable`).
El bundle es inmutable durante una ejecución y expone un `ZoneDataVersion` y
un SHA-256. El manifiesto y el lockfile incluyen ambos valores; cambiar reglas
históricas, aliases o transiciones cambia el hash del artefacto. Un target que
anuncia el source set de zonas debe proporcionar el bundle completo o rechazar
la selección estáticamente; nunca instala un stub que consulte el sistema
operativo.

`ZoneDatabase.zone` solo selecciona datos del snapshot ya materializado. No
abre archivos, no hace DNS, no mira `TZ`, `LANG`, `LC_*`, `HOME`, variables del
proceso ni la hora actual. La ausencia de un ID devuelve `ZoneUnavailable` y la
ausencia del bundle devuelve `ZoneDataUnavailable`; no se sustituye por UTC ni
por la zona local del host. La resolución es determinista para la pareja
`(version, hash)` y el mismo corpus produce los mismos offsets en VM y nativo.

`TimeZone.offsetAt` recibe un `UtcDateTime`, no un `Instant`: es una consulta
civil pura sobre el bundle. Devuelve un `UtcOffset` cuyo valor firmado en
segundos está dentro de `[-86399, 86399]`. `ZonedDateTime` conserva el ID, la versión del bundle, la
fecha/hora local y el offset elegido; por ello una actualización de datos no
puede cambiar silenciosamente el significado de un valor ya creado.

## Gaps, folds y conversiones locales

Una transición de zona puede hacer que una hora local no exista (gap) o que
exista dos veces (fold). `TimeZone.resolve` siempre recibe un
`ResolvePolicy`:

| Política | Gap | Fold |
|---|---|---|
| `Reject` | `NonexistentLocalTime` | `AmbiguousLocalTime` |
| `Earlier` | error | primera ocurrencia |
| `Later` | error | segunda ocurrencia |
| `ShiftForward` | avanza por el tamaño del gap | segunda ocurrencia |

El offset resultante y la versión del bundle quedan fijados en el
`ZonedDateTime`. `toUtc` comprueba el cálculo y devuelve `OutOfRange` si el
resultado no cabe; nunca vuelve a consultar una zona distinta. `UtcDateTime.inZone`
usa la tabla del `TimeZone` y no presenta ambigüedad porque parte de UTC.

La igualdad de `ZonedDateTime` compara el instante civil UTC, el ID de zona y
la versión del bundle. Dos valores con el mismo texto local en un fold no son
iguales si eligieron offsets distintos. La igualdad no compara punteros ni
identidad de objetos del host.

## Reloj civil y ancla monotónica

`CivilClock.now` y `CivilClock.sample` son operaciones síncronas, no
`selectable` y no crean tasks. Solo existen en la interfaz que declara la
capability `civil-clock`; su ausencia produce `E1008` de forma estática. El
reloj de pared puede saltar hacia delante o atrás y no ofrece la garantía de
monotonicidad de `std.time.now`.

`CivilClock.sample` requiere `civil-clock` y `clock`. Devuelve un
`CivilAnchor` con un `Instant` del proveedor monotónico activo y el
`UtcDateTime` observado en la misma frontera. Las conversiones del ancla son
aritmética comprobada:

~~~text
toUtc(i)    = utc0 + (i - instant0)
toInstant(u)= instant0 + (u - utc0)
~~~

El ancla no convierte un número Unix, no fabrica una época y no actualiza el
reloj. Si el `Instant` pertenece a otro dominio, si la diferencia desborda o
si el valor está fuera del horizonte finito publicado por el target, devuelve
`DomainMismatch` u `OutOfRange`. Un salto posterior del reloj civil no altera
el ancla ya tomada; para una lectura actual se llama a `now` de nuevo.

Esta relación es deliberadamente explícita: el código que solo necesita medir
latencia usa `Instant` y `Duration`; el que necesita presentar una fecha usa
tipos civiles; solo una frontera que necesite ambos pide una muestra y maneja
su error. No existe conversión implícita `Instant -> DateTime`, epoch oculto,
overload por nombre ni API `nowAsync`.

## Capability, límites y diagnóstico

El source set core (tipos, parsing, formato, aritmética y consultas sobre un
bundle ya disponible) no requiere capabilities de host. El source set de
`CivilClock` requiere `civil-clock`; el source set de `CivilAnchor` requiere
`civil-clock + clock`. `std.testing` puede sustituir el bundle por un fixture
versionado y sellado, pero no concede ninguna de esas capabilities ni cambia
las firmas de producción.

Los límites mínimos del target son: año `1..9999`, texto de zona de 255 bytes,
al menos 4096 transiciones consultables por zona, un bundle finito y un
horizonte de ancla finito. Excederlos produce `ResourceLimit` u `OutOfRange`
antes de reservar estado o publicar un valor parcial. Los errores son nominales
y no incluyen errno, paths físicos, locale ni mensajes no deterministas.

Los hooks de diagnóstico privados pueden emitir `zone.lookup`, `zone.resolve`,
`civil.now` y `civil.anchor`, con `run_id`, `operation_id`, `zone_version`,
`source_revision` y una secuencia estable. No publican payloads, texto de
environment ni offsets secretos por defecto y no forman una API de aplicación.

## Exclusiones y promoción

Este contrato no añade `Calendar`, locale, `Holiday`, cron, scheduler, timers,
sleep, `Task`, `Future`, un segundo `Duration`, un segundo `Instant`, formato
cultural, timezone del host, parsing permissivo, leap seconds, retries ni
consultas durante compilación. `std.time` tampoco crea una API paralela
sincrónica/asíncrona: leer el reloj es síncrono y el resto de operaciones sigue
el modelo único del lenguaje.

La frontera permanece `contract-locked` hasta cerrar
`STD-CIVIL-TIME-IMPL-001`, `STD-CIVIL-TIME-HOST-001`,
`STD-CIVIL-TIME-TEST-001`, `STD-CIVIL-TIME-PERF-001`,
`STD-CIVIL-TIME-CONF-001` y `STD-CIVIL-TIME-DOC-001`. Esas leaves deben
publicar el bundle hash-bound, demostrar equivalencia VM/nativo, medir parsing,
lookup y conversiones, y mantener la separación monotónica/civil. El contrato
no promociona todavía símbolos runtime ni cierra Gate S1.
