# Native language conformance contract

The language leaf runs the common-MIR probe through the conformance adapter for
each candidate backend. It covers a scalar return, a managed `Result` error
tag/payload and a trapped panic diagnostic. Every observation is compared with
the independent VM oracle in the probe; a missing case, changed tag or changed
diagnostic is a failure, not a tolerated backend difference.

The target is explicit (`x86_64-unknown-linux-gnu`) and reports are path-free.
This leaf does not select a backend or replace the larger G5 corpus; it proves
that the adapter and the candidate observation schema are usable for the
language surface before the testing and stdlib leaves are coordinated.
