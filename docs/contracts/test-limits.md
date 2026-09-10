# Finite test limits and phase deadlines

**Status:** public phase deadlines, instruction accounting, VM heap/frame
ownership, hosted byte/path/environment/synchronization ownership and evidence budgets are implemented.
Task, scope, select and cleanup storage is also charged. Other hosted payloads
and remaining scheduler capacity accounting remain open under
`UTEST-LIMIT-001`.

`tondo_compiler::test_limits` models a coordinator-side resource profile
for leaves and suite phases. Defaults are finite for work, memory,
depth, output, artifacts, snapshots, metadata, virtual timers, ready queues
and instructions. The host-independent profile can represent a disabled
wall-clock timeout, but the canonical CLI defaults and any sidecar always
supply a positive cap and reject `--timeout none`.

`LimitProfile::canonical_bytes` and its SHA-256 identify the effective values
without host paths or map-order dependence. The existing sealed envelope gets
its output/artifact/snapshot limits through `envelope_limits`, leaving one
conversion boundary instead of duplicate budget semantics.

## Atomic accounting

`BudgetLedger::reserve` folds duplicate dimensions, preflights every delta and
commits all charges only when every dimension fits. A failed operation leaves
all counters unchanged; zero, overflow and exhausted reservations are typed
errors. The effective profile is available for report publication.

The public envelope uses that ledger to reserve evidence work, metadata and
output before publication. Output includes messages from failures and skips,
tags, logs, stream bytes, and artifact/snapshot descriptors. Attachments and
snapshots have separate byte and count limits. Empty records still consume
work and metadata. Worker report import is one transaction; an invalid or
over-budget later record cannot publish an earlier prefix. A fixed resource
terminal remains available after exhaustion, and later cleanup messages cannot
replace it with an ordinary retryable panic.

## Executable VM accounting

The public default is 10,000,000 instructions and 64 MiB of accounted memory
per node. All cooperative children and blocking jobs share their node's
instruction counter and memory budget. Setup instruction accounting pauses
during descendant execution; teardown starts a new instruction allowance.
Sibling leaves get independent budgets. A minimum instruction limit of one
still produces a terminal report rather than failing in compiler scaffolding.

Stdout/stderr quotas count raw bytes before UTF-8/Base64 serialization. `Writer`
and JSON, MessagePack, Protobuf, YAML, Base64 and hex writer adapters route to
the active envelope. A rejected write publishes no bytes; a codec cannot
convert the runner quota into a recoverable language error. This capture
boundary is independent from the owned response accounting described below.
`Writer.write` and `io.writeAll` borrow the registered `Bytes` payload through
output admission and append; they allocate no intermediate payload copy.
`Reader.read` and `io.readAll` reserve the retained byte buffer before copying
input or advancing the hosted reader. A memory rejection preserves the cursor
and registry identity; the existing I/O size checks keep their error precedence.
The returned `Bytes` retains the reservation until its last host root is collected.
Owned I/O dispatch also prepays its closed response descriptors as described
below; general host construction remains an open boundary.

VM heap memory counts the existing estimated object payload sizes. Frame
storage is also admitted before allocating its local slots and loans: 128
logical bytes per frame plus 32 per slot and 32 per loan. These portable values
are included in the effective resource-profile hash. Charges follow parked
frames and release on return, unwind or worker destruction. Each phase's depth
counts its own loaded frames; retained ancestor frames do not reduce a child's
depth allowance. Collection during child-frame admission keeps the parked
parent roots visible, including when admission fails.

A charge stays with the allocating node until GC or heap destruction, and is released
on reclamation. A mutation preflights its full growth against that owner.
Concurrent worker reservations are atomic. Retained ancestor state remains
charged to the ancestor, so it cannot consume the child's allowance. This
measure is neither process RSS nor OS allocator capacity. Heap statistics
continue to describe heap payloads, independently of frame and host charges.

Hosted `Bytes`, `BytesBuilder`, formatting builders, `Path`, environment `Name`
and environment `Value` reserve 32 logical
descriptor bytes plus payload length. Batch byte construction admits the whole
sum before publishing any token. Builder growth reserves against its original
owner before mutation; a failed append preserves the content and charge.
Finishing a byte builder creates a separately charged copy in the calling
phase. The descriptor cost is included in the effective resource-profile hash.

An environment snapshot reserves its sealed payload bytes and a descriptor for
each embedded argument, name and value before copying those inputs. Its cache is
weak: discarding the last live reference permits collection and a later call
constructs a new snapshot. Arguments and selected values returned from a snapshot
are separately admitted copies in the calling phase; a getter does not clone
the whole snapshot. Filesystem directory listings admit their complete path
batch before publishing tokens.

The test VM traces typed host tokens through live heap objects, frames,
closures, pending arguments and completions before host calls and on memory
pressure. The host follows borrowed-view, guard, collection and channel edges,
and retains pending hosted operations. Blocking bridges publish roots for
queued and running jobs, returned values in transit and unconsumed completions.
Materialization protects both earlier VM values and host tokens in the
remaining detached input. Unreachable buffers and snapshots release their owner charge.
Ordinary non-test executions do not perform this additional tracing pass.

Hosted concurrent arrays, maps, sets, stacks and queues charge their owned
detached values: 32 bytes per value node, copied string and nominal-name bytes,
and four bytes per function type argument. Host tokens count only their local
descriptor; the referenced payload retains its own registry owner. A collection
adds 32 descriptor bytes and eight bytes per member's iteration generation.
Literal maps and sets resolve duplicate members before admitting the retained
copies. Mutation preflights replacement or growth against the original owner;
removal releases the member and generation charge. Collection tracing preserves
nested host values and reclaims unreachable generation tables.

Hosted mutexes, read/write locks, Once and atomics apply the same detached-value
accounting. Conditions, semaphores and barriers reserve a 32-byte descriptor.
Acquiring a guard or permit reserves its descriptor before changing lock
ownership, reader counts or permits. Explicit release and terminal cleanup drop
the charge. The phase collector follows guard-to-owner edges and finalizes an
unreachable guard even when its resource remains live.

VM-to-host argument batches reserve every detached copy before copying any
payload. An alias passed twice counts twice. The snapshot model charges 32 bytes
per value plus owned strings, nominal/callable names and type arguments. The
admission walk reserves 64 bytes per active frame; construction separately
reserves 32 per node and 64 per maximum data depth. Data depth is bounded at 256
for accounted test snapshots, independently of VM call depth. Cycles preserve
path markers. Admission failure releases the partial reservation; one collection
may retry it while rooting all source arguments. The resource profile binds
`value-32-workspace-32-frame-64-depth256/1` and its depth/frame units.

Synchronous and deferred dispatch retain the argument charge until the call
returns. Async dispatch retains it through host admission. Blocking submissions
retain it in the queue and through worker import, and an owned worker-to-host
request moves its payload and charge together. A borrowed worker call uses the
same bounded snapshot admission before copying. Cancellation and rejected
admission release those arguments. Importing a `String` moves its payload charge
into the heap slot and reserves only added heap metadata and capacity; the
original detached descriptors remain charged until batch import completes.
`moved-string-payload-heap-growth/1` identifies this transfer. A blocking worker
admits its complete returned snapshot before copying it. The returned value
and charge travel together through completion and remain charged after the
worker exits, until consumption or cancellation. Parent import applies the
same String payload transfer.

Synchronous host responses and delivered asynchronous completions own a
detached-value reservation in the receiving phase. It remains paired with the
result through worker reply queues, ready task queues, discard, and VM import.
Ready task completions also retain their typed host roots until consumption.
Argument snapshots keep their original account. Rejected replies
and failed imports release the response reservation, and String import moves
the admitted payload without a second charge. The effective resource profile
binds `owned-host-pending-ready-value32-account-walk64-nominal-cancel/2`. Detached
measurement keeps one 64-byte cursor per active parent, reserved before extending
its workspace. Wide collections need no per-child pending table. The same lazy
traversal traces every detached host edge without recursion or payload copies.
Measurement workspace is released before reserving the measured payload.
This admission
measures an already constructed response: host construction preflight,
remaining host-internal pending output, and general import workspace remain open boundaries.

Owned replies now preflight their complete typed VM heap graph before building
any child. The borrowed walk validates the verified scalar, aggregate, nominal,
generic-payload, opaque-witness and host-token representations. It counts every
heap object and its storage using the VM heap layout, including collection
slots and retained String capacity. The existing String payload reservation
moves into a pool for the complete heap graph; detached descriptors remain
charged through conversion. Both phase memory and heap object limits reject
before construction. A malformed child likewise rejects before earlier siblings
are allocated.

Nominal payload descriptors are specialized as complete type graphs during
bytecode admission, including a parameter nested inside an Array or other
container. The compiler retains concrete payload types of inactive variants
within its finite type-table limit. Import therefore uses already resolved
descriptors, without a second substitution or runtime type-search workspace.

Construction uses an iterative stack whose capacity is the maximum active
parent depth computed by the borrowed walk. The full capacity is admitted before
allocation, at the actual compiled import-frame size recorded in the resource
profile. Partially constructed values remain GC roots in these frames. The
stack and detached reservations are released after success or rejection;
completed heap objects retain their transferred charges. The walk uses 64 bytes
per active parent and accepts at most 256 parent levels. The profile binds
`typed-import-heap-pool-rust-frame-depth256-sync-async-worker-io-storage-bound/14`, its frame byte
size, admission descriptor byte size and depth.
Console `Input`/`Output` acquisition admits the registry entry, detached result
and complete typed VM result before publishing the handle. Immediate hosted
`read`, `readAll`, `write` and `writeAll` likewise admit the receiving result
before consuming input or emitting output. This covers both asynchronous start
and synchronous worker dispatch. The nominal `ReadResult.Data` transport costs
106 bytes, EOF costs 74, and an `IoError` result costs 71; an integer, Bytes or
Unit success costs 64. These are logical descriptor costs, not allocator calls.
Console cancellation costs 76 bytes; `readLine` EOF costs 64, an invalid-data
result costs 115, and a valid line costs 96 plus its UTF-8 byte length. These
transport reservations coexist with the complete typed VM result admission
before cursor movement or output. Repeated cancellation retains its existing
nominal response without constructing a second descriptor.
Cancellation reserves the actual nominal error shape. Rejected admission
retires the request and any unused result reservation. Terminal collection
retains only handles reachable from the detached return; a worker publishes
that graph to its parent without another budgeted cleanup request.

`fs.open` reserves its possible File/FsError result, the File host record and
path conversion storage before calling the OS. Transport retains 64 bytes for
a File result or 71 for FsError; the host record retains 32 bytes until cleanup.
Path scratch reserves the native byte length on Unix and three times the UTF-8
length elsewhere for the validation copy plus native encoding. That scratch
is released after the operation. A memory rejection cannot create or truncate
a file and publishes no File identity. This does not promise rollback of an
OS operation that fails after it starts.

`writeAll`, `createDirectory`, `remove`, `rename` and `atomicWrite` likewise
reserve their complete Unit/FsError result and path conversion storage before
effects. The Unit result retains 64 transport bytes and FsError retains 71.
Rename reserves both paths. Writes reserve their copied payload before charging
the temporary write quota. Atomic writes also reserve a 44-byte generated name
and its independent native path (parent bytes, one separator and 44 name bytes)
before charging that quota or selecting a temporary identity. Rejected memory
admission preserves files, directories and the quota. A bounded compiled test
sweeps 385 memory budgets per operation, including success, rejection and
terminal handle retirement.

File `read`, `write` and `flush` reserve their transport and complete typed VM
result before calling the OS. Read transport is 96 bytes for `some(Bytes)`,
64 for EOF, and 71 for `FsError`, including cancellation. A read reserves the
requested buffer capacity and retains that charge after a short read; EOF and
errors release it without publishing a Bytes handle. This requires neither a
seekable file nor an optimistic metadata snapshot. A phase-memory rejection
preserves the file cursor and prevents writes. Ordinary OS short I/O retains
its usual progress semantics.

An unpredictable host result can explicitly request a `StorageBound` over
1..16 non-nested typed previews. Admission reserves the maximum object count,
frame depth, structural bytes and String bytes independently, before committing
the host effect. Import still validates the declared type and every child,
rejects any exceeded bound before heap construction, transfers only actual
String bytes and releases unused capacity. This storage envelope does not
restrict returned values to a whitelist. Ordinary previews retain their exact
storage checks. These File guarantees do not establish admission for the other
filesystem entry points or close the overall T0 resource gate.

Blocking argument batches use the same graph admission and iterative
construction. The complete batch validates its slot types and payloads and
reserves all heap objects before constructing the first argument. All arguments
share one frame buffer sized for their maximum depth; completed earlier
arguments stay rooted while later arguments are built. Blocking captures may
contain verified closure environments and bytecode function references; ordinary
host replies continue to reject both. A 256-level closure capture chain remains
executable, and a 257-level chain rejects before constructing any object.
For valid-index `sync.Array.set` in a test participation, the host exposes a
borrowed preview of the previous item with its Result wrapper. Before dispatch,
the VM admits the complete additional heap graph, import workspace and admission
descriptor, and holds the required heap object slots. The host then admits
transport and retained-storage growth before replacing the item. Either
reservation failing leaves the array unchanged. This applies to ordinary
suspendible calls and spawned host tasks. The task retains the VM admission
until delivery, even when another phase becomes active; other tasks cannot use
its held object slots. Import validates the actual response against the preview
within the already reserved workspace and consumes the existing reservations.
Cancellation, provider failure and malformed delivery release unused bytes and
slots. The public regression observes suite-owned state during teardown after
a child's rejected or successful replacement.

Map.remove, Stack.pop and Queue.dequeue use the same protocol with borrowed
optional-result previews. A present item remains in its collection until VM,
transport and storage admission succeed; an absent item preadmits its None
heap object. Public tests cover exact reproduced rejection budgets, successful
removal, and nested generic payloads followed by empty results through direct
and spawned calls.

Map.insert previews both Result and Option wrappers from its borrowed previous
entry, including the None returned for a new key. The same bounded walk counts
both parent frames without constructing a provisional response. Rejection
preserves an existing value or the absence of a new key. A map at its collection
capacity declines a success preview for a new key and retains its existing
recoverable error route; replacing an existing entry remains valid.

Set.insert, Stack.push and Queue.enqueue likewise preadmit their fixed Result
heap graph before growth. Set previews the actual insertion Bool, including
false for an already present key at capacity; Stack and Queue preview Unit.
New entries beyond the collection capacity retain the recoverable error route.
Public regressions preserve the suite's length after rejection and exercise
successful and duplicate insertion through direct and spawned calls.

Array and Map compareExchange preadmit Result, the selected CompareExchange
variant, and its observed payload before committing. The enum preview resolves
the declared ordinal and single tuple field through the verified descriptor;
Map adds its separate optional wrapper without flattening an optional V.
Public tests cover Exchanged and Mismatch, nested generic values, absence,
replacement and removal through direct and spawned calls. Invalid indices and
new keys beyond map capacity retain their recoverable error routes. Borrowed
previews and actual observed copies agree on retained String capacity.

Synchronous Atomic.swap and Atomic.compareExchange now use the same
pre-dispatch admission: swap previews the previous Copy value, including
heap-backed tuples and records, and compareExchange previews its selected enum.
The synchronous caller retains the reservation locally through import or error;
it creates no pending task. Memory-order and token validation precede admission.
Direct calls, function values, helpers and defer preserve observed values; exact
rejected-budget public cases preserve suite-owned atomic storage. Host root
collection runs once before the preview and dispatch for both call modes.

Buffered channel reception also reserves the complete VM result before removing
the queue's first value. Synchronous `tryReceive` previews its `Item` variant;
suspendible `receive` and iterator reception preview the buffered value at the
poll that can commit it, after checking receiver FIFO order. Poll admission runs
with the waiting task's account even when another phase is active. A rejected
preview retires only that waiter and its request storage, without consuming the
message or allocating a cancellation reply. Accepted replies keep their pool
through direct, deferred or spawned delivery. A compiled Tondo regression checks
the actual host queue after memory rejection, independently of user `defer`
execution. Public tests cover nested generic values, FIFO results and terminal
states; `tryReceive` remains synchronous and cannot be spawned directly.

When a pending sender and receiver rendezvous, the host reserves both typed VM
results and both detached responses before moving the message or acknowledging
the send. A scoped planner resolves each pending call's result type and original
phase account. Tentative reservations hold heap bytes, import workspace and
object slots; rejecting either endpoint releases the peer's tentative
reservations without completing that peer. Batch publication validates every
recipient, account and heap identity before retaining any pool in a waiting
task. Opaque reservation descriptors include the recipient identity in their
charged size. Cancellation or failure releases only the terminated task's pool;
the other task can still import its admitted result.
The compiled regression preserves an uncommitted peer at a fixed rejected
budget. Focused tests cover separate and shared accounts, insufficient VM or
transport memory, invalid batches, cancellation and failure. Public tests carry
nested generic values with both registration and Join-consumption orders.
Pending sends into finite and unbounded buffers use the same scoped planner
before enqueueing. Their complete typed acknowledgement and detached response
are reserved before the channel admits queue growth. If that growth rejects,
the failed-call path releases the prepared import. Fixed-budget compiled
regressions preserve an empty queue after rejection and execute normally with
sufficient memory.

Synchronous `trySend` and `tryReceive` also jointly admit the active caller and
its pending peer before delivery. The scoped planner has the caller's verified
result type and original account; reservation recipients distinguish the active
caller from a pending call without reserving any numeric call ID as a sentinel.
Rejecting a peer may retire that peer and try the next FIFO waiter, with fresh
result admission for the selected payload. Rejecting the caller leaves the peer
uncommitted. Buffered `trySend` admits its acknowledgement before queue growth;
buffered `tryReceive` retains its existing pre-dispatch preview without a second
reservation. The synchronous caller takes back its pool on both success and
host failure. Tests cover shared and separate accounts, failed batches,
published-pool release, fixed-budget rejection of both transfer directions,
generic values and empty/closed results. Ordinary execution without a phase
account retains its reference dispatch route.
Channel construction and endpoint forks stage their exact detached result with
checked prospective identities, then admit the complete typed VM graph before
publishing any identity or changing endpoint counts. Receiver close previews
the final receiver's queue as two borrowed slices in FIFO order. Both slices
form one typed array; admission validates every child without copying or
draining it. The actual result moves the original payloads, including String
spare capacity, into the admitted graph. Earlier receiver closes admit an empty
array and preserve the queue. Sender close returns scalar Unit and retains its
existing prepaid transport without requiring a VM heap object.
Fixed-budget compiled regressions preserve registry identities, counts and
queued values on rejection. Split-array tests cover empty, contiguous and
wrapped queues, invalid children and types, exact/short budgets and released
reservations. Public bounded/unbounded channel tests cover generic fork/close
results, final-receiver drain order and returned values from closed sends.
The blocking bridge carries the same known host previews and admitted channel
methods through the worker's own typed result admission. Before sending its
request, an accounted worker collects unreachable heap objects using its live
verified roots and exports owned object-capacity information and its original
phase account. It stays inside the scoped host call until the response arrives;
no VM values or borrowed heap references cross this boundary. The servicing
engine uses the bridge's same immutable program to measure borrowed results,
without copying payloads or reserving space in the parent heap for a worker's
result. Complete byte, import-workspace and object-slot reservations return to
the worker with the response, and only that worker can consume its pool.

A request-state lock orders cancellation against atomic publication of the
worker result and any waiting parent peer. Cancellation before publication
rejects the batch; after publication the worker waits for its response before
resuming or releasing its paused heap. Rejected parent batches restore the
worker token for ordinary RAII cleanup. Host errors and disconnected replies
release the worker pool without taking back an independently committed peer's
pool. The additional context and locked-state storage retain the original
phase charge until their last owner drops. Their byte count is bound by
`worker_host_import_context_bytes` in the resource profile; it describes this
import context, not every bridge metadata allocation or process RSS.

Tests cover independent heaps and accounts, shared quotas, exact/short bytes
and object limits, invalid recipients, cancellation on either side of commit,
actual worker-thread reply/disconnection/error paths, and complete release.
A compiled public test preserves channel identities when its worker result
cannot fit. Public CLI cases transfer nested generic values through bounded
and unbounded worker channels, forks, borrowed receive previews and final
receiver drain. Unconverted host operations remain separate admission
boundaries. These verified routes do not establish complete channel or T0
accounting.

General root/metadata scratch, inline function type-argument storage, and
combined host-effect/import admission for other operations remain open
boundaries. Invalid-index Array.set error replies and ordinary execution outside
test participation retain their existing admission routes. This does not close T0.

Owned console print calls and I/O Reader/Writer calls prepay their closed response
descriptors before consuming input or emitting bytes. `Reader.read` reserves at
most 96 bytes, `readAll` and Writer responses 64 bytes, and console print calls
32 bytes; unused optional descriptors are released after the call. These known
shapes transfer the reservation without a measurement walk. The profile binds
`prepaid-console32-io96-collection64/1`. `Set.insert`, `Stack.push`, and
`Queue.enqueue` also prepay their 64-byte response before modifying storage.
Reader buffer admission includes this live
response reservation, so rejection cannot advance the cursor or publish a token.

Owned `sync.Array.set` and `compareExchange`, `sync.Map.insert`, `remove` and
`compareExchange`, `sync.Set.remove`, `sync.Stack.pop`, and `sync.Queue.dequeue`
reserve their reply from the borrowed previous state before modifying storage.
Array/Map `get`, Stack/Queue `peek`, and all five collection snapshots also
reserve before copying their result. Each detached node costs 32 bytes; names
and string payloads add their exact UTF-8 lengths. Option, Result and
`CompareExchange` wrappers are included. The measurement walk admits its
64-byte parent cursors, releases that workspace, and then reserves the complete
reply. Limit rejection leaves collection content and generation identities
unchanged. The existing storage owner independently admits replacement growth;
its rejection also releases the receiving reply reservation. Exact reservations
move through synchronous delivery or the immediate async ready queue.
All five collection literals prepay their 32-byte returned token before copying
retained values or publishing collection/generation identities. Map and Set
deduplication use borrowed iteration without a size-dependent temporary index;
Map retains first key position and last value. Collection storage and response
can belong to the same or independent accounts; either rejection releases the
tentative reservations and preserves identities.

Cursor start prepays its 64-byte cutoff tuple. Cursor next reserves 96 framing
bytes plus the selected payload, or 128 plus both key and value for Map, before
copying. An exhausted cursor reserves 32 bytes. Registry kind and generation
count are validated even for empty/exhausted cursors. Iteration order, cutoff
and reinsertion semantics are unchanged. The resource profile binds
`prepaid-collection-reply-value32-walk64-literal32-cursor64-96-128/2` to this
boundary. Other host construction and combined host/VM admission remain open.

Mutex/RwLock acquisition replies prepay 64 bytes before taking a guard;
Semaphore acquisition prepays 32. Try-acquisition reserves 64 bytes on success
and 32 for an empty Option; recoverable `SyncError` results reserve 73 bytes.
Pending acquisition first checks availability, so a still-parked request does
not reserve a response. Once available, response and guard storage must both
fit before taking the lock, incrementing readers or consuming a permit. The
prepared response remains charged to the request's original phase. Guard
getters reserve 32 bytes plus the borrowed payload before cloning it. Unlock
and permit release prepay their 32-byte result before consuming the guard or
changing resource state. Internal structural cleanup remains independent of
these user-call response reservations. The profile binds
`prepaid-sync-guard64-permit32-empty32-error73-ref32-release32/1` to this boundary.

`Condition.wait` validates the condition and owned mutex guard, then reserves
its 32-byte reply before releasing the mutex or publishing the waiter. That
reservation survives notification, cancellation and waiting to reacquire the
mutex. Returning the guard cannot require a new response reservation from the
polling phase. Direct wait previews and notifications also prepay 32 bytes;
invalid registry values reject before releasing or notifying anything.

Every asynchronous barrier participant reserves its 75-byte `BarrierRole`
result before changing arrivals, including single-party barriers. A generation
therefore publishes all participants with their already admitted responses.
Cancelling an incomplete generation shrinks each response to the 73-byte
`SyncError` result and resets the generation for reuse. Direct barrier previews
reserve the exact 75-byte success or 73-byte recoverable error before changing
arrivals. Cancelling a parked Mutex/RwLock/Semaphore acquisition reuses 73 bytes
of retired request metadata for its error, so an exhausted phase can retire its
waiter without acquiring the resource or requesting more memory. These replies
retain their original phase through owned poll/wait. The profile binds
`prepaid-condition32-barrier75-cancel73-reused-request/1` to this boundary.

Mutex, RwLock, Condition, Semaphore and Barrier constructors prepay their
64-byte success response; Once and Atomic constructors prepay 32 bytes.
Semaphore and Barrier validation errors prepay their exact 73-byte result.
Response and retained registry storage must both fit before publishing an
identity, including when their accounts are shared. Atomic load/swap responses
admit the current payload before copying or replacing it; store prepays Unit,
and compareExchange prepays its named variant and previous payload before any
successful exchange. The original storage owner separately admits replacement
storage. Raw-host Once views prepay their optional/reference framing and
retained payload, empty value or reentrant error. Ordinary Once initializer
execution remains a VM continuation. The profile binds
`prepaid-sync-construct32-64-error73-atomic-payload-once-ref64/1` to these rules.
Other host construction and combined host/VM admission remain open.

Direct owned channel `receive` and `tryReceive` reserve their response before
removing a buffered value or consuming a waiting sender. The framing is 32 bytes
for Option and 42 bytes for `TryReceive`, including its name; the payload uses
the same measured detached model. Rejection preserves the message and does not
acknowledge the waiting sender. Delivery moves the retained payload without
copying it. Empty/closed results retain only their framing. An open blocking
receive still requires the scheduler, and invalid endpoints remain host errors.
The profile binds `prepaid-direct-receive-option32-try42/1`.

Direct and pending sends admit their 64-byte success acknowledgement before
buffer publication or delivery. Rendezvous reserves both responses before
moving the message. Tentative sender admission releases on receiver rejection;
a sender quota failure retires only that sender and delivers no payload. The
receiver can continue waiting or select another admitted sender. Pending
completion charges belong to the request's original phase, including when the
other endpoint initiates delivery. Send errors reserve 73 bytes plus their
payload (`SendError`); try-send errors use 76 bytes plus the payload
(`TrySendError`). Pending errors move the retained payload. Closed pending
receives prepay their 32-byte `none` response. The profile binds
`prepaid-channel-ack64-error73-try76-end32/1` to these rules.

Channel endpoint operations also prepay their responses before publishing an
identity or changing endpoint counts. Successful bounded/unbounded construction
uses 128 bytes, fork uses 64, and sender close and iterator adoption use 32.
Rejected bounded capacities use 76 bytes for their `ChannelError` result.
Receiver close admits 32 bytes plus every queued value when closing the final
receiver; earlier receiver closes return an empty 32-byte array. A failed reply
reservation leaves endpoint identities, counts, iterator adoption and the FIFO
queue unchanged. The original storage owner separately admits new channel and
endpoint storage. The profile binds
`prepaid-channel-new128-fork64-close32-values/1` to this boundary.

Immediate async dispatch retains the admitted response alongside its request
reservation while the result is pending in the host. Pending channel delivery reserves
its receiving result before committing and retains that reservation separately
from request metadata. Owned poll and wait completions move the response charge
into the VM's ready queue; changing the currently active phase does not reassign
it, and delivery does not reserve the same payload again. Raw detached host
entrypoints remain reference access rather than the VM's owning transport.

Cancelling a ready response reuses 64 bytes of retired request metadata for the
cancellation envelope. The old detached value is dropped before releasing its
payload reservation; reusable bytes replenish the request reserve so repeated
cancellation remains bounded. Unused payload bytes are released. Remaining
host response construction and pending-operation admission still require their
own preflight; this ownership route alone does not close `UTEST-LIMIT-001`.

Managed `Copy + Discard` block locals and temporaries stop being VM roots after
their scope's defer and task drains. This includes loop `break` and `continue`
edges, so completed iterations do not retain old payloads until their slots
happen to be overwritten. This does not introduce a last-use optimizer or
shorten function-wide parameter and return storage.

Place validation reserves 32 bytes per resolved path, 64 per projection and
eight per selected slice index. A multi-place check additionally reserves its
32-byte-per-path working table before allocating it. Detached map keys use the
same complete snapshot admission as call arguments; their payload reservation
moves into the path without releasing or charging a second copy. Active loans
retain those paths until release, including unwind. Nested path extensions
retain the original account. Slice normalization exposes an exact index count
before materialization, preserving omitted bounds, reverse strides and extreme
steps. Diagnostic path hashing streams key text without allocating a formatted
copy. `path-32-component-64-index-8-snapshot/1` and its units are included in the
resource-profile hash.

Array slice reads and writes consume normalized indices without an index
buffer. String slices traverse UTF-8 in the selected direction, first measuring
the exact output bytes and then constructing the result. They allocate no
character table or full source copy. String and Array slice results reserve
their heap header and final buffer capacity before construction and transfer
that reservation into the heap slot. `streamed-indices-preadmitted-heap-buffer/1`
identifies this route in the resource profile. Array element copies retain
their normal value semantics and separate heap charges. This does not cover
all general mutation or copy-on-write workspace.

`String.length()` and `String.byteLength()` measure the managed String directly
in the VM, without a detached copy. The former counts Unicode scalar values;
the latter counts UTF-8 bytes. `managed-length-and-byte-length/1` identifies
this route in the resource profile. The hosted reference methods retain the
same observable result.

An asynchronous host call reserves 128 bytes of job metadata and the callable
name before starting work. Synchronization routes that can retain pending
arguments also reserve snapshot capacity. An immediately completed operation
borrows the caller's admitted arguments and charges its own retained output;
it does not reserve a second argument copy. The profile binds
`pending-sync-argument-capacity/1` to that distinction. The job retains
the creating phase's account through pending and ready states. Polling uses that
account and restores the caller's account afterward; completion or cancellation
releases the reservation when no pending or ready record remains. These values
are included in the resource-profile hash. Remaining host snapshots and payloads
still require separate accounting; this reservation does not claim those bytes.

A channel reserves 256 descriptor bytes, 32 per configured queue slot and 32
per endpoint before publishing any identity. An unbounded channel reserves no
configured slots, but every queued detached value still charges the channel's
original owner. Failed growth or endpoint forks preserve the queue and endpoint
counts. Receiving and explicit close release queued storage; the last closed
endpoint and retired waiter release the channel entry. Pending receive results
are admitted against the receiving phase before matching or dequeuing a value.
A receiving phase's resource failure retires that receive and preserves the
sender's payload and terminal. Direct send/receive operations also fulfill an
existing opposite waiter exactly once, without requiring a second queued call.

Public channel regressions cover constructor rejection even when a language
error is handled, bounded and unbounded retained payloads, and repeated drained
payloads. They preserve siblings and do not retry a runner resource limit.

`testing.diffText` and `assertTextEqual` admit the complete bounded diff workspace
before allocating line views, Myers frontiers, pending partitions or hunk text.
The linear reservation uses 32 logical bytes for the result descriptor, 96 for
pending output ranges, 16 per line view, 32 per frontier slot in each direction,
48 per possible partition slot, and 32 plus text bytes per retained hunk. The
partition bound is twice the total line count plus one; each frontier has twice
the ceiling of half that count plus three slots. Hunk text is bounded by both
the input prefixes and the output allowance. Equal inputs reserve only the
result descriptor. `TextDiffPlan::memory_bytes` is the executable formula; the
resource profile binds its model identifier. After computing, the workspace
reservation shrinks to the actual retained descriptor and hunks. That charge
stays with its creating phase during conversion to the public nominal record.
The conversion admits 200 bytes for the record and its fields, plus 76 bytes per
hunk and its String descriptor. The old and new descriptor vectors overlap;
hunk string payloads move without duplication. The profile binds these units
and `moved-hunks-overlapping-descriptors/1`. The result then belongs to ordinary
VM value storage, without a host registry token. The complete detached return
lifetime remains open. Rendering and assertion-message construction admit
their exact output size before copying any bytes, including when a caller
constructs oversized hunks directly.

Generic assertion diagnostics admit 32 logical bytes plus their exact formatted
message length before construction. Each displayed value reserves its 32-byte
descriptor, 1024-byte UTF-8 prefix and truncation marker before copying. Intrinsic
scalar and Array display streams into that prefix, using an admitted iterative
cursor stack at 64 logical bytes per retained slot. User-defined `Display`
executes under the same phase limits before its returned String is truncated.
The resource profile binds `static-display-utf8-prefix-1024-frame-64/1` alongside
the hosted `utf8-prefix-1024/1` formatter. Successful assertions skip Display;
Result assertions move their successful payload without a detached snapshot.
A quota failure leaves the assertion terminal unpublished; the runner records
the memory limit. This admission covers message construction, not the still-open
general detached-value transport boundary.

When a synchronous callback exhausts a phase, its VM frames retain a caller
continuation until structural unwind reaches the test boundary. Every function
that owns cleanup retains one verified empty unwind drain, even a pure infinite
loop with no ordinary exit. The drain removes registered entries and performs
required structural cleanup; explicit user defers remain subject to the
existing exhausted-budget rule. Memory and instruction failures inside Display
preserve sibling results and remain non-retryable.

After an exhausted phase drains, host retirement preserves all ancestor,
sibling and pending-call roots before the next test starts. Unreachable channel
endpoints close structurally, releasing queued payloads without constructing
the array returned by the public `Receiver.close` call. Final engine retirement
uses the same route and preserves detached return roots. Ordinary collection
still rejects abandonment of a receiver with pending values. This retirement
does not run user callbacks or convert the original resource terminal into an
infrastructure failure.

Synchronous callbacks also poll host cancellation between instructions. They
yield to the same scheduler unwind when a phase deadline or external interrupt
is pending. Timeout remains retryable and preserves sibling execution; external
interruption stops subsequent tests. Neither route becomes a language panic or
an instruction-limit failure. Cleanup callbacks are not interrupted again by
the request that initiated their drain.

`String.replace`, `trim`, `toLowerAscii` and `toUpperAscii` admit their exact
UTF-8 output length and one 32-byte detached descriptor before construction.
Replacement counts non-overlapping matches without materializing output; an
empty needle matches scalar boundaries, preserving Unicode and CR/LF bytes.
The profile binds `exact-utf8-text-output-32/1`. Exact-budget and one-byte-short
host checks cover construction; the public CLI also verifies Unicode results,
non-retryable expansion exhaustion and sibling continuation. General detached
transport lifetimes and the other text constructors remain separate open work.

Hosted generators and float tolerances each retain one 32-byte descriptor
charge until their last live reference is reclaimed. `GenerationId` is an
ordinary record whose construction admits 108 bytes. The public
`FloatToleranceError`, `TempError` and `GenerationError` enums use nominal
variants with typed causes; their Result constructions admit 83, 73 and 79 bytes
respectively and create no registry entry. Their detached transport lifetime
remains separate from this construction proof. Generator scalar results reserve their detached Result wrapper
and scalar before advancing the stream. Bytes/text generation first predicts
the exact selected buffer capacity on a copy of the scalar generator state;
only an admitted call advances the stream. Bytes retain their usual buffer
charge, while text output storage is reserved during construction before VM
materialization. A failed quota preserves both draw count and generator state;
language bounds/exhaustion errors also preserve the complete state. The shared
kernel retains the specified sequence for successful calls.

Shrinking uses the same 32-byte detached-value descriptors plus string payload
bytes for both returned and temporary candidates. The outer Result and each
temporary candidate array also reserve their descriptors. A nested replacement
moves its existing charge to the parent result without a release/reacquire gap
or an additional copy. Rejected admission drops all partial candidates and
leaves no result or host handle published. The `candidate-32-utf8-depth64/1`
model binds this construction protocol; the existing 4096-candidate and
64-level depth limits remain in effect. Focused tests cover exact peak/retained
accounting, deduplication, move accounting, independent candidate order and
quota rejection through the public runner.
Phase admission precedes the ordinary hosted byte cap: handling a
`GenerationError` cannot absorb a runner memory quota. A separate hosted byte
cap still returns its language error when the phase has enough memory.

Public regressions distinguish many retained buffers, which exhaust the cap,
from successive discarded buffers, which are reclaimed. Concurrent blocking
jobs also retain their live and returned buffers while temporary ones are
collected. Concurrent collection tests cover retained/discarded values, atomic
growth rejection, removal, duplicate admission and cross-phase ownership.
The shared account does not yet include other hosted payloads,
transient host snapshots or all scheduler metadata. Their existing bounded
checks do not discharge those remaining requirements.

Task publication reserves 512 logical bytes for the retained task record and
its fixed ready/completion/parent entries, plus 32 per captured scheduler value.
The charge remains with its creating phase until the execution drops the task
table; consuming a Join does not free its retained task slot. Scope slots cost
96 bytes and have the same table lifetime. A selected-call region reserves 64
bytes plus 128 per arm before allocating its arm table, and releases that
reservation when the region closes. Hosted and blocking task admission happens
before starting external work.

Explicit and fallback cleanup records reserve 96 bytes plus 32 per capture or
guard projection and the bytes of retained assertion text. An owner guard
reserves at least one projection slot, covering every target admitted by the
verifier for a later transfer. Its retarget therefore cannot need additional
memory after the value has moved. A Store and its immediately following cleanup
transitions are charged together before executing and finish before yielding or
observing cancellation. Executing, replacing
or disarming a cleanup releases its reservation. Public tests distinguish 256
simultaneously registered defers, which exhaust a 16 KiB cap, from 512 successive
executed defers, which reuse that allowance. A separate regression bounds the
retained metadata of 512 successive spawned and awaited tasks at 32 KiB.
An instruction-boundary sweep covers interruption before a spawned Join's
fallback is registered. Scope unwind consumes only completions still owned by
that scope, after reaping children and collecting their panics; completion
ownership already transferred to a Group remains with that Group.

`AsyncIterator.collect` charges each retained temporary element before growing
its buffer and transfers storage to the admitted result array. Test-node quota
exhaustion stays a runner resource limit, including through `spawn`; handling a
language `CollectionError` cannot turn it into a passing test. Ordinary negative
collection limits retain their recoverable language error.

VM one-shot, Once and Group state reserves 256 descriptor bytes. Group children
add 32 bytes each before transferring a Join, and release their charge when
removed. Removing a group with waiting consumers leaves its state and charge
intact. One-shot result storage is admitted before publishing either token.

Executor pools reserve 256 descriptor bytes, 256 per worker and 32 per admitted
queue slot. Actors reserve 256 bytes and 32 per mailbox slot, state value or
captured step argument. This is a reservation for configured capacity, not RSS
or a statement that every slot is occupied. The result wrapper is admitted
before publishing the constructor's state, advancing its identity or starting
blocking workers. Retained closed pool, actor and one-shot registry entries keep
their owner charge until execution destruction; Group removal releases its
entry immediately. Public pool and actor tests cover a fitting capacity and
resource rejection that cannot be recovered as a language error.

The resource-profile hash binds the actual VM defaults (65,536 stack frames and
1,000,000 heap objects), the sidecar limits, and separate finite allowances of
100,000,000 compiler-entry instructions and 1,000,000 structural containment
instructions. Envelope metadata defaults to 1 MiB; artifact and snapshot counts
default to 256 each. Virtual timer admission uses the sidecar's timer count,
preflights metadata, reuses a released slot and restores the previous clock's
limit after a virtual domain closes. These are the executable values; the
independent Rust model's depth and ready-queue defaults are not runtime proof.

Closed worker transport validates positive VM and envelope limits before
materializing runtime providers. Exhaustion is `resource-limit`, preserves
sibling reports and does not enter retry. Structural unwind cancels/reaps owned
work with its own finite instruction allowance; it may skip explicit user
cleanup, as permitted by testing section 7.8.

## Timeouts and interruption

`PhaseDeadline` uses monotonic integer nanoseconds. A suite can pause its own
phase while it waits for selected descendants; paused time is excluded from
the setup/teardown deadline. `None` represents an intentionally disabled
wall-clock deadline for non-sidecar consumers and does not disable any
structural budget. `InterruptController` models the first cancellation
request, one finite grace period and forced termination of a non-cooperative
worker. Clock regressions are rejected rather than wrapped. These are model
operations. The public worker now emits bounded, sequenced node and cleanup
transitions to a coordinator watchdog. Each active phase has its own deadline;
the coordinator pauses the parent while a descendant executes and applies the
closed setup/teardown caps, reduced by an explicit CLI timeout when supplied.

The 2026-09-07 audit exercised one suite with two leaves, each awaiting
`time.sleep(time.Duration.fromNanoseconds(350000000))`. Each selected alone
passed with `--timeout 500ms`. Selecting both caused exit 3 and
`test worker timed out`, while both passed with `--timeout 1500ms`. This
contradicted the independent body/setup/teardown deadlines in testing section
7.8. The regression now passes with independent setup, nested leaves and
teardown. A deadline request names the active phase generation; the VM cancels
that test boundary, drains observable cleanup and preserves sibling execution.
Timeout is reported separately from language panic or external interruption.

The coordinator allows the bounded cleanup grace before reaping a worker that
does not respond. Failure to establish clean isolation is infrastructure, and
does not produce a successful report. Worker bootstrap and final transport also
have a finite 30-second envelope. Completed leaf results survive a suite
teardown timeout. Retry integration uses the same immutable compiled artifact
and independent process for each selected retry unit.

This does not close all structural limits per phase or prove every native and
non-cooperative cleanup route. `UTEST-LIMIT-001` and T0 remain open until those
remaining integration boundaries and the required gates are verified.
