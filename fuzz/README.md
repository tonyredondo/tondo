# Persistent fuzzing targets

This isolated Cargo workspace contains the three permanent M10.5 fuzz
boundaries:

- `frontend`: arbitrary bytes through lexer, parser, CST reconstruction and
  canonical formatting under defensive limits;
- `protocols`: manifest, lockfile, interface, artifact, privileged-unit,
  conformance-manifest and adapter protocol decoders;
- `admission`: typed-by-construction programs through HIR, MIR, bytecode and
  execution, plus direct structural mutation of the bytecode type catalog.

Corpora are versioned below `corpus/<target>/`. Crashes belong in the ordinary
regression suite after minimization; `artifacts/` is deliberately ignored.

The deterministic pull-request smoke command is:

~~~text
cargo +nightly fuzz run frontend -- -runs=1000 -seed=1001
cargo +nightly fuzz run protocols -- -runs=1000 -seed=1002
cargo +nightly fuzz run admission -- -runs=1000 -seed=1003
~~~

Nightly campaigns increase `-runs` or use a bounded `-max_total_time`; every
failure report must retain target, seed and minimized input.
