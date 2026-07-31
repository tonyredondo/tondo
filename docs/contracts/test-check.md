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
`async? fn(): Unit ! E`:

- `Unit` and `Never` are the only admitted normal results;
- a test may use bare `return`, but cannot return a value;
- suite setup cannot use `return` at all (`E1205`);
- the error union is normalized by nominal name, must be duplicate-free and
  every member must satisfy `Discard`; and
- `await` and the virtual-time operations infer async without an `async test`
  spelling.

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

`withVirtualTime` requires an async `Send + CallOnce` closure accepting
`ref VirtualTime`, returning `Unit`, and neither escaping nor sharing the
controller. The controller is therefore opaque and cannot become a Tondo value
or capability.

All test-only operations are rejected with `E2003` when presented by a
production source. There is no friend flag or runtime context lookup in this
contract.
