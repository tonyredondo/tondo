# Static test-body contract

**Status:** ordinary compiler checking and public hosted execution are verified
through the testing gate. The independent model retains its separate scope.

`tondo_compiler::test_check` is an independent bounded model of supplied test
body facts. Its success alone does not establish the compiler's public path.
The executable path uses the ordinary resolver and HIR checker for names,
types, ownership, loans, terminals, `Send`, `Share` and `unsafe`. Compiler-owned
leaf and suite callbacks infer `Unit ! E` through the same closure inference
used by ordinary generic functions. Their error parameter requires `Discard`.

## Hidden entry shape

Both a test body and a suite setup are checked as a private
`fn(): Unit ! E`; the checker records the inferred suspendible effect when the
body calls a suspendible operation, iterates an `AsyncIterator`, uses `await` or
registers suspendible cleanup:

- `Unit` and `Never` are the only admitted normal results;
- a test may use `return` with a compatible `Unit` outcome;
- suite setup cannot use `return` at all (`E1205`);
- repeated inferred error members collapse to one nominal union member, and
  every member must satisfy `Discard` (`E1105`); incompatible normal or return
  types use the ordinary `E1102` diagnostic; and
- suspendible calls and the virtual-time operations infer suspension without an
  extra keyword or an `async test` spelling; direct calls wait implicitly.

The model's `TestBodyContract` is immutable evidence for its supplied facts;
public lowering consumes checked HIR. A suite rejects its own `return` with
`E1205`, while an ordinary nested closure retains its own return boundary.
Nested test closures also keep independent inferred error unions.

The hosted VM consumes each hidden entry's recoverable error after ordinary
cleanup and records `failed-error`. The error type, terminal source location
and prior envelope remain associated with that attempt. Failed setup blocks
selected descendants through their suite attempt; unrelated roots continue.
Retries use fresh participation processes and preserve every prior attempt.
Error payloads are not implicitly serialized or reflected into reports.

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
`ref VirtualTime`, returning `Unit ! E`, and neither escaping nor sharing the
controller. The error union propagates unchanged. The boundary itself must be called directly rather than spawned;
its result is awaited implicitly (`await` on the direct call is rejected);
controlled tasks are spawned inside the callback's structured scope. The
controller is therefore opaque and cannot become a Tondo value or capability.

All test-only operations are rejected with `E2003` when presented by a
production source. There is no friend flag or runtime context lookup in this
contract.
