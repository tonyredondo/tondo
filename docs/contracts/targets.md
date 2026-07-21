# Bootstrap target contract

**Status:** accepted for M0

The initial target is:

~~~text
name                 = tondo-vm-hosted
diagnostic source ID = target:tondo-vm-hosted
profile              = hosted
edition              = 0.1
capabilities         = [console]
~~~

`console` exposes only the provisional `std.console.print(String): Unit` shim
recorded in `bootstrap-host.md`. Process, filesystem, network, threads, FFI, and
other hosted capabilities remain absent until their contracts and runtime paths
exist. A custom request may omit `console`; its selected bootstrap standard
package then omits `std.console`, and an import is rejected with `E1008` rather
than reaching a failing runtime stub.

The VM target is a real target identity, not shorthand for the current host
machine. Adding a capability changes build identity and applicable conformance
cases. A missing capability must eventually reject the API during compilation;
it cannot install a runtime stub that always fails.
