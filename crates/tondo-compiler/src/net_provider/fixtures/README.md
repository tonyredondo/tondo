# Local TLS fixtures

`localhost-cert.pem` and `localhost-test-key.pem` are a self-signed RSA test
identity, created only for this repository's in-memory TLS and loopback tests.
The private key is public test data and must never be used for a real service.
The certificate names `localhost`, has CA=false, and is valid from
2026-10-06 21:25:55 UTC through 2036-10-03 21:25:55 UTC.

Tests supply a fixed validation time inside this interval and also exercise
expired certificates. They require neither OpenSSL nor an external network.
These files are not part of the production root bundle.
