# std.sync VM/native conformance

This contract closes STD-SYNC-CONF-001. It compares the hosted VM fixture
with a fresh-process execution of the current native std.sync bridge using the
same eight logical cases. The machine-readable authority is
testing/stdlib-sync-conformance.json.

The VM fixture is the source-level oracle. It executes the complete sync
surface already exposed by the bootstrap module: Mutex and RwLock guards,
Condition wait/notify, Semaphore permits, Once retry and memoization, Barrier
generations, Atomic memory orders, and a small collection consumer. The
complete source-level surface is therefore covered by the hosted oracle,
while the native lane remains a deliberately smaller bridge proof. The
fixture also emits the static threads case; the companion compile-fail
fixture proves that spawn thread is rejected when the target omits that
capability.

The native probe is deliberately narrower. It exercises only the private ABI
that exists today: all valid atomic orderings and invalid combinations,
linearizable compare-exchange workers, epoch parking with timeout/wake-one/
wake-all, opaque-handle reclamation, the atomic publication model used by
Once, epoch generations used by Barrier, native worker completion/cancellation,
and a collection handle smoke. Native locks, Once and Barrier objects are not
claimed as a public ABI; their native observations are explicitly labelled as
atomic or epoch bridge evidence until native AOT lowering exists.

The eight cases are ordered as atomic-orders, compare-exchange, parking-wakeup,
cleanup-no-poison, once-publication, barrier-generations, threads-capability,
and collection-conformance. The final collection case consumes
STD-SYNC-COLLECTION-CONF-001: the parent runner executes that child lane and
requires its verified report before accepting the delegated observation.
Consequently the sync owner cannot close while collections are merely
documented.

Every native case runs in a fresh process and requires zero live objects before
reporting success. Cooperative VM operations remain scheduler-owned and never
block an executor worker. Native workers are permitted only in the
capability-gated bridge case. Reports contain hashes and normalized
observations, never physical paths, addresses, process IDs or timestamps.
native_status means verified runtime ABI evidence; it does not claim generic
native AOT lowering or a release.

Run the lane with:

    scripts/stdlib-sync-conformance.sh

The runner writes
target/reliability/evidence/stdlib-sync-conformance.json with fixture and
probe hashes, the exact VM lines, native observations, the static rejection
result, the required collection-child status and cleanup comparison flags.
Contract mutations are exercised by
scripts/stdlib-sync-conformance-test.sh.
