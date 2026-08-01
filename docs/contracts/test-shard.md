# Stable test sharding

**Status:** implemented for `UTEST-SHARD-001`

`tondo_compiler::test_shard` owns the pure partition boundary after selection
and before execution ordering. `ShardSpec` accepts only one-based
`index/count` values with positive canonical decimal components. `ShardResult`
validates non-empty, unique visible IDs and returns only the assignments for
the requested shard in UTF-8 byte order.

The assignment is exactly:

```text
1 + uint256_be(SHA-256("tondo-test-shard-v1\\0" || UTF8(test_id))) mod count
```

The complete digest is retained for reports, and the algorithm is named
`sha256-mod-v1`. It is independent of discovery order, host platform, job
count, and random execution seed. A shard with no assigned IDs is valid when
the pre-shard selection was non-empty. `partition_all` provides the same
validation for every shard so callers can check disjointness and exact union
without adding runner state or sharing a fixture between processes.
