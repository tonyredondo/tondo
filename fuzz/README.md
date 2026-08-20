# Persistent fuzzing targets

This isolated Cargo workspace contains the permanent M10.5 fuzz
boundaries:

- `frontend`: arbitrary bytes through lexer, parser, CST reconstruction and
  canonical formatting under defensive limits;
- `protocols`: manifest, lockfile, interface, artifact, privileged-unit,
  conformance-manifest and adapter protocol decoders;
- `admission`: typed-by-construction programs through HIR, MIR, bytecode and
  execution, plus direct structural mutation of the bytecode type catalog;
- `stdlib_codecs`: arbitrary bytes through the bounded JSON, MessagePack and
  Protobuf readers, including one-byte fragmentation. The deterministic
  external wire oracle is kept in `crates/tondo-stdlib/tests/codec_conformance.rs`.
- `stdlib_owners`: one bounded route for every S1A standard-library owner.
  The first byte selects the owner and the remainder is the payload; every
  route has a fixed limit, an executable oracle and a versioned corpus under
  `corpus/stdlib_owners/`.

Corpora are versioned below `corpus/<target>/`. Crashes belong in the ordinary
regression suite after minimization; `artifacts/` is deliberately ignored.

The deterministic pull-request smoke command is:

~~~text
cargo +nightly fuzz run frontend -- -runs=1000 -seed=1001
cargo +nightly fuzz run protocols -- -runs=1000 -seed=1002
cargo +nightly fuzz run admission -- -runs=1000 -seed=1003
cargo +nightly fuzz run stdlib_codecs -- -runs=1000 -seed=1004
cargo +nightly fuzz run stdlib_owners -- -runs=1000 -seed=1022
~~~

Nightly campaigns increase `-runs` or use a bounded `-max_total_time`; every
failure report must retain target, seed and minimized input.
