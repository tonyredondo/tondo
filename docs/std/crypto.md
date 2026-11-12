# std.crypto

secrets compared in constant time, passwords hashed for storage, keys derived from secrets and passwords, data sealed with a secret key, signatures, and the keys and certificates of PEM files. The primitives are ring's, in the runtime; PEM, DER and X.509 are read and written here.

```tondo
import std.console
import std.crypto

let stored = crypto.hashPassword("correct horse")
console.println("{crypto.verifyPassword("correct horse", stored)}")
console.println("{crypto.equalSecrets("a".bytes(), "b".bytes())}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.crypto` shows the same in a terminal.

- [Functions](#functions): [`equalSecrets`](#equalsecrets), [`hashPassword`](#hashpassword), [`verifyPassword`](#verifypassword), [`hkdf`](#hkdf), [`pbkdf2`](#pbkdf2), [`certificates`](#certificates)
- [CryptoError](#cryptoerror)
- [Cipher](#cipher)
- [SecretKey](#secretkey): [`SecretKey.generate`](#secretkeygenerate), [`SecretKey.fromBytes`](#secretkeyfrombytes), [`SecretKey.bytes`](#secretkeybytes), [`SecretKey.seal`](#secretkeyseal), [`SecretKey.open`](#secretkeyopen)
- [Curve](#curve)
- [KeyKind](#keykind)
- [PrivateKey](#privatekey): [`PrivateKey.generate`](#privatekeygenerate), [`PrivateKey.fromPem`](#privatekeyfrompem), [`PrivateKey.pem`](#privatekeypem), [`PrivateKey.kind`](#privatekeykind), [`PrivateKey.publicKey`](#privatekeypublickey), [`PrivateKey.sign`](#privatekeysign)
- [PublicKey](#publickey): [`PublicKey.fromPem`](#publickeyfrompem), [`PublicKey.pem`](#publickeypem), [`PublicKey.kind`](#publickeykind), [`PublicKey.verify`](#publickeyverify)
- [Certificate](#certificate): [`Certificate.fromDer`](#certificatefromder)

### Functions

#### equalSecrets

```tondo
pub fn equalSecrets(a: Array[Byte], b: Array[Byte]): Bool
```

Whether two secrets are the same, in a time that depends only on their
lengths, so that how long it takes tells nothing of where they differ.
`==` on bytes, as on two `hash.Digest`s, takes such a time too.

#### hashPassword

```tondo
pub fn hashPassword(password: String): String
```

The password hashed to be stored, with Argon2id and a salt of its own,
in PHC's format: `$argon2id$v=19$m=19456,t=2,p=1$...`.

#### verifyPassword

```tondo
pub fn verifyPassword(password: String, hash: String): Bool
```

Whether `password` is the one `hash` was made from. It reads the hashes
of `hashPassword`, Argon2's of other programs, bcrypt's (`$2b$...`), and
PBKDF2 with SHA-256 as PHC and passlib (`$pbkdf2-sha256$...`) and Django
(`pbkdf2_sha256$...`) write it, so that the users of another program
keep their passwords.

#### hkdf

```tondo
pub fn hkdf(
    secret: Array[Byte],
    salt: Array[Byte] = [],
    info: Array[Byte] = [],
    length: Int = 32,
): Array[Byte]
```

Keys of `length` bytes, at most 8160, derived from a secret with HKDF
and SHA-256 (RFC 5869): one secret gives a key for each `info`.

#### pbkdf2

```tondo
pub fn pbkdf2(
    password: Array[Byte],
    salt: Array[Byte],
    iterations: Int,
    length: Int = 32,
): Array[Byte]
```

A key of `length` bytes derived from a password with PBKDF2 and
HMAC-SHA-256, `iterations` times, at least once: the more, the slower to
guess. `hashPassword` is the one to store passwords with.

#### certificates

```tondo
pub fn certificates(pem: String): Array[Certificate] ! CryptoError
```

The certificates of a PEM text, in its order, as a chain is written.

### CryptoError

```tondo
pub type CryptoError = {
    message: String
}
```

Why a key, a PEM text, a certificate or sealed data could not be used.

```tondo
import std.console
import std.crypto

let key = crypto.SecretKey.generate()
match key.open("not sealed".bytes()) {
    ok(data) => console.println("{data.length()}")
    err(error) => console.println("refused: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.

### Cipher

```tondo
pub enum Cipher {
    Aes256Gcm
    ChaCha20Poly1305
}
```

An AEAD cipher: it encrypts and authenticates.

```tondo
import std.console
import std.crypto

let key = crypto.SecretKey.generate(cipher: crypto.Cipher.ChaCha20Poly1305)
console.println("{key.bytes().length()} bytes")
```

### SecretKey

```tondo
pub type SecretKey = {
    // private fields
}
```

A secret key that seals data: encrypts it so that only its holder reads
it, and so that any change is found when it opens.

```tondo
import std.console
import std.crypto

let key = crypto.SecretKey.generate()
let sealed = key.seal("the secret".bytes(), context: "user 7".bytes())
let opened = key.open(sealed, context: "user 7".bytes())?
console.println(String.fromBytes(opened)?)
```

#### SecretKey.generate

```tondo
pub fn SecretKey.generate(cipher: Cipher = Cipher.Aes256Gcm): SecretKey
```

A new random key.

#### SecretKey.fromBytes

```tondo
pub fn SecretKey.fromBytes(
    bytes: Array[Byte],
    cipher: Cipher = Cipher.Aes256Gcm,
): SecretKey ! CryptoError
```

The key of 32 bytes kept elsewhere, as `bytes()` gave them.

#### SecretKey.bytes

```tondo
pub fn SecretKey.bytes(self): Array[Byte]
```

The key's 32 bytes, to keep it.

#### SecretKey.seal

```tondo
pub fn SecretKey.seal(self, data: Array[Byte], context: Array[Byte] = []): Array[Byte]
```

The data sealed: a random nonce of 12 bytes, the data encrypted, and a
tag of 16 that `open` checks. `context`, as an identifier the data
belongs to, is not sealed with it but must be the same to open it.

#### SecretKey.open

```tondo
pub fn SecretKey.open(
    self,
    sealed: Array[Byte],
    context: Array[Byte] = [],
): Array[Byte] ! CryptoError
```

The data that `seal` sealed with this key and `context`.

### Curve

```tondo
pub enum Curve {
    Ed25519
    P256
    P384
}
```

A curve that `PrivateKey.generate` makes keys on.

```tondo
import std.console
import std.crypto

let key = crypto.PrivateKey.generate(curve: crypto.Curve.P256)
console.println("{key.kind()}")
```

### KeyKind

```tondo
pub enum KeyKind {
    Ed25519
    EcdsaP256
    EcdsaP384
    Rsa
}
```

What signs with a key: Ed25519, ECDSA on P-256 with SHA-256 or on
P-384 with SHA-384, or RSA with PKCS #1 v1.5 and SHA-256.

```tondo
import std.console
import std.crypto

let key = crypto.PrivateKey.generate()
match key.kind() {
    crypto.KeyKind.Ed25519 => console.println("Ed25519")
    other => console.println("{other}")
}
```

Implements `Display`.

### PrivateKey

```tondo
pub type PrivateKey = {
    // private fields
}
```

A private key, which signs.

```tondo
import std.console
import std.crypto

let key = crypto.PrivateKey.generate()
let signature = key.sign("message".bytes())
console.println("{key.publicKey().verify("message".bytes(), signature)}")
console.print(key.pem())
```

#### PrivateKey.generate

```tondo
pub fn PrivateKey.generate(curve: Curve = Curve.Ed25519): PrivateKey
```

A new random key on `curve`. RSA keys are not made here: they are read
from PEM.

#### PrivateKey.fromPem

```tondo
pub fn PrivateKey.fromPem(text: String): PrivateKey ! CryptoError
```

The key of a PEM text: PKCS #8 (`PRIVATE KEY`), or RSA's PKCS #1
(`RSA PRIVATE KEY`) or SEC 1 (`EC PRIVATE KEY`), as OpenSSL writes them.
An encrypted key is decrypted first: `openssl pkcs8 -in key.pem`.

#### PrivateKey.pem

```tondo
pub fn PrivateKey.pem(self): String
```

The key in PEM, as PKCS #8.

#### PrivateKey.kind

```tondo
pub fn PrivateKey.kind(self): KeyKind
```

What the key signs with.

#### PrivateKey.publicKey

```tondo
pub fn PrivateKey.publicKey(self): PublicKey
```

The public key that verifies what this key signs.

#### PrivateKey.sign

```tondo
pub fn PrivateKey.sign(self, data: Array[Byte]): Array[Byte]
```

The signature of the data: of 64 bytes with Ed25519; with ECDSA, its
two numbers one after the other, of 64 or 96 bytes, as JWT and WebCrypto
write them; with RSA, as long as its modulus.

### PublicKey

```tondo
pub type PublicKey = {
    // private fields
}
```

A public key, which verifies what its private key signed.

```tondo
import std.console
import std.crypto

let signer = crypto.PrivateKey.generate()
let shared = crypto.PublicKey.fromPem(signer.publicKey().pem())?
console.println("{shared.verify("data".bytes(), signer.sign("data".bytes()))}")
```

#### PublicKey.fromPem

```tondo
pub fn PublicKey.fromPem(text: String): PublicKey ! CryptoError
```

The key of a PEM text: `PUBLIC KEY`, as OpenSSL writes it, RSA's
`RSA PUBLIC KEY`, or a `CERTIFICATE`, whose key it is.

#### PublicKey.pem

```tondo
pub fn PublicKey.pem(self): String
```

The key in PEM, as `PUBLIC KEY`.

#### PublicKey.kind

```tondo
pub fn PublicKey.kind(self): KeyKind
```

What the key verifies.

#### PublicKey.verify

```tondo
pub fn PublicKey.verify(self, data: Array[Byte], signature: Array[Byte]): Bool
```

Whether `signature` is this key's private key's of the data.

### Certificate

```tondo
pub type Certificate = {
    subject: String
    issuer: String
    names: Array[String]
    serialNumber: String
    validFrom: time.DateTime
    validUntil: time.DateTime
    der: Array[Byte]
}
```

An X.509 certificate: whom it names and who signed it, the names it is
for, its serial number in hexadecimal, when it is valid, and its DER.

```tondo
import std.console
import std.crypto
import std.fs

for certificate in crypto.certificates(fs.readText("server.pem")?)? {
    console.println("{certificate.subject} until {certificate.validUntil} for {certificate.names}")
}
```

#### Certificate.fromDer

```tondo
pub fn Certificate.fromDer(der: Array[Byte]): Certificate ! CryptoError
```

A certificate of DER, as a TLS connection gives it.
