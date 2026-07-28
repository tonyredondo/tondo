# Bootstrap target contract

**Status:** released for Tondo 0.1

The initial target is:

~~~text
name                 = tondo-vm-hosted
diagnostic source ID = target:tondo-vm-hosted
profile              = hosted
edition              = 0.1
capabilities         = [console, process]
~~~

`console` exposes the provisional `std.console.print(String): Unit` shim in
`bootstrap-host.md`. `process` exposes the closed surface in
`process-host.md`. Filesystem, network, FFI, and other hosted capabilities
remain absent until their contracts and runtime paths exist. A custom request
may omit either registered capability; its selected bootstrap standard package
then omits the corresponding module, and an import is rejected with `E1008`
rather than reaching a failing runtime stub.

The VM target is a real target identity, not shorthand for the current host
machine. Adding a capability changes build identity and applicable conformance
cases. A missing capability must eventually reject the API during compilation;
it cannot install a runtime stub that always fails.

The versioned registry is `tondo-capabilities/1` and recognizes exactly
`process`, `threads`, `filesystem`, `network`, `console`, `environment`,
`clock`, `entropy`, and `dynamic-linking`. Recognition is distinct from target
support: `tondo-vm-hosted` supports only `console` and `process`, so requesting
the registered `network` capability is a project error rather than a silently
ignored feature.

Project manifests record registry, target, profile, selected capabilities, and
features. Source-set conditions are resolved against those values before any
source bytes are requested or lexed. Interfaces and artifacts retain the same
identity and are rejected when mixed across targets. The concrete format is
defined in `../../TONDO_TOOLCHAIN_SPEC.md`.
