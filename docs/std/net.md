# std.net

TCP connections and listeners, with TLS, UDP sockets and names, on the runtime's network. Every operation waits like any call, without holding a worker, and takes a timeout of its own; there is none unless one is given.

```tondo
import std.console
import std.net

let connection = net.connect("example.com", 80)?
connection.write("HEAD / HTTP/1.0\r\nHost: example.com\r\n\r\n")?
console.println(connection.readLine()?.unwrapOr(""))
connection.close()?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.net` shows the same in a terminal.

- [Functions](#functions): [`unixAddress`](#unixaddress), [`address`](#address), [`ip`](#ip), [`network`](#network), [`resolve`](#resolve), [`connect`](#connect), [`connectUnix`](#connectunix), [`connectTls`](#connecttls), [`listen`](#listen), [`listenUnix`](#listenunix), [`bind`](#bind), [`bindUnix`](#bindunix)
- [NetError](#neterror)
- [Address](#address-1)
- [Ip](#ip-1): [`Ip.isV4`](#ipisv4), [`Ip.isV6`](#ipisv6), [`Ip.isLoopback`](#ipisloopback), [`Ip.isPrivate`](#ipisprivate), [`Ip.isUnspecified`](#ipisunspecified)
- [Network](#network-1): [`Network.contains`](#networkcontains), [`Network.ip`](#networkip), [`Network.prefix`](#networkprefix)
- [Stream](#stream): [`Stream.startTls`](#streamstarttls), [`Stream.read`](#streamread), [`Stream.readLine`](#streamreadline), [`Stream.readAll`](#streamreadall), [`Stream.readText`](#streamreadtext), [`Stream.write`](#streamwrite), [`Stream.writeBytes`](#streamwritebytes), [`Stream.closeWrite`](#streamclosewrite), [`Stream.close`](#streamclose), [`Stream.reader`](#streamreader), [`Stream.writer`](#streamwriter), [`Stream.localAddress`](#streamlocaladdress), [`Stream.remoteAddress`](#streamremoteaddress)
- [Tls](#tls): [`Tls.files`](#tlsfiles), [`Tls.pem`](#tlspem), [`Tls.from`](#tlsfrom)
- [TlsVersion](#tlsversion)
- [Certificates](#certificates): [`Certificates`](#certificates-1), [`Certificates.set`](#certificatesset), [`Certificates.setForProtocol`](#certificatessetforprotocol), [`Certificates.removeForProtocol`](#certificatesremoveforprotocol), [`Certificates.isServing`](#certificatesisserving)
- [Listener](#listener): [`Listener.address`](#listeneraddress), [`Listener.accept`](#listeneraccept), [`Listener.close`](#listenerclose)
- [Socket](#socket): [`Socket.address`](#socketaddress), [`Socket.sendTo`](#socketsendto), [`Socket.receive`](#socketreceive), [`Socket.close`](#socketclose)

### Functions

#### unixAddress

```tondo
pub fn unixAddress(path: String): Address
```

The address of the Unix socket at `path`, for `sendTo`.

#### address

```tondo
pub fn address(text: String, port: Int): Address ! NetError
```

The address of an IP address, in its usual form, and a port.

#### ip

```tondo
pub fn ip(text: String): Ip ! NetError
```

The address `text` writes, in its usual forms; an IPv4 in an IPv6, as
`::ffff:10.0.0.1`, is that IPv4.

#### network

```tondo
pub fn network(text: String): Network ! NetError
```

The network `text` writes in CIDR notation, as `10.0.0.0/8` or
`fd00::/8`; an address alone is the network of that address.

#### resolve

```tondo
pub fn resolve(host: String, timeout: time.Duration? = none): Array[String] ! NetError
```

The IP addresses of a name, in the order the system prefers them.

#### connect

```tondo
pub fn connect(host: String, port: Int, timeout: time.Duration? = none): Stream ! NetError
```

A connection to `host`, a name or an IP address, trying each of its
addresses until one answers.

#### connectUnix

```tondo
pub fn connectUnix(path: String, timeout: time.Duration? = none): Stream ! NetError
```

A connection to the Unix socket at `path`.

#### connectTls

```tondo
pub fn connectTls(
    host: String,
    port: Int,
    trust: String? = none,
    identity: Tls? = none,
    timeout: time.Duration? = none,
): Stream ! NetError
```

A connection over TLS, which verifies that the server's certificate is
for `host` and comes from a root the system trusts or an authority of
`trust`, the path of a PEM file or its text. `identity` is the
certificate and key it shows to a server that asks for one.

#### listen

```tondo
pub fn listen(host: String, port: Int, tls: Tls? = none): Listener ! NetError
```

A listener on `host` and `port`, port 0 for any free one, which
`address()` tells. With `tls`, each connection does its handshake
before its first read or write, whose error it is if it fails.

#### listenUnix

```tondo
pub fn listenUnix(path: String): Listener ! NetError
```

A listener on the Unix socket at `path`, which it makes, replacing one
nobody answers on any more, and `close` removes.

#### bind

```tondo
pub fn bind(host: String, port: Int): Socket ! NetError
```

A socket on `host` and `port`, port 0 for any free one.

#### bindUnix

```tondo
pub fn bindUnix(path: String? = none): Socket ! NetError
```

A socket of datagrams on the Unix socket at `path`, which it makes,
replacing one nobody answers on any more, and `close` removes; without
`path`, one without a name, which only sends.

### NetError

```tondo
pub enum NetError {
    /// The name does not resolve: the name.
    NotFound(String)
    /// Nothing listens at the address.
    Refused
    /// The operation took longer than its timeout.
    TimedOut
    /// The other side ended the connection abruptly.
    Reset
    /// The connection, listener or socket is closed.
    Closed
    /// Another socket has the address.
    AddressInUse
    /// No route reaches the network or the host.
    Unreachable
    /// An address or port that is not valid here: why.
    InvalidAddress(String)
    /// The TLS handshake failed, or a certificate is not trusted: why.
    Tls(String)
    /// Anything else the system reports: what it said.
    Io(String)
}
```

Why a network operation failed.

```tondo
import std.console
import std.net
import std.time

match net.connect("127.0.0.1", 9, timeout: time.seconds(1)) {
    ok(stream) => stream.close()?
    err(net.NetError.Refused) => console.println("nothing listens on port 9")
    err(net.NetError.TimedOut) => console.println("no answer in a second")
    err(error) => console.println("{error}")
}
```

Implements `Display`.

### Address

```tondo
pub type Address = {
    ip: String
    port: Int
    path: String
}
```

An IP address and a port, or the path of a Unix socket, with `ip` and
`port` empty; a TCP or UDP address has `path` empty.

```tondo
import std.console
import std.net

let listener = net.listen("127.0.0.1", 0)?
let address = listener.address()
console.println("listening on {address.ip}:{address.port}")
let dns = net.address("8.8.8.8", 53)?
console.println("{dns.ip} {dns.port} {net.unixAddress("/tmp/app.sock").path}")
```

Implements `Display`.

### Ip

```tondo
pub type Ip = {
    // private fields
}
```

An IP address, v4 or v6, in its canonical form: two ways of writing the
same address are equal, and it can be a key.

```tondo
import std.console
import std.net

for text in ["10.0.0.7", "::1", "2001:db8::0:1"] {
    let address = net.ip(text)?
    console.println("{address} v4 {address.isV4()} private {address.isPrivate()} loopback {address.isLoopback()}")
}
```

Implements `Display`.

#### Ip.isV4

```tondo
pub fn Ip.isV4(self): Bool
```

Whether it is an IPv4 address.

#### Ip.isV6

```tondo
pub fn Ip.isV6(self): Bool
```

Whether it is an IPv6 address.

#### Ip.isLoopback

```tondo
pub fn Ip.isLoopback(self): Bool
```

Whether it is this machine's: `127.0.0.0/8` or `::1`.

#### Ip.isPrivate

```tondo
pub fn Ip.isPrivate(self): Bool
```

Whether it is of a network for private use: `10.0.0.0/8`, `172.16.0.0/12`,
`192.168.0.0/16` (RFC 1918) or `fc00::/7` (RFC 4193).

#### Ip.isUnspecified

```tondo
pub fn Ip.isUnspecified(self): Bool
```

Whether it is the address of no machine: `0.0.0.0` or `::`.

### Network

```tondo
pub type Network = {
    // private fields
}
```

A network of IP addresses: an address and how many of its first bits its
addresses share, as `10.0.0.0/8` writes it.

```tondo
import std.console
import std.net

let office = net.network("192.168.10.0/24")?
for text in ["192.168.10.42", "192.168.11.1"] {
    console.println("{text} in {office.ip()}/{office.prefix()}: {office.contains(net.ip(text)?)}")
}
```

Implements `Display`.

#### Network.contains

```tondo
pub fn Network.contains(self, member: Ip): Bool
```

Whether the address is of the network.

#### Network.ip

```tondo
pub fn Network.ip(self): Ip
```

Its first address.

#### Network.prefix

```tondo
pub fn Network.prefix(self): Int
```

How many of the first bits its addresses share.

### Stream

```tondo
pub type Stream = {
    // private fields
}
```

A TCP connection, a handle its copies share. Reads and writes go
through `reader()` and `writer()`, so the two ways can be mixed; a
write returns once the connection holds the bytes, and they go out in
order while the program goes on.

```tondo
import std.console
import std.net

fn echo(listener: net.Listener): !net.NetError {
    let client = listener.accept()?
    let line = client.readLine()?.unwrapOr("")
    client.write("echo: {line}\n")?
    client.close()?
}

let listener = net.listen("127.0.0.1", 0)?
let server = spawn echo(listener)
let stream = net.connect("127.0.0.1", listener.address().port)?
stream.write("hello\n")?
console.println(stream.readLine()?.unwrapOr(""))
stream.close()?
server.wait()?
```

#### Stream.startTls

```tondo
pub fn Stream.startTls(
    self,
    host: String,
    trust: String? = none,
    identity: Tls? = none,
    timeout: time.Duration? = none,
): Stream ! NetError
```

The connection over TLS from here on, as a client of `host`, verified
as `connectTls` verifies it: for protocols that start plain and ask for
TLS, as PostgreSQL, MySQL and SMTP do. The plain stream is closed; what
it held unread or unsent is lost, so TLS starts when the protocol says,
with nothing in flight.

#### Stream.read

```tondo
pub fn Stream.read(
    self,
    max: Int = 65536,
    timeout: time.Duration? = none,
): Array[Byte]? ! NetError
```

Up to `max` bytes, as soon as some arrive, or `none` at the end.

#### Stream.readLine

```tondo
pub fn Stream.readLine(self, timeout: time.Duration? = none): String? ! NetError
```

The next line without its `\n` or `\r\n`, or `none` at the end.

#### Stream.readAll

```tondo
pub fn Stream.readAll(self, timeout: time.Duration? = none): Array[Byte] ! NetError
```

Everything until the other side ends the connection.

#### Stream.readText

```tondo
pub fn Stream.readText(self, timeout: time.Duration? = none): String ! NetError
```

Everything until the other side ends the connection, as UTF-8 text.

#### Stream.write

```tondo
pub fn Stream.write(self, text: String, timeout: time.Duration? = none): !NetError
```

Writes the text, waiting at most `timeout` when there is one.

#### Stream.writeBytes

```tondo
pub fn Stream.writeBytes(self, data: Array[Byte], timeout: time.Duration? = none): !NetError
```

Writes the bytes, waiting at most `timeout` when there is one.

#### Stream.closeWrite

```tondo
pub fn Stream.closeWrite(self, timeout: time.Duration? = none): !NetError
```

Sends what is pending and ends the writing direction: the other side
reads the end, and can still answer.

#### Stream.close

```tondo
pub fn Stream.close(self, timeout: time.Duration? = none): !NetError
```

Sends what is pending and closes the connection. The last copy of a
stream that goes away closes it too, sending what is pending on its own.

#### Stream.reader

```tondo
pub fn Stream.reader(self): io.Reader
```

The connection's reads as a stream of `std.io`, for `csv`, `json` or
`readLine` without timeouts.

#### Stream.writer

```tondo
pub fn Stream.writer(self): io.Writer
```

The connection's writes as a stream of `std.io`.

#### Stream.localAddress

```tondo
pub fn Stream.localAddress(self): Address
```

This end's address and port.

#### Stream.remoteAddress

```tondo
pub fn Stream.remoteAddress(self): Address
```

The other end's address and port.

### Tls

```tondo
pub type Tls = {
    certificate: String
    privateKey: String
    clients: String?
    minimumVersion: TlsVersion
}
```

What TLS shows and asks: a certificate chain and its private key, each
the path of a PEM file or its text; for a server, the authorities whose
certificates it requires of its clients, as mutual TLS does; and the
lowest version of TLS it accepts.

```tondo
import std.net

let tls = net.Tls.files("cert.pem", "key.pem", minimumVersion: net.TlsVersion.Tls13)
let listener = net.listen("0.0.0.0", 8443, tls: some(tls))?
for {
    let client = listener.accept()?
    client.write("hello over TLS\n")?
    client.close()?
}
```

#### Tls.files

```tondo
pub fn Tls.files(
    certificate: String,
    privateKey: String,
    clients: String? = none,
    minimumVersion: TlsVersion = TlsVersion.Tls12,
): Tls
```

A certificate chain and its private key in PEM files; `clients` names
one of the authorities of the clients a server requires certificates
of.

#### Tls.pem

```tondo
pub fn Tls.pem(
    certificate: String,
    privateKey: String,
    clients: String? = none,
    minimumVersion: TlsVersion = TlsVersion.Tls12,
): Tls
```

A certificate chain and its private key as PEM text, as an environment
variable or `embed.text` holds them; `clients` is the PEM of the
authorities of the clients a server requires certificates of.

#### Tls.from

```tondo
pub fn Tls.from(
    certificates: Certificates,
    clients: String? = none,
    minimumVersion: TlsVersion = TlsVersion.Tls12,
): Tls
```

A server's TLS that shows `certificates`, which can change while it
serves.

### TlsVersion

```tondo
pub enum TlsVersion {
    Tls12
    Tls13
}
```

A version of TLS.

```tondo
import std.console
import std.net

let modern = net.Tls.pem("-----BEGIN CERTIFICATE-----...", "-----BEGIN PRIVATE KEY-----...", minimumVersion: net.TlsVersion.Tls13)
let version = match modern.minimumVersion {
    net.TlsVersion.Tls12 => "1.2"
    net.TlsVersion.Tls13 => "1.3"
}
console.println("clients need TLS {version} or later")
```

### Certificates

```tondo
pub type Certificates = {
    // private fields
}
```

Certificates a server shows that can change while it serves, so a
renewed one goes in without a restart: `net.listen(host, 443, tls:
net.Tls.from(certificates))`. Until one is set, handshakes fail.

```tondo
import std.fs
import std.net

// Certificates the server can change while it serves, as a renewal does.
let certificates = net.Certificates()
certificates.set(fs.readText("cert.pem")?, fs.readText("key.pem")?)?
let listener = net.listen("0.0.0.0", 8443, tls: some(net.Tls.from(certificates)))?
// Later, with a renewed certificate:
certificates.set(fs.readText("renewed.pem")?, fs.readText("renewed-key.pem")?)?
listener.close()
```

#### Certificates

```tondo
pub fn Certificates(protocols: Array[String] = []): Certificates
```

New certificates. `protocols` are the ones a client may ask for to be
shown a certificate of its own, as an ACME validation asks for
`acme-tls/1`, which servers that show these offer.

#### Certificates.set

```tondo
pub fn Certificates.set(self, certificate: String, privateKey: String): !NetError
```

The certificate chain and private key shown from now on, each the path
of a PEM file or its text.

#### Certificates.setForProtocol

```tondo
pub fn Certificates.setForProtocol(
    self,
    protocol: String,
    name: String,
    certificate: String,
    privateKey: String,
): !NetError
```

The certificate shown to a client that asks for `protocol` and the
server `name`, instead of the others.

#### Certificates.removeForProtocol

```tondo
pub fn Certificates.removeForProtocol(self, protocol: String, name: String)
```

Forgets the certificate for `protocol` and `name`.

#### Certificates.isServing

```tondo
pub fn Certificates.isServing(self): Bool
```

Whether a server shows them.

### Listener

```tondo
pub type Listener = {
    // private fields
}
```

Accepts TCP connections. Iterating it gives each new connection until
it is closed; `spawn detached handle(connection)` serves each in a task
of its own.

```tondo
import std.console
import std.net
import std.time

fn serve(client: net.Stream) {
    _ = client.write("hello\n")
    _ = client.close()
}

let listener = net.listen("127.0.0.1", 0)?
console.println("port {listener.address().port}")
for {
    match listener.accept(timeout: time.milliseconds(100)) {
        ok(client) => spawn detached serve(client)
        err(net.NetError.TimedOut) => break
        err(error) => fail error
    }
}
listener.close()
```

Implements `Iterator[Stream]`.

#### Listener.address

```tondo
pub fn Listener.address(self): Address
```

The address and port it accepts connections at.

#### Listener.accept

```tondo
pub fn Listener.accept(self, timeout: time.Duration? = none): Stream ! NetError
```

The next connection.

#### Listener.close

```tondo
pub fn Listener.close(self)
```

Stops accepting: waiting `accept` calls fail with `Closed`, and
iterations end.

### Socket

```tondo
pub type Socket = {
    // private fields
}
```

A UDP socket. Datagrams arrive whole or not at all, in any order, at
most once.

```tondo
import std.console
import std.net

let server = net.bind("127.0.0.1", 0)?
let client = net.bind("127.0.0.1", 0)?
client.sendTo("ping".bytes(), server.address())?
let (data, from) = server.receive()?
server.sendTo("pong".bytes(), from)?
let (reply, _) = client.receive()?
console.println("{String.fromBytes(data)?} {String.fromBytes(reply)?}")
```

#### Socket.address

```tondo
pub fn Socket.address(self): Address
```

The address and port the socket is bound to.

#### Socket.sendTo

```tondo
pub fn Socket.sendTo(self, data: Array[Byte], to: Address): !NetError
```

Sends one datagram of at most 65 535 bytes.

#### Socket.receive

```tondo
pub fn Socket.receive(self, timeout: time.Duration? = none): (Array[Byte], Address) ! NetError
```

The next datagram, whole, and who sent it.

#### Socket.close

```tondo
pub fn Socket.close(self)
```

Closes the socket: waiting `receive` calls fail with `Closed`.
