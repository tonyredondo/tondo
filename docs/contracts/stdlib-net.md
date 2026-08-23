# Contrato de `std.net`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-NET-001`. El registro machine-readable está en
[`testing/stdlib-net.json`](../../testing/stdlib-net.json) y la integración
normativa se enlaza desde [`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la frontera de red, pero no afirma que el runtime público de
sockets, DNS o TLS ya esté implementado.

`std.net` solo aparece cuando el target selecciona la capability `network`.
Importar el módulo no abre sockets, consulta DNS, lee proxies o certificados,
crea tasks ni toca el entorno. Las operaciones de red son explícitas y usan el
único modelo suspendible de Tondo: una llamada directa espera implícitamente,
`spawn` devuelve el `Join` ordinario y no existe una familia `connectAsync`.

La frontera `HOST` tiene estado `required-after-native-gate`: el contrato
alimenta `DIAG-RUNTIME-001` y la elección de backend, mientras que los
adaptadores de VM/nativo y el proveedor TLS quedan detrás de `NATIVE-001`.

## Superficie pública

~~~tondo
pub type HostName
pub type IpAddress
pub type SocketAddress
pub type NetLimits
pub type NetOptions
pub type TcpListener
pub type TcpStream
pub type TcpReadHalf
pub type TcpWriteHalf
pub type UdpSocket
pub type Datagram
pub type TlsConfig
pub type TlsStream
pub type TlsReadHalf
pub type TlsWriteHalf

pub enum NetError {
    InvalidHostName
    InvalidAddress
    InvalidPort
    InvalidLimit
    InvalidDeadline
    ResourceLimit
    ResolveFailed
    ConnectionRefused
    ConnectionReset
    NotConnected
    AddressInUse
    Unreachable
    DatagramTooLarge
    Timeout
    Cancelled
    Closed
    CapabilityMissing
    Host
}

pub enum TlsError {
    InvalidServerName
    InvalidCertificate
    CertificateRejected
    HandshakeFailed
    Unsupported
    ResourceLimit
    Timeout
    Cancelled
    Closed
    Transport(NetError)
}

pub enum TlsVerification {
    PlatformRoots
    PinnedCertificate(Bytes)
}

pub enum Shutdown { Read, Write, Both }

pub fn hostName(text: String): HostName ! NetError
pub fn IpAddress.parse(text: String): IpAddress ! NetError
pub fn socketAddress(ip: IpAddress, port: Int): SocketAddress ! NetError
pub fn NetLimits.create(maxRead: Int, maxDatagram: Int, maxResults: Int): NetLimits ! NetError
pub fn NetLimits.defaults(): NetLimits
pub fn options(deadline: Instant?, limits: NetLimits): NetOptions ! NetError

pub fn resolve(host: HostName, port: Int, options: NetOptions): Array[SocketAddress] ! NetError suspends
pub fn connect(address: SocketAddress, options: NetOptions): TcpStream ! NetError suspends
pub fn listen(address: SocketAddress, backlog: Int): TcpListener ! NetError
pub fn TcpListener.accept(ref self, options: NetOptions): TcpStream ! NetError selectable
pub fn TcpListener.close(self): Unit

pub fn TcpStream.split(self): (TcpReadHalf, TcpWriteHalf)
pub fn TcpStream.localAddress(self): SocketAddress ! NetError
pub fn TcpStream.peerAddress(self): SocketAddress ! NetError
pub fn TcpStream.shutdown(ref self, how: Shutdown, options: NetOptions): Unit ! NetError suspends
pub fn TcpStream.close(self): Unit
pub fn TcpReadHalf.read(ref self, max: Int, options: NetOptions): ReadResult ! NetError selectable
pub fn TcpReadHalf.close(self): Unit
pub fn TcpWriteHalf.write(ref self, data: Bytes, options: NetOptions): Int ! NetError suspends
pub fn TcpWriteHalf.flush(ref self, options: NetOptions): Unit ! NetError suspends
pub fn TcpWriteHalf.shutdown(ref self, options: NetOptions): Unit ! NetError suspends
pub fn TcpWriteHalf.close(self): Unit

pub fn bind(address: SocketAddress): UdpSocket ! NetError
pub fn UdpSocket.sendTo(ref self, data: Bytes, destination: SocketAddress, options: NetOptions): Unit ! NetError suspends
pub fn UdpSocket.receiveFrom(ref self, options: NetOptions): Datagram ! NetError selectable
pub fn UdpSocket.localAddress(self): SocketAddress ! NetError
pub fn UdpSocket.close(self): Unit
pub fn Datagram.bytes(self): Bytes
pub fn Datagram.source(self): SocketAddress

pub fn tlsConfig(verification: TlsVerification): TlsConfig ! TlsError
pub fn TlsStream.connect(stream: TcpStream, server: HostName, config: TlsConfig, options: NetOptions): TlsStream ! TlsError suspends
pub fn TlsStream.split(self): (TlsReadHalf, TlsWriteHalf)
pub fn TlsStream.close(self): Unit
pub fn TlsReadHalf.read(ref self, max: Int, options: NetOptions): ReadResult ! TlsError suspends
pub fn TlsReadHalf.close(self): Unit
pub fn TlsWriteHalf.write(ref self, data: Bytes, options: NetOptions): Int ! TlsError suspends
pub fn TlsWriteHalf.flush(ref self, options: NetOptions): Unit ! TlsError suspends
pub fn TlsWriteHalf.shutdown(ref self, options: NetOptions): Unit ! TlsError suspends
pub fn TlsWriteHalf.close(self): Unit
~~~

`HostName`, `IpAddress` y `SocketAddress` son valores inmutables, `Copy`,
`Discard`, `Send`, `Share`, `Equatable` y `Key`. Los handles de red son afines:
no son `Copy`, `Clone` ni `Share`; pueden transferirse a una task o thread que
cumpla `Send`, pero nunca se comparten mediante una dirección raw. `NetLimits`
y `NetOptions` son snapshots inmutables y copiables. `TlsConfig` es una
configuración inmutable que puede compartirse; no contiene callbacks ni un
provider nativo suministrado por el usuario.

`TcpStream.split` consume el stream y separa de forma explícita el owner de
lectura y el de escritura. Cada mitad es afín y debe consumirse con `close`;
el último cierre libera el transporte subyacente. Así pueden existir un task
lector y otro escritor sin alias mutable implícito. Un stream no dividido se
cierra directamente y no tiene métodos de I/O concurrentes.

## Direcciones, puertos y DNS

`IpAddress.parse` acepta únicamente literales IPv4 e IPv6 en su forma
canónica aceptada por el target; no consulta DNS. `socketAddress` acepta puertos
entre `0` y `65535`. El puerto cero solo es válido para `listen`/`bind` y pide
un puerto efímero; `connect`, `resolve` y `sendTo` requieren un puerto
positivo. Ninguna operación normaliza, hace IDNA, interpreta un path Unix o
convierte silenciosamente texto inválido.

`hostName` acepta nombres DNS ASCII de hasta 253 bytes, con labels de 1 a 63
bytes, sin NUL, espacios, barras ni un punto inicial. El caller debe convertir
IDNA a ASCII antes de llamar. Un literal IP puede usarse directamente y no
necesita resolución.

`resolve` es la única operación que consulta el resolver seleccionado por el
target. Devuelve direcciones en el orden del provider, elimina duplicados
byte-a-byte y limita el resultado a `NetLimits.maxResults`. No reintenta, no
implementa Happy Eyeballs, no lee `HTTP_PROXY`, `NO_PROXY`, `/etc/resolv.conf`
desde Tondo ni ejecuta un shell. El provider puede usar la configuración del
host que el target haya declarado; esa dependencia forma parte de la identidad
del target y nunca del ambiente del programa.

La conformance usa un resolver sellado y controlable. La conformance no afirma
que una respuesta DNS externa sea reproducible; solo exige que el orden,
duplicados, límites, errores y cancelación del provider seleccionado sean
observables y estén documentados.

## Límites, deadlines y cancelación

`NetLimits.create` exige límites positivos y finitos. `maxRead` limita los
bytes que puede publicar una lectura, `maxDatagram` el tamaño máximo recibido o
enviado por UDP y `maxResults` el número de direcciones de una resolución.
`defaults()` devuelve los valores normativos del target, no consulta CPU,
memoria, environment ni variables del proceso. Un límite no representable o
que exceda el presupuesto del runtime devuelve `NetError.ResourceLimit` antes
de reservar estado.

`options` recibe un `Instant?` monotónico explícito. `none` significa que la
operación no tiene deadline; no instala un timeout ambiental. Un `Instant` exige
la capability `clock` además de `network` y debe pertenecer al provider activo.
Un reloj de otro dominio o un deadline no representable devuelve
`NetError.InvalidDeadline` antes de registrar la operación.
El deadline cubre DNS, cola del provider, handshake o I/O de la operación a la
que se pasa, pero no elimina el cleanup: tras vencer, el host cierra o
desregistra sus recursos antes de publicar `Timeout`.

La prioridad de outcomes es: validación estática/argumentos, cancelación
observada antes del commit, `Timeout`, error host y resultado normal. Una
operación que ya hizo commit publica su resultado aunque la cancelación llegue
después. Cancelar una espera de `accept` o `receiveFrom` no retira una conexión
ni un datagrama; perder un brazo `select` desregistra la espera sin consumirlo.
No hay polling público, sleeps internos ni retries automáticos.

Todas las operaciones suspendibles responden a la cancelación cooperativa del
scope. Ningún worker cooperativo espera bloqueado a `connect`, DNS, TLS o a un
descriptor host. El adaptador usa readiness/worker host y devuelve el control a
Tondo; `spawn thread` solo es una decisión explícita del caller, no un fallback
del módulo.

## TCP

`listen` valida un backlog positivo, reserva el socket y devuelve un listener
afín. `accept` es `selectable`: `prepare` registra readiness, `commit` retira
una conexión exactamente una vez y `rollback` no la pierde. `close` deja de
aceptar, despierta waiters con `Closed` y libera el descriptor después del
cleanup.

`connect` opera sobre una dirección ya resuelta. No hace DNS, no intenta otras
direcciones y no cambia de transporte si falla. El socket se publica solo
después del commit de conexión; un fallo o cancelación no deja un stream medio
construido.

`TcpStream.shutdown` solo está disponible antes de `split` y aplica `Read`,
`Write` o `Both` de forma explícita. `split` consume el stream; después de esa
operación el writer usa `TcpWriteHalf.shutdown` para emitir FIN y el cierre del
reader es terminal. Esto evita que un enum de shutdown pueda alcanzar una mitad
que ya no posee el estado necesario para ejecutarlo.

`TcpReadHalf.read` devuelve `ReadResult.Data` con uno o más bytes, menos de
`max` cuando el kernel entrega un chunk corto, o `ReadResult.Eof` después de un
EOF limpio. Es `selectable`; un brazo perdedor no consume bytes. Un timeout o
cancelación posterior a un chunk ya publicado afecta a la siguiente llamada,
no borra el chunk observado. `max <= 0` es `InvalidLimit` y no toca el socket.

`TcpWriteHalf.write` puede aceptar un prefijo y devuelve su longitud; el caller
debe repetir con el resto. La implementación nunca devuelve un error con una
longitud parcial desconocida: si no puede aceptar más antes del deadline,
devuelve `Timeout` y no afirma haber consumido bytes no reportados. `flush`
espera a que el buffer del adaptador se entregue al transporte y no promete
durabilidad remota. Ningún buffer Tondo de escritura crece sin límite.

`shutdown` permite cerrar la mitad de lectura, de escritura o ambas según el
estado del half; cerrar escritura emite FIN cuando el host lo permite. Es una
operación suspendible porque puede drenar el adaptador; `close` es terminal y
libera el half sin esperar confirmación remota. Un reset, FIN, error de host o
uso posterior se traduce a `NetError` sin exponer errno, paths o mensajes del
SO.

## UDP y datagrams

`bind` crea un `UdpSocket` afín. `sendTo` es datagram-atómico: el datagram
completo se acepta o se devuelve un error; nunca se publica un prefijo como si
fuera un mensaje válido. Si excede `maxDatagram` devuelve
`DatagramTooLarge` antes de tocar el socket. `receiveFrom` es `selectable` y
devuelve exactamente un `Datagram` con bytes y dirección de origen. Un datagram
mayor que el límite no se trunca silenciosamente: se descarta y la operación
devuelve `DatagramTooLarge`, permitiendo al caller aumentar su límite de forma
explícita.

UDP no promete entrega, orden, unicidad ni ausencia de duplicados. No se
añaden reintentos, framing, multicast, broadcast, raw sockets ni una cola
Tondo ilimitada. El kernel aplica su propio buffer acotado; la API publica
`ResourceLimit`, `Timeout` o `Host` cuando ese buffer no puede progresar.

## TLS boundary

`tlsConfig` es la única forma de construir una configuración. `PlatformRoots`
usa el bundle de confianza versionado por el target; no lee variables de
entorno, directorios del usuario ni una CA global mutable. `PinnedCertificate`
recibe bytes DER explícitos y se valida antes del handshake. No existe un modo
`Insecure`, una aceptación de certificados inválidos por defecto, downgrade a
plaintext ni callback de verificación escrito en Tondo.

`TlsStream.connect` consume un `TcpStream`, valida el `HostName` para SNI y
hostname verification, ejecuta el handshake con el mismo deadline y publica el
stream solo cuando la sesión es válida. Un fallo de transporte se envuelve en
`TlsError.Transport(NetError)`; un certificado rechazado no expone el certificado
ni el motivo dependiente del provider. Si el handshake falla, el transporte se
cierra y no queda un socket parcialmente utilizable.

`TlsStream.split` sigue las mismas reglas de ownership que TCP. Las mitades
leen y escriben plaintext; el provider conserva cifrado, record framing,
renegotiation prohibida y límites internos. `flush` puede emitir records
pendientes. `shutdown` del writer intenta enviar `close_notify`; cancelación o
unwind puede cerrar directamente sin prometer un alert remoto.

Las mitades TLS no publican `selectable`: un record puede exigir leer y escribir
durante la misma transición del provider. El caller compone esa operación con
el `spawn` y `Join` normales, sin crear un segundo selector ni una API paralela.

El contrato no fija una biblioteca TLS concreta, ABI, cipher suite privada ni
formato de trust store. Sí fija que el target declare el provider, las suites
permitidas, la versión TLS mínima, los límites y el hash del bundle de roots.

## Diagnóstico, cleanup y portabilidad

El runtime puede emitir eventos privados en `std.net`: `resolve.start`,
`resolve.finish`, `connect.start`, `connect.finish`, `listener.accept.prepare`,
`listener.accept.commit`, `listener.accept.rollback`, `stream.read`,
`stream.write`, `stream.shutdown`, `udp.receive`, `udp.send`, `tls.handshake`,
`resource.timeout`, `resource.cancel` y `resource.close`. Cada evento lleva como
mínimo `run_id`, `task_id`, `operation_id`, `resource_id`, `event_sequence`,
`state`, `source_revision` y `target`; payloads, direcciones, certificados y
bytes se omiten por defecto. Son hooks privados para `DIAG-RUNTIME-001`, no una
API pública de tracing.

El cleanup de cualquier recurso afín es idempotente internamente y se ejecuta
en éxito, error, timeout, cancelación, panic y abandono defensivo del VM. Un
programa seguro no puede abandonar un handle vivo ni usarlo después de
`close`; el host rechaza tokens stale o forjados. El cleanup nunca mata tasks
Tondo ni altera el resultado primario salvo que la prioridad de error del
contrato lo exija.

El comportamiento portable común cubre ownership, partial I/O, deadlines,
cancelación, límites, errores nominales y ausencia de efectos por import. IPv4,
IPv6, resolver, socket readiness y TLS son diferencias declaradas del target.
Unix-domain sockets, QUIC, HTTP, WebSocket, RPC/gRPC, proxy autodetectado,
multicast, raw sockets, FFI/ABI de sockets y APIs de framework quedan fuera de
este owner.

## Exclusiones y promoción

El contrato excluye `connectAsync`, `acceptAsync`, `NetFuture`, `SocketPoller`,
`std.net.select`, callbacks de readiness, polling público, retries implícitos,
Happy Eyeballs implícito, buffer ilimitado, TLS inseguro, downgrade plaintext,
resolver configurable por environment, `HttpClient`, `RpcClient`, QUIC,
WebSocket, Unix sockets y raw sockets.

La implementación queda pendiente de
`STD-NET-IMPL-001`, `STD-NET-HOST-001`, `STD-NET-TEST-001`,
`STD-NET-PERF-001`, `STD-NET-CONF-001` y `STD-NET-DOC-001`. El contrato puede
alimentar `DIAG-RUNTIME-001` y `NATIVE-001`, pero no promociona símbolos
runtime antes de cerrar esas leaves.
