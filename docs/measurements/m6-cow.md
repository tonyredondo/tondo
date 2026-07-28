# M6 collection copy-on-write measurement

**Status:** accepted for OPT-COW-001 and OPT-COW-002

**Date:** 2026-07-28

**Scope:** bootstrap VM, source-to-bytecode-to-runtime

## Reproduction

~~~bash
CARGO_INCREMENTAL=0 cargo test --locked -p tondo-compiler \
  bytecode::lower::tests::collection_copy_profile_justifies_cow_with_reproducible_workloads \
  -- --nocapture
~~~

The test compiles and executes three representative read-heavy Tondo programs.
Each performs 65 logical collection copies through ordinary language
operations. Counters are deterministic VM work units; compilation time and
wall-clock noise are excluded.

| Workload | Elements | Logical copies | Eager elements traversed | COW elements traversed | COW buffer shares |
|---|---:|---:|---:|---:|---:|
| `Array[Int]` numeric readings | 256 | 65 | 16,640 | 0 | 65 |
| `Map[Int, Int]` lookup table | 128 | 65 | 8,320 | 0 | 65 |
| `Set[Int]` membership set | 128 | 65 | 8,320 | 0 | 65 |
| **Total** | — | **195** | **33,280** | **0** | **195** |

The measured read-heavy paths avoid all 33,280 top-level element traversals.
They still allocate one managed logical wrapper per copy, so object identity,
GC tracing, and source-level value semantics remain unchanged.

## Adoption boundary

COW is enabled only when every stored shallow value is scalar, immutable
`String`, or `Ref`: `T` for `Array[T]`, both `K` and `V` for `Map[K, V]`, and
`K` for `Set[K]`.

Other compounds continue through the eager recursive walker. Eligible
collection leaves inside those compounds may share independently. Before a
write, `is_unique` and `Arc::make_mut` separate shared storage. Heap limits
continue to charge full logical capacity per owner, preserving the eager
worst-case memory bound.

## Semantic gate

The eight existing `tests/runtime/value-copy/` programs run against both eager
and COW, with normal GC and with collection requested from the first
allocation. The oracle compares only language observables; a separate test
requires actual COW sharing and detachment. This is the acceptance condition
for OPT-COW-003.
