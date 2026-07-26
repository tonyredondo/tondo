# Value-copy equivalence corpus

These fixtures are the representation-independent oracle for Tondo value-copy
semantics. They cover the six observable boundaries required by VALUE-002:
logical value, independence after writes, `Ref` identity, iteration, language
panic, and survival under GC pressure.

The ordinary runtime harness checks each case against its public sidecars. A
second integration test runs the same source with an initial GC threshold of
one and compares the complete driver observation. Capacity stays unchanged;
neither path can inspect heap handles, addresses, allocation counts, reference
counts, collector schedules, or whether a future runtime uses eager copies or
copy-on-write.

A candidate COW runtime must run this unchanged corpus and produce the same
observations before it can replace the eager reference implementation.
