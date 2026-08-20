# Static test-body contract

**Status:** implemented for `UTEST-CHECK-001`

`tondo_compiler::test_check` is the small semantic adapter between ordinary
Tondo checking and the hidden entries used by the test target. It does not
execute a body, create an envelope, or relax language rules. The resolver and
HIR checker remain responsible for names, types, ownership, loans, terminals,
`Send`, `Share` and `unsafe`; this boundary consumes those facts and rejects a
failed proof.

## Hidden entry shape

Both a test body and a suite setup are checked as a private
`fn(): Unit ! E`; the checker records the inferred suspendible effect when the
body calls a suspendible operation, iterates an `AsyncIterator`, uses `await` or
registers suspendible cleanup:

- `Unit` and `Never` are the only admitted normal results;
- a test may use bare `return`, but cannot return a value;
- suite setup cannot use `return` at all (`E1205`);
- the error union is normalized by nominal name, must be duplicate-free and
  every member must satisfy `Discard`; and
- suspendible calls and the virtual-time operations infer suspension without an
  extra keyword or an `async test` spelling; direct calls wait implicitly.

The resulting `TestBodyContract` is immutable input for lowering. It carries
the exact operation list and the ordinary facts needed by later admission
verifiers.

## Sealed operations

The checker admits only the monomorphic shapes of `std.testing`:
`log(String)`, `tags(Map[String, String])`, `failNow(String)`,
`skip(String)`, `attach(String, String, bytes.Bytes)`, `snapshot(String,
String)`, plus `withVirtualTime`, `settle` and `advance(Duration)`. Attachment
and snapshot names are unique per entry and canonical; media types contain one
slash and no whitespace. Duplicate evidence receives the corresponding P-code
family, while negative or overflowing virtual durations are rejected before
runtime.

`withVirtualTime` requires a suspendible `Send + CallOnce` closure accepting
`ref VirtualTime`, returning `Unit`, and neither escaping nor sharing the
controller. The boundary itself must be called directly rather than spawned;
its result is awaited implicitly (`await` on the direct call is rejected);
controlled tasks are spawned inside the callback's structured scope. The
controller is therefore opaque and cannot become a Tondo value or capability.

All test-only operations are rejected with `E2003` when presented by a
production source. There is no friend flag or runtime context lookup in this
contract.
