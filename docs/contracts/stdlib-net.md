# Contrato de `std.net`

**Estado:** contrato `contract-locked` para STD-0.1B, cerrado por
`STD-NET-001`. El registro machine-readable está en
[`testing/stdlib-net.json`](../../testing/stdlib-net.json) y la integración
normativa se enlaza desde [`TONDO_STANDARD_LIBRARY_SPEC.md`](../../TONDO_STANDARD_LIBRARY_SPEC.md).
Este cierre fija la frontera de red, pero no afirma que el runtime público de
sockets, DNS o TLS ya esté implementado.

The public hosted implementation is tracked separately in
[`stdlib-net-host.md`](stdlib-net-host.md). Its current-source promotion requires
the general selection prerequisite, joint quality gate and exact-SHA CI.
The private-provider register retains its historical kernel-only evidence;
the live integration state is the separate `host` entry. The independent model,
controlled public replay and bounded fuzz scope are recorded in
[`stdlib-net-test.md`](stdlib-net-test.md) and
[`testing/stdlib-net-test.json`](../../testing/stdlib-net-test.json).

`std.net` solo aparece cuando el target selecciona la capability `network`.
Importar el módulo no abre sockets, consulta DNS, lee proxies o certificados,
crea tasks ni toca el entorno. Las operaciones de red son explícitas y usan el
único modelo suspendible de Tondo: una llamada directa espera implícitamente,
`spawn` devuelve el `Join` ordinario y no existe una familia `connectAsync`.

The original design records `required-after-native-gate` as the HOST
prerequisite. `NATIVE-001` is complete. This historical dependency does not
describe the live HOST status or establish native networking support.

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
pub fn TcpListener.accept(self, options: NetOptions): TcpStream ! NetError selectable
pub fn TcpListener.localAddress(self): SocketAddress ! NetError
pub fn TcpListener.close(listener: TcpListener): Unit

pub fn TcpStream.split(stream: TcpStream): (TcpReadHalf, TcpWriteHalf)
pub fn TcpStream.localAddress(self): SocketAddress ! NetError
pub fn TcpStream.peerAddress(self): SocketAddress ! NetError
pub fn TcpStream.shutdown(self, how: Shutdown, options: NetOptions): Unit ! NetError suspends
pub fn TcpStream.close(stream: TcpStream): Unit
pub fn TcpReadHalf.read(self, max: Int, options: NetOptions): ReadResult ! NetError selectable
pub fn TcpReadHalf.close(reader: TcpReadHalf): Unit
pub fn TcpWriteHalf.write(self, data: Bytes, options: NetOptions): Int ! NetError suspends
pub fn TcpWriteHalf.flush(self, options: NetOptions): Unit ! NetError suspends
pub fn TcpWriteHalf.shutdown(self, options: NetOptions): Unit ! NetError suspends
pub fn TcpWriteHalf.close(writer: TcpWriteHalf): Unit

pub fn bind(address: SocketAddress): UdpSocket ! NetError
pub fn UdpSocket.sendTo(self, data: Bytes, destination: SocketAddress, options: NetOptions): Unit ! NetError suspends
pub fn UdpSocket.receiveFrom(self, options: NetOptions): Datagram ! NetError selectable
pub fn UdpSocket.localAddress(self): SocketAddress ! NetError
pub fn UdpSocket.close(socket: UdpSocket): Unit
pub fn Datagram.bytes(self): Bytes
pub fn Datagram.source(self): SocketAddress

pub fn tlsConfig(verification: TlsVerification): TlsConfig ! TlsError
pub fn TlsStream.connect(stream: TcpStream, server: HostName, config: TlsConfig, options: NetOptions): TlsStream ! TlsError suspends
pub fn TlsStream.split(stream: TlsStream): (TlsReadHalf, TlsWriteHalf)
pub fn TlsStream.close(stream: TlsStream): Unit
pub fn TlsReadHalf.read(self, max: Int, options: NetOptions): ReadResult ! TlsError suspends
pub fn TlsReadHalf.close(reader: TlsReadHalf): Unit
pub fn TlsWriteHalf.write(self, data: Bytes, options: NetOptions): Int ! TlsError suspends
pub fn TlsWriteHalf.flush(self, options: NetOptions): Unit ! TlsError suspends
pub fn TlsWriteHalf.shutdown(self, options: NetOptions): Unit ! TlsError suspends
pub fn TlsWriteHalf.close(writer: TlsWriteHalf): Unit
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

`TcpListener.localAddress` returns the bound address, including the port assigned
by the operating system after `listen` with port zero. The query borrows the
listener and neither accepts a connection nor consumes the listener.

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

## Adopted hosted provider and explicit target inputs

The hosted provider uses the pinned private dependencies Tokio 1.53.2,
Hickory Resolver 0.26.3, Rustls 0.23.45 with AWS-LC, and webpki-roots 1.0.9.
Tondo retains its own scheduler and suspension model. Importing `std.net`
does not construct a runtime, resolver or TLS connection.

Selecting `network` requires this additional human manifest table:

~~~toml
[target.network]
resolver_servers = ["192.0.2.53:53", "[2001:db8::53]:53"]
~~~

The addresses above are documentation examples, not default resolvers.
The ordered list contains 1..8 distinct numeric IP endpoints with positive
ports; each spelling is at most 64 bytes. Host names, IPv6 scope/flow IDs,
duplicate numeric addresses and unknown keys are rejected. Omitting the table
with `network`, or supplying it without `network`, is an error. The complete
ordered configuration enters the project manifest and build identity. There
is no public `Resolver` object and no fallback to system DNS configuration.

The private provider selects one endpoint per explicit resolution, rotating
in declared order from the first endpoint. Both A and AAAA use that endpoint;
the result places A records before AAAA records and retains record order within
each family. There is no retry or server failover within a resolution,
including UDP retransmissions below the resolver layer. Search domains, hosts
files, cache, TCP fallback and opportunistic encrypted-DNS discovery are
disabled. A DNS transaction has a declared five-second provider response
bound; exhaustion is `ResolveFailed`, rather than an implicit `NetOptions`
deadline. A caller's earlier deadline still covers the complete operation.
Provider futures belong to the operation and are polled by its owner; dropping
the operation synchronously retires those futures and their sockets.

TLS admits TLS 1.2 and 1.3 with explicit AES-GCM suites for ECDHE/RSA or
ECDHE/ECDSA in TLS 1.2, and AES-GCM/ChaCha20-Poly1305 suites in TLS 1.3.
Session resumption and early data are disabled. Platform roots come only from
the versioned Mozilla bundle; its identity hashes the ordered trust anchors,
including field lengths and optional name constraints. An explicit DER pin
must match the exact received leaf and still pass certificate validity, name,
chain and handshake-signature verification. Pins are bounded to 64 KiB;
provider outgoing buffers are bounded to 64 KiB. These are provider bounds,
not RSS or operating-system allocation measurements.

Terminal operations and `split` use qualified associated calls with ordinary
by-value parameters, for example `TcpStream.close(stream)` and
`TcpStream.split(stream)`. Observational methods use the canonical shared
`self` receiver. The contract does not introduce a `ref self` receiver or an
implicit consuming instance receiver.

The private Rust kernel/provider has state `verified-kernel-private-provider`;
its focused tests and source-bound quality reports are recorded in
[stdlib-net-implementation.md](stdlib-net-implementation.md). Public VM
registration, compiler ownership checks, target activation and production
scope cleanup belong to the separate `STD-NET-HOST-001` evidence. Native ABI/AOT
execution and conformant promotion are not established by this private
implementation checkpoint. Each tracker owner requires its complete
functional gate and publication CI.

The live production hosted boundary is `verified-production-hosted` in
[stdlib-net-host.md](stdlib-net-host.md), and the independent model, regression
and bounded-fuzz boundary is `verified` in
[stdlib-net-test.md](stdlib-net-test.md). Their joint source `d130e2b` passes
current-source quality, all local checks and exact-checkout Linux CI. The
register's `implementation` and `private_provider_draft` retain the original
private checkpoint, including its originally required follow-ups. Live owner
progression belongs to `host`, `model`, `measurement` and `promotion.next_blocks`;
its next leaf is `STD-NET-DOC-001`. Native ABI/AOT and complete API conformance/promotion
remain outside these hosted/test boundaries.

The verified hosted performance baseline is
[stdlib-net-performance.md](stdlib-net-performance.md), with 21 controlled
loopback routes and 27 retained samples per route. It measures admitted private
host calls and owner polling, without bytecode VM or native runtime/AOT timing.

Public hosted conformance is verified in
[stdlib-net-conformance.md](stdlib-net-conformance.md): four common finite
groups compare actual public VM calls with a fresh native Rust kernel process,
while real TLS, selection, listener/address and deadline observations remain
explicitly VM-only. Clean source `4b42b1da` passes its complete local capture,
current-source quality and exact-checkout CI. This is not a native Tondo
network provider or AOT claim.

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

The owner order is `STD-NET-HOST-001`, followed by `STD-NET-TEST-001`,
`STD-NET-PERF-001`, `STD-NET-CONF-001` and `STD-NET-DOC-001`. The contract can
inform `DIAG-RUNTIME-001` and `NATIVE-001`; public runtime symbols require their
own executable integration and gates.

## Executable usage guide for `std.net`

`STD-NET-DOC-001` qualifies usage of the production hosted provider. Tondo is
an unpublished draft. These examples use the public compiler and ordinary
hosted CLI; they do not establish native networking ABI/AOT or portable target
promotion. The actual networking contract and signatures appear above.

### Capabilities and explicit configuration

Use an ordinary `tondo.toml` project. Its selected `network` capability requires
an explicit resolver list; `clock` permits the deadline example and `console`
permits its terminal output. The complete ordered resolver list enters build
identity. No program environment variable or system resolver supplies it.

<!-- net-project -->
~~~toml
[package]
name = "net_usage"

[target]
name = "tondo-vm-hosted"
profile = "hosted"
capabilities = ["clock", "console", "network"]

[target.network]
resolver_servers = ["127.0.0.1:9"]
~~~

`127.0.0.1:9` is an unused endpoint in this closed loopback example, not a
default DNS service. The only resolutions below fail validation or use an
already expired deadline, before DNS I/O. Replace the list with deliberately
chosen reachable DNS servers when resolving real names. Both A and AAAA use
one declared endpoint; endpoint selection rotates between explicit calls.

Importing `std.net` requires `network` even for address values. Import alone
starts no I/O. Configuring an endpoint alone neither selects the capability
nor starts an operation. Missing capabilities produce `E1008`.

<!-- net-imports -->
~~~tondo
import std.bytes
import std.console
import std.io
import std.net
import std.time
~~~

### Values and keys

`HostName` preserves the caller's ASCII spelling, including case and a trailing
dot. It performs no IDNA conversion or case normalization. Convert IDNA before
calling `hostName`. `IpAddress.parse` accepts numeric IPv4/IPv6, never DNS.
These immutable address values are `Copy`, `Send`, `Share` and `Key`.

Port zero requests an assigned port only for `listen` or `bind`; connecting,
sending and resolving require a positive port. `localAddress()` observes the
assigned address without accepting traffic or consuming the owner.

<!-- net-doc:values-and-keys -->
~~~tondo
fn valuesAndKeys(): !net.NetError {
    let host = net.hostName("EXample.test.")?
    let hosts: Map[net.HostName, Int] = [host: 1]
    assert(hosts[host] == some(1))
    assert(host != net.hostName("example.test.")?)
    let ip = net.IpAddress.parse("127.0.0.1")?
    let addresses: Map[net.SocketAddress, Int] = [net.socketAddress(ip, 0)?: 2]
    assert(addresses[net.socketAddress(ip, 0)?] == some(2))
    match net.IpAddress.parse("example.test") {
        err(net.NetError.InvalidAddress) => ()
        _ => panic("IP parsing must not perform DNS")
    }
    match net.NetLimits.create(0, 1, 1) {
        err(net.NetError.InvalidLimit) => ()
        _ => panic("zero limit admitted")
    }
}
~~~

### TCP ownership and partial I/O

Register terminal cleanup immediately after acquiring each affine handle.
`TcpStream.split(stream)` consumes the stream; each half needs its own close.
An early `?` still runs every registered `defer`. A stream is not `Copy` or
`Share`; transfer ownership to a task rather than implicitly sharing a handle.

TCP is a byte stream. A successful write may accept a prefix, so advance by
the returned length and submit the remainder. Reads may return fewer bytes
than requested; `ReadResult.Eof` is separate from a data chunk. A nonempty
successful write makes positive progress. The example uses one-byte reads
and verifies EOF after the writer's explicit shutdown.

`flush` hands the provider buffer to the transport; it does not promise remote
application processing. Writer shutdown emits FIN when the host supports it;
terminal close retires the local owner. The last half close frees the transport.

<!-- net-doc:tcp-roundtrip -->
~~~tondo
fn tcpRoundtrip(): !(bytes.BytesError | net.NetError) {
    let ip = net.IpAddress.parse("127.0.0.1")?
    let options = net.options(none, net.NetLimits.defaults())?
    let listener = net.listen(net.socketAddress(ip, 0)?, 1)?
    defer net.TcpListener.close(listener)
    let destination = listener.localAddress()?
    let client = net.connect(destination, options)?
    let (reader, writer) = net.TcpStream.split(client)
    defer net.TcpReadHalf.close(reader)
    defer net.TcpWriteHalf.close(writer)
    let server = listener.accept(options)?
    let (peerReader, peerWriter) = net.TcpStream.split(server)
    defer net.TcpReadHalf.close(peerReader)
    defer net.TcpWriteHalf.close(peerWriter)
    let payload = bytes.Bytes("hello")?
    var written = 0
    for written < payload.length() {
        let suffix = payload.slice(written, payload.length())?
        let count = writer.write(suffix, options)?
        assert(count > 0)
        written += count
    }
    writer.flush(options)?
    var received = 0
    for received < payload.length() {
        match peerReader.read(1, options)? {
            io.ReadResult.Data(chunk) => {
                assert(chunk.equal(payload.slice(received, received + 1)?))
                received += 1
            }
            io.ReadResult.Eof => panic("early EOF")
        }
    }
    writer.shutdown(options)?
    match peerReader.read(1, options)? {
        io.ReadResult.Eof => ()
        _ => panic("shutdown lost EOF")
    }
}
~~~

### UDP message boundaries

UDP sends and receives whole datagrams. An oversized received datagram is
discarded and returns `DatagramTooLarge`; no truncated message is published.
Increasing the limit later cannot restore that discarded packet. The following
empty and valid datagrams still make progress. An empty datagram is data, not
stream EOF, and `Datagram.source()` identifies its sender.

UDP does not promise delivery, ordering, uniqueness or automatic retries.

<!-- net-doc:atomic-datagrams -->
~~~tondo
fn atomicDatagrams(): !(bytes.BytesError | net.NetError) {
    let address = net.socketAddress(net.IpAddress.parse("127.0.0.1")?, 0)?
    let sender = net.bind(address)?
    defer net.UdpSocket.close(sender)
    let receiver = net.bind(address)?
    defer net.UdpSocket.close(receiver)
    let destination = receiver.localAddress()?
    let sendOptions = net.options(none, net.NetLimits.defaults())?
    let readOptions = net.options(none, net.NetLimits.create(8, 4, 3)?)?
    sender.sendTo(bytes.Bytes("12345")?, destination, sendOptions)?
    match receiver.receiveFrom(readOptions) {
        err(net.NetError.DatagramTooLarge) => ()
        _ => panic("partial datagram published")
    }
    sender.sendTo(bytes.Bytes("")?, destination, sendOptions)?
    let empty = receiver.receiveFrom(readOptions)?
    assert(empty.source() == sender.localAddress()?)
    assert(empty.bytes().length() == 0)
    sender.sendTo(bytes.Bytes("ok")?, destination, sendOptions)?
    let next = receiver.receiveFrom(readOptions)?
    assert(next.bytes().equal(bytes.Bytes("ok")?))
}
~~~

### Errors, deadlines and cancellation

Errors are values: propagate them with `?` or inspect their nominal variant.
Argument validation precedes cancellation, deadline and provider outcomes.
The invalid port below is refused before an expired deadline is considered.

`NetOptions` is an immutable snapshot. `none` requests no deadline and may
wait indefinitely; it installs no ambient timeout. `some(instant)` uses an
explicit monotonic `Instant` from the selected clock. A deadline covers the
whole operation, including DNS, provider queue, TLS handshake or I/O. Cleanup
finishes before a timeout is published. Cross-clock inputs are rejected.

<!-- net-doc:errors-and-deadlines -->
~~~tondo
fn errorsAndDeadlines(): !(time.ClockError | net.NetError) {
    let options = net.options(some(time.now()?), net.NetLimits.defaults())?
    let host = net.hostName("example.test")?
    match net.resolve(host, 0, options) {
        err(net.NetError.InvalidPort) => ()
        _ => panic("validation lost precedence")
    }
    match net.resolve(host, 443, options) {
        err(net.NetError.Timeout) => ()
        _ => panic("expired deadline did not refuse")
    }
}
~~~

Cancellation uses Tondo's ordinary structured task scopes. Direct suspension
waits implicitly; `spawn` returns a `Join`, which is consumed by `await`.
Cancellation observed before commit retires an operation without publishing
partial state. Progress already committed remains the result even if later
cancelled. Cleanup also applies to error, panic and abandoned execution.

### Selection keeps losing data

`TcpListener.accept`, `TcpReadHalf.read` and `UdpSocket.receiveFrom` are
`selectable`. Losing an arm unregisters its wait without taking a connection,
byte chunk or datagram. A timeout or cancellation before commit obeys the same
ownership rule. The following ordinary program consumes the winner and then
checks that the other socket still delivers its complete message.

<!-- net-doc:selection-keeps-datagrams -->
~~~tondo
fn selectionKeepsDatagrams(): !(bytes.BytesError | net.NetError) {
    let address = net.socketAddress(net.IpAddress.parse("127.0.0.1")?, 0)?
    let options = net.options(none, net.NetLimits.defaults())?
    let sender = net.bind(address)?
    defer net.UdpSocket.close(sender)
    let first = net.bind(address)?
    defer net.UdpSocket.close(first)
    let second = net.bind(address)?
    defer net.UdpSocket.close(second)
    let payload = bytes.Bytes("x")?
    sender.sendTo(payload, first.localAddress()?, options)?
    sender.sendTo(payload, second.localAddress()?, options)?
    let winner = select {
        let result = first.receiveFrom(options) => {
            assert(result?.bytes().equal(payload))
            0
        }
        let result = second.receiveFrom(options) => {
            assert(result?.bytes().equal(payload))
            1
        }
    }
    let retained = if winner == 0 {
        second.receiveFrom(options)?
    } else {
        first.receiveFrom(options)?
    }
    assert(retained.bytes().equal(payload))
}
~~~

### DNS and TLS

`resolve(host, port, options)` returns ordered, deduplicated socket addresses.
Exceeding `maxResults` rejects the entire result, rather than exposing a partial
list. DNS uses only declared numeric endpoints: no system fallback, cache,
search domain, hosts file, hidden retry, failover or implicit TCP fallback.
The provider's five-second response bound is `ResolveFailed`; an earlier
explicit deadline is `Timeout`. Connecting a numeric socket address never
performs another resolution or attempts other addresses automatically.

`PlatformRoots` uses the target's fixed Mozilla root bundle, not a mutable
system trust store. An explicit DER pin still requires normal certificate
validity, chain, server-name and handshake verification. TLS 1.2 and 1.3 are
supported by the selected hosted provider with its declared suites.

The executable guide validates TLS configuration only:

<!-- net-doc:tls-configuration -->
~~~tondo
fn tlsConfiguration(): !(bytes.BytesError | net.TlsError) {
    _ = net.tlsConfig(net.TlsVerification.PlatformRoots)?
    match net.tlsConfig(net.TlsVerification.PinnedCertificate(bytes.Bytes("")?)) {
        err(net.TlsError.InvalidCertificate) => ()
        _ => panic("invalid certificate admitted")
    }
}
~~~

For a connection, `TlsStream.connect(stream, server, config, options)` consumes
the acquired TCP stream. Failure closes that transport, so the caller must not
also retain a deferred TCP close for the consumed value. On success, split the
TLS stream and register cleanup for both TLS halves. TLS reads and writes obey
partial-I/O rules; writer shutdown attempts `close_notify`, while cancellation
may close directly. TLS halves are suspendible and are not `selectable`.

Real TLS 1.2/1.3 handshakes, wrong-name refusal, peer-observed EOF and DNS
responses against a controlled resolver belong to the separate public
conformance fixtures. This guide does not call an external service or claim a
TLS handshake from the configuration-only example.

### Limits and costs

`NetLimits.defaults()` selects 64 KiB reads, 65,535-byte datagrams and 128 DNS
results. Explicit limits must be positive; target ceilings are 64 MiB per read,
65,535 bytes per datagram and 1,024 DNS results. Remaining runtime budgets can
refuse smaller requests. TLS pins and outgoing provider buffers have separate
64 KiB bounds. These limits do not measure RSS or OS allocator calls.

`Bytes.slice` materializes a copy in the current hosted route. The TCP example
copies each remaining suffix; do not infer a zero-copy API or throughput claim
from its short demonstration. Networking performance evidence separately
retains 27 samples for each of 21 admitted private hosted bridge routes. It
does not measure the bytecode interpreter or native networking execution.

### Executable verification

Run the checked project with `tondo check --project acceptance/projects/net-usage`
and `tondo run --project acceptance/projects/net-usage` from the repository root.
Successful execution prints exactly `net-doc-ok` and closes every acquired
handle. Repository checks use a delegated process scope and bounded command
timeouts; the example itself installs only the explicit deadline shown above.

The exact imports, TOML configuration, six functions and entrypoint are bound
to the executable project fixture. The entrypoint must call every example and
propagate failures; equal edits that skip a function are rejected.

<!-- net-doc:entrypoint -->
~~~tondo
fn main(): !(bytes.BytesError | net.NetError | net.TlsError | time.ClockError) {
    valuesAndKeys()?
    tcpRoundtrip()?
    atomicDatagrams()?
    errorsAndDeadlines()?
    selectionKeepsDatagrams()?
    tlsConfiguration()?
    _ = console.println("net-doc-ok")
}
~~~

The guide checker refuses changed fragments or capabilities. Public CLI
checks demonstrate the three missing-capability `E1008` cases and the affine
listener diagnostic. Running a deliberately wrong assertion must fail, so
matching documentary text alone cannot establish executable correctness.

### Promotion boundary

This usage owner inherits independently qualified HOST, TEST, PERF and CONF
prerequisites. Its quality and publication gates remain separate. The boundary
is the actual production hosted VM on Linux x86_64, with controlled loopback
providers and no external service. Native networking ABI/AOT, unexecuted
portable targets, SIMD and code size are not promoted. Full six-dimension
normative requirement trace is not inferred from this guide.

The next owner after verified usage is `STD-LOG-IMPL-001`.
