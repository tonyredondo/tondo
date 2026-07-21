# Bootstrap target contract

**Status:** accepted for M0

The initial target is:

~~~text
name                 = tondo-vm-hosted
diagnostic source ID = target:tondo-vm-hosted
profile              = hosted
edition              = 0.1
capabilities         = []
~~~

An empty capability set is deliberate. Console output currently exists only as
a future bootstrap shim and is not yet exposed to Tondo source. Process,
filesystem, network, threads, FFI, and other hosted capabilities remain absent
until their contracts and runtime paths exist.

The VM target is a real target identity, not shorthand for the current host
machine. Adding a capability changes build identity and applicable conformance
cases. A missing capability must eventually reject the API during compilation;
it cannot install a runtime stub that always fails.
